use std::env;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use anyhow::{bail, Context, Result};
use ringbuf::traits::Producer;

use echoless_core::ReferenceChannels;

use super::resample::InterleavedLinearResampler;

// tap 的默认/常见采样率;实际值由 helper 的 ELTP 流头上报(跟随系统输出设备),
// 与管线不一致时 reader 线程插固定比率线性重采样(A5)。
pub const SAMPLE_RATE: u32 = 48_000;

// --stream-stdout 流头:magic "ELTP" + u32 version + u32 sample_rate + u32 channels(全 LE)。
const STREAM_HEADER_MAGIC: &[u8; 4] = b"ELTP";
const STREAM_HEADER_LEN: usize = 16;

const HELPER_ENV: &str = "ECHOLESS_PROCESS_TAP_HELPER";
const DEV_HELPER: &str = "tools/macos-process-tap-poc/.build/echoless-process-tap-poc";

pub struct MacProcessTapStream {
    child: Child,
    reader: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
}

impl Drop for MacProcessTapStream {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        let _ = self.child.kill();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let _ = self.child.wait();
    }
}

pub fn helper_available() -> bool {
    helper_path().is_ok()
}

pub fn helper_path() -> Result<PathBuf> {
    if let Ok(path) = env::var(HELPER_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        bail!(
            "{HELPER_ENV} 指向的 Process Tap helper 不存在: {}",
            path.display()
        );
    }

    if let Some(path) = current_exe_neighbor("echoless-process-tap-poc")? {
        return Ok(path);
    }
    if let Some(path) = current_exe_neighbor("echoless-process-tap")? {
        return Ok(path);
    }

    for base in current_dir_ancestors()? {
        let candidate = base.join(DEV_HELPER);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    bail!(
        "未找到 macOS Process Tap helper;先运行 tools/macos-process-tap-poc/build.sh,或设置 {HELPER_ENV}"
    )
}

/// 无弹窗查询系统音频录制授权状态(helper 走私有 TCCAccessPreflight)。
/// 与 probe 不同:绝不触发授权弹窗,可在普通 doctor 里随时调用。
/// helper 缺失 / 执行失败 / 输出不认识 → None(调用方回退 "unknown")。
pub fn preflight_permission() -> Option<&'static str> {
    let helper = helper_path().ok()?;
    let output = Command::new(&helper)
        .arg("--preflight-permission")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    match String::from_utf8_lossy(&output.stdout).trim() {
        "granted" => Some("granted"),
        "denied" => Some("denied"),
        "undetermined" => Some("undetermined"),
        _ => None,
    }
}

pub fn probe_permission() -> Result<String> {
    let helper = helper_path()?;
    let output = Command::new(&helper)
        .arg("--probe-permission")
        .arg("--mono")
        .output()
        .with_context(|| format!("启动 macOS Process Tap 权限探测失败: {}", helper.display()))?;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        return Ok(if stderr.is_empty() {
            "Process Tap permission probe succeeded.".to_string()
        } else {
            stderr
        });
    }
    bail!(
        "Process Tap permission probe failed with status {}: {}",
        output.status,
        if stderr.is_empty() {
            "(no stderr)"
        } else {
            &stderr
        }
    )
}

pub fn start<P>(
    mode: ReferenceChannels,
    target_rate: u32,
    producer: P,
    drops: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
) -> Result<MacProcessTapStream>
where
    P: Producer<Item = f32> + Send + 'static,
{
    let helper = helper_path()?;
    let mut command = Command::new(&helper);
    command.arg("--stream-stdout");
    command
        .arg("--exclude-pid")
        .arg(std::process::id().to_string());
    if mode == ReferenceChannels::Mono {
        command.arg("--mono");
    }
    command.stdout(Stdio::piped()).stderr(Stdio::inherit());

    let mut child = command
        .spawn()
        .with_context(|| format!("启动 macOS Process Tap helper 失败: {}", helper.display()))?;
    let stdout = child
        .stdout
        .take()
        .context("macOS Process Tap helper stdout 未打开")?;
    let reader_running = running.clone();
    let channels = usize::from(mode.channel_count());
    let reader = thread::spawn(move || {
        read_pcm_stream(stdout, channels, target_rate, producer, drops, reader_running)
    });

    Ok(MacProcessTapStream {
        child,
        reader: Some(reader),
        running,
    })
}


fn read_pcm_stream<P>(
    mut stdout: impl Read,
    channels: usize,
    target_rate: u32,
    mut producer: P,
    drops: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
) where
    P: Producer<Item = f32>,
{
    let mut read_buf = [0u8; 16 * 1024];
    let mut pending = Vec::<u8>::with_capacity(16 * 1024);
    // 流头解析:None = 还没读够 16 字节判定
    let mut resampler: Option<InterleavedLinearResampler> = None;
    let mut header_done = false;
    let mut samples = Vec::<f32>::with_capacity(4 * 1024);

    while running.load(Ordering::SeqCst) {
        match stdout.read(&mut read_buf) {
            Ok(0) => break,
            Ok(n) => {
                pending.extend_from_slice(&read_buf[..n]);
                if !header_done {
                    if pending.len() < STREAM_HEADER_LEN {
                        continue;
                    }
                    if &pending[..4] == STREAM_HEADER_MAGIC {
                        let rate = u32::from_le_bytes(pending[8..12].try_into().unwrap());
                        if rate != 0 && rate != target_rate {
                            eprintln!(
                                "macOS Process Tap: 系统输出 {rate} Hz ≠ 管线 {target_rate} Hz,启用线性重采样"
                            );
                            resampler = Some(InterleavedLinearResampler::new(
                                rate,
                                target_rate,
                                channels,
                            ));
                        }
                        pending.drain(..STREAM_HEADER_LEN);
                    } else {
                        // 旧版 helper 没有流头:按默认 48k 处理,这批字节就是 PCM。
                        eprintln!("macOS Process Tap: helper 未上报流头,按 {SAMPLE_RATE} Hz 处理");
                        if SAMPLE_RATE != target_rate {
                            resampler = Some(InterleavedLinearResampler::new(
                                SAMPLE_RATE,
                                target_rate,
                                channels,
                            ));
                        }
                    }
                    header_done = true;
                }
                let complete = pending.len() / 4 * 4;
                samples.clear();
                for chunk in pending[..complete].chunks_exact(4) {
                    samples.push(f32::from_bits(u32::from_le_bytes([
                        chunk[0], chunk[1], chunk[2], chunk[3],
                    ])));
                }
                if complete > 0 {
                    pending.drain(..complete);
                }
                if let Some(rs) = resampler.as_mut() {
                    for sample in rs.process(&samples) {
                        if producer.try_push(sample).is_err() {
                            drops.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                } else {
                    for &sample in &samples {
                        if producer.try_push(sample).is_err() {
                            drops.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
            Err(err) => {
                eprintln!("macOS Process Tap helper 读取失败: {err}");
                break;
            }
        }
    }
}

fn current_exe_neighbor(name: &str) -> Result<Option<PathBuf>> {
    let path = env::current_exe()?;
    let Some(dir) = path.parent() else {
        return Ok(None);
    };
    let candidate = dir.join(name);
    Ok(candidate.is_file().then_some(candidate))
}

fn current_dir_ancestors() -> Result<Vec<PathBuf>> {
    let cwd = env::current_dir()?;
    Ok(cwd.ancestors().map(Path::to_path_buf).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_dir_ancestors_includes_current_directory() {
        let cwd = env::current_dir().unwrap();
        let ancestors = current_dir_ancestors().unwrap();
        assert_eq!(ancestors.first(), Some(&cwd));
    }
}
