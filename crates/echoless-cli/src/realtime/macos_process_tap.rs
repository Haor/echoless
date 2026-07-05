use std::env;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ringbuf::traits::Producer;

use echoless_core::ReferenceChannels;

use super::resample::InterleavedInputResampler;

// tap 的默认/常见采样率;实际值由 helper 的 ELTP 流头上报(跟随系统输出设备),
// 与管线不一致时 reader 线程插固定比率 rubato 重采样。
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
        // kill 关闭 helper stdout 后,reader 的 read 返回 0 自然退出。但 helper
        // 若陷入内核不可中断状态,SIGKILL 延迟生效,无限期 join 会拖住整个
        // 停机(审计 B-19)。限时等待:超时则不 join 也不 wait(阻塞),reader
        // 线程随本进程退出回收;孤儿 helper 由其 getppid 自愈兜底(B-01)。
        let mut exited = false;
        for _ in 0..20 {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    exited = true;
                    break;
                }
                Ok(None) => thread::sleep(Duration::from_millis(25)),
                Err(_) => break,
            }
        }
        if exited {
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        } else {
            eprintln!("macOS Process Tap helper 未及时退出,跳过 reader 回收(自愈兜底接管)");
        }
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
        // Current helper intentionally maps private-TCC denied/unknown outputs to
        // "undetermined" so the UI can use the request path first. Keep this arm
        // defensive for older helpers or future contract changes.
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
        read_pcm_stream(
            stdout,
            channels,
            target_rate,
            producer,
            drops,
            reader_running,
        )
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
    let mut resampler: Option<InterleavedInputResampler> = None;
    let mut header_done = false;
    // tap 实际交织声道数;默认按请求值,流头上报不一致时以头为准(审计 B-05)。
    let mut source_channels = channels;
    let mut samples = Vec::<f32>::with_capacity(4 * 1024);
    let mut remapped = Vec::<f32>::with_capacity(4 * 1024);

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
                        let hdr_channels = u32::from_le_bytes(pending[12..16].try_into().unwrap());
                        // 声道协商(审计 B-05):helper 上报的是 tap 实际格式,
                        // Core Audio 不保证严格满足 mono/stereo 请求;若仍按请求值
                        // 解读交织流,整条参考会声道错位,AEC 静默失效。
                        if hdr_channels != 0 && hdr_channels as usize != channels {
                            eprintln!(
                                "macOS Process Tap: tap 实际 {hdr_channels} 声道 ≠ 请求 {channels} 声道,按实际解读并适配"
                            );
                            source_channels = hdr_channels as usize;
                        }
                        if rate != 0 && rate != target_rate {
                            eprintln!(
                                "macOS Process Tap: 系统输出 {rate} Hz ≠ 管线 {target_rate} Hz,启用 rubato 重采样"
                            );
                            resampler =
                                Some(InterleavedInputResampler::new(rate, target_rate, channels));
                        }
                        pending.drain(..STREAM_HEADER_LEN);
                    } else {
                        // 旧版 helper 没有流头:按默认 48k 处理,这批字节就是 PCM。
                        eprintln!("macOS Process Tap: helper 未上报流头,按 {SAMPLE_RATE} Hz 处理");
                        if SAMPLE_RATE != target_rate {
                            resampler = Some(InterleavedInputResampler::new(
                                SAMPLE_RATE,
                                target_rate,
                                channels,
                            ));
                        }
                    }
                    header_done = true;
                }
                // 按整帧消费(4 字节 × 实际声道):半帧留 pending 下轮,
                // 否则声道适配的逐帧处理会丢样本导致交织错位。
                let frame_bytes = 4 * source_channels;
                let complete = pending.len() / frame_bytes * frame_bytes;
                samples.clear();
                for chunk in pending[..complete].chunks_exact(4) {
                    samples.push(f32::from_bits(u32::from_le_bytes([
                        chunk[0], chunk[1], chunk[2], chunk[3],
                    ])));
                }
                if complete > 0 {
                    pending.drain(..complete);
                }
                let adapted: &[f32] = if source_channels == channels {
                    &samples
                } else {
                    remap_interleaved(&samples, source_channels, channels, &mut remapped);
                    &remapped
                };
                if let Some(rs) = resampler.as_mut() {
                    for &sample in rs.process(adapted) {
                        if producer.try_push(sample).is_err() {
                            drops.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                } else {
                    for &sample in adapted {
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

/// 交织声道适配:source→target。降声道 = 逐帧平均下混;升声道 = 循环复制源声道。
fn remap_interleaved(samples: &[f32], source: usize, target: usize, out: &mut Vec<f32>) {
    out.clear();
    for frame in samples.chunks_exact(source) {
        if target == 1 {
            out.push(frame.iter().sum::<f32>() / source as f32);
        } else {
            for t in 0..target {
                out.push(frame[t % source]);
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
    use ringbuf::traits::{Consumer, Split};
    use ringbuf::HeapRb;

    #[test]
    fn current_dir_ancestors_includes_current_directory() {
        let cwd = env::current_dir().unwrap();
        let ancestors = current_dir_ancestors().unwrap();
        assert_eq!(ancestors.first(), Some(&cwd));
    }

    // 审计 B-05:流头声道数与请求不一致时以头为准,逐帧下混到请求布局。
    #[test]
    fn stream_header_channel_mismatch_downmixes_to_requested_layout() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(STREAM_HEADER_MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes()); // tap 实际 stereo
        for s in [1.0f32, 0.0, 0.5, 0.25] {
            bytes.extend_from_slice(&s.to_le_bytes());
        }

        let (producer, mut consumer) = HeapRb::<f32>::new(16).split();
        let drops = Arc::new(AtomicU64::new(0));
        let running = Arc::new(AtomicBool::new(true));
        read_pcm_stream(
            std::io::Cursor::new(bytes),
            1, // 管线请求 mono
            48_000,
            producer,
            drops.clone(),
            running,
        );

        let out: Vec<f32> = std::iter::from_fn(|| consumer.try_pop()).collect();
        assert_eq!(out, vec![0.5, 0.375]);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn remap_interleaved_upmixes_mono_to_stereo_by_duplication() {
        let mut out = Vec::new();
        remap_interleaved(&[0.1, -0.2], 1, 2, &mut out);
        assert_eq!(out, vec![0.1, 0.1, -0.2, -0.2]);
    }
}
