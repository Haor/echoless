use std::env;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ringbuf::traits::Producer;
use serde_json::json;

use echoless_core::ReferenceChannels;

use super::frame_ring::push_interleaved_frames;
use super::resample::InterleavedInputResampler;

// --stream-stdout 流头:magic "ELTP" + u32 version + u32 sample_rate + u32 channels(全 LE)。
const STREAM_HEADER_MAGIC: &[u8; 4] = b"ELTP";
const STREAM_HEADER_LEN: usize = 16;
const STREAM_HEADER_VERSION: u32 = 1;
const STARTUP_READY_POLL: Duration = Duration::from_millis(100);
const PROCESS_TAP_HEADER_DEADLINE: Duration = Duration::from_secs(25);

const HELPER_ENV: &str = "ECHOLESS_PROCESS_TAP_HELPER";
const DEV_HELPER: &str = "tools/macos-process-tap-poc/.build/echoless-process-tap-poc";

pub struct MacProcessTapStream {
    child: Child,
    reader: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
}

#[derive(Debug, Eq, PartialEq)]
enum ReaderExit {
    StartupFailed(String),
    Stopped,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StreamHeader {
    sample_rate: u32,
    channels: usize,
}

struct ProcessTapStartConfig {
    mode: ReferenceChannels,
    target_rate: u32,
    header_deadline: Duration,
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
            eprintln!("macOS Process Tap helper did not exit in time; skipping reader join (parent-death self-heal takes over)");
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
            "Process Tap helper referenced by {HELPER_ENV} does not exist: {}",
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
        "macOS Process Tap helper not found; run tools/macos-process-tap-poc/build.sh first, or set {HELPER_ENV}"
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
        .with_context(|| {
            format!(
                "failed to launch macOS Process Tap permission probe: {}",
                helper.display()
            )
        })?;
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
    status_json: bool,
) -> Result<MacProcessTapStream>
where
    P: Producer<Item = f32> + Send + 'static,
{
    let helper = helper_path()?;
    start_with_helper(
        &helper,
        ProcessTapStartConfig {
            mode,
            target_rate,
            header_deadline: PROCESS_TAP_HEADER_DEADLINE,
        },
        producer,
        drops,
        running,
        move |message| emit_process_tap_failure(status_json, message),
    )
}

fn start_with_helper<P, E>(
    helper: &Path,
    config: ProcessTapStartConfig,
    producer: P,
    drops: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    on_error: E,
) -> Result<MacProcessTapStream>
where
    P: Producer<Item = f32> + Send + 'static,
    E: Fn(String) + Send + 'static,
{
    let mut command = Command::new(helper);
    command.arg("--stream-stdout");
    command
        .arg("--exclude-pid")
        .arg(std::process::id().to_string());
    if config.mode == ReferenceChannels::Mono {
        command.arg("--mono");
    }
    command.stdout(Stdio::piped()).stderr(Stdio::inherit());

    let mut child = command.spawn().with_context(|| {
        format!(
            "failed to launch macOS Process Tap helper: {}",
            helper.display()
        )
    })?;
    let helper_pid = child.id();
    let stdout = child
        .stdout
        .take()
        .context("macOS Process Tap helper stdout was not opened")?;
    let reader_running = running.clone();
    let channels = usize::from(config.mode.channel_count());
    let (ready_tx, ready_rx) = sync_channel(1);
    let reader = thread::spawn(move || {
        let exit = read_pcm_stream(
            stdout,
            channels,
            config.target_rate,
            producer,
            drops,
            reader_running.clone(),
            ready_tx,
        );
        handle_reader_exit(exit, &reader_running, on_error);
    });

    let mut stream = MacProcessTapStream {
        child,
        reader: Some(reader),
        running,
    };

    let startup_started = Instant::now();
    loop {
        if !stream.running.load(Ordering::SeqCst) {
            bail!("macOS Process Tap startup was cancelled");
        }
        let remaining = config
            .header_deadline
            .saturating_sub(startup_started.elapsed());
        if remaining.is_zero() {
            bail!(
                "macOS Process Tap header deadline exceeded after {} ms (pid {helper_pid})",
                config.header_deadline.as_millis(),
            );
        }
        match ready_rx.recv_timeout(STARTUP_READY_POLL.min(remaining)) {
            Ok(Ok(())) if stream.running.load(Ordering::SeqCst) => return Ok(stream),
            Ok(Ok(())) => bail!("macOS Process Tap helper stopped during startup"),
            Ok(Err(message)) => bail!("macOS Process Tap helper failed to start: {message}"),
            Err(RecvTimeoutError::Disconnected) => {
                bail!("macOS Process Tap reader stopped before reporting readiness")
            }
            Err(RecvTimeoutError::Timeout) => {
                match stream.child.try_wait() {
                    Ok(Some(status)) => {
                        bail!(
                            "macOS Process Tap helper exited before readiness with status {status}"
                        )
                    }
                    Ok(None) => {}
                    Err(err) => bail!("failed to inspect macOS Process Tap helper status: {err}"),
                }
                if startup_started.elapsed() >= config.header_deadline {
                    bail!(
                        "macOS Process Tap header deadline exceeded after {} ms (pid {helper_pid})",
                        config.header_deadline.as_millis(),
                    );
                }
            }
        }
    }
}

fn read_pcm_stream<P>(
    mut stdout: impl Read,
    channels: usize,
    target_rate: u32,
    mut producer: P,
    drops: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    ready: SyncSender<Result<(), String>>,
) -> ReaderExit
where
    P: Producer<Item = f32>,
{
    let header = match read_stream_header(&mut stdout) {
        Ok(header) => header,
        Err(message) => {
            let _ = ready.send(Err(message.clone()));
            return ReaderExit::StartupFailed(message);
        }
    };

    let source_channels = header.channels;
    if source_channels != channels {
        eprintln!(
            "macOS Process Tap: tap actually reports {source_channels} channels != requested {channels}; interpreting as reported and adapting"
        );
    }
    let mut resampler = if header.sample_rate != target_rate {
        eprintln!(
            "macOS Process Tap: system output {} Hz != pipeline {target_rate} Hz; enabling rubato resampling",
            header.sample_rate
        );
        Some(InterleavedInputResampler::new(
            header.sample_rate,
            target_rate,
            channels,
        ))
    } else {
        None
    };

    if ready.send(Ok(())).is_err() {
        return ReaderExit::Stopped;
    }

    let mut read_buf = [0u8; 16 * 1024];
    let mut pending = Vec::<u8>::with_capacity(16 * 1024);
    let mut samples = Vec::<f32>::with_capacity(4 * 1024);
    let mut remapped = Vec::<f32>::with_capacity(4 * 1024);

    while running.load(Ordering::SeqCst) {
        match stdout.read(&mut read_buf) {
            Ok(0) => {
                return ReaderExit::Failed("closed its audio stream unexpectedly (EOF)".to_string())
            }
            Ok(n) => {
                pending.extend_from_slice(&read_buf[..n]);
                // 按整帧消费(4 字节 × 实际声道):半帧留 pending 下轮,
                // 否则声道适配的逐帧处理会丢样本导致交织错位。
                let frame_bytes = 4 * source_channels;
                let complete = pending.len() / frame_bytes * frame_bytes;
                samples.clear();
                for chunk in pending[..complete].chunks_exact(4) {
                    samples.push(super::sanitize_input_sample(f32::from_bits(
                        u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
                    )));
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
                    push_interleaved_frames(&mut producer, rs.process(adapted), channels, &drops);
                } else {
                    push_interleaved_frames(&mut producer, adapted, channels, &drops);
                }
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return ReaderExit::Failed(format!("audio stream read failed: {err}")),
        }
    }

    ReaderExit::Stopped
}

fn read_stream_header(reader: &mut impl Read) -> std::result::Result<StreamHeader, String> {
    let mut bytes = [0u8; STREAM_HEADER_LEN];
    reader
        .read_exact(&mut bytes)
        .map_err(|err| format!("could not read ELTP stream header: {err}"))?;
    if &bytes[..4] != STREAM_HEADER_MAGIC {
        return Err("reported an invalid ELTP stream header magic".to_string());
    }

    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    if version != STREAM_HEADER_VERSION {
        return Err(format!(
            "reported unsupported ELTP stream version {version} (expected {STREAM_HEADER_VERSION})"
        ));
    }
    let sample_rate = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if sample_rate == 0 {
        return Err("reported an invalid zero sample rate".to_string());
    }
    let channels = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    if channels == 0 {
        return Err("reported an invalid zero channel count".to_string());
    }

    Ok(StreamHeader {
        sample_rate,
        channels,
    })
}

fn claim_reader_failure(exit: ReaderExit, running: &AtomicBool) -> Option<String> {
    let ReaderExit::Failed(message) = exit else {
        return None;
    };
    running.swap(false, Ordering::SeqCst).then_some(message)
}

fn handle_reader_exit(exit: ReaderExit, running: &AtomicBool, on_error: impl FnOnce(String)) {
    let Some(message) = claim_reader_failure(exit, running) else {
        return;
    };
    on_error(format!("macOS Process Tap helper {message}"));
}

fn emit_process_tap_failure(status_json: bool, message: String) {
    if status_json {
        super::emit::emit_stdout_line(
            json!({
                "type": "stream_error",
                "stream": "reference",
                "message": message,
                "fatal": true,
            })
            .to_string(),
        );
    } else {
        eprintln!("{message}");
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
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::AtomicUsize;

    static NEXT_HELPER_ID: AtomicUsize = AtomicUsize::new(0);

    fn stream_header(sample_rate: u32, channels: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(STREAM_HEADER_MAGIC);
        bytes.extend_from_slice(&STREAM_HEADER_VERSION.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes
    }

    struct TempHelper {
        script: PathBuf,
        header: Option<PathBuf>,
    }

    impl TempHelper {
        fn script(body: &str) -> Self {
            let id = NEXT_HELPER_ID.fetch_add(1, Ordering::Relaxed);
            let script = env::temp_dir().join(format!(
                "echoless-process-tap-test-{}-{id}.sh",
                std::process::id()
            ));
            fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
            let mut permissions = fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&script, permissions).unwrap();
            Self {
                script,
                header: None,
            }
        }

        fn with_header(after_header: &str) -> Self {
            let id = NEXT_HELPER_ID.fetch_add(1, Ordering::Relaxed);
            let base = format!("echoless-process-tap-test-{}-{id}", std::process::id());
            let header = env::temp_dir().join(format!("{base}.header"));
            fs::write(&header, stream_header(48_000, 1)).unwrap();
            let script = env::temp_dir().join(format!("{base}.sh"));
            let header_arg = header.to_string_lossy().replace('\'', "'\\''");
            fs::write(
                &script,
                format!("#!/bin/sh\n/bin/cat '{header_arg}'\n{after_header}\n"),
            )
            .unwrap();
            let mut permissions = fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&script, permissions).unwrap();
            Self {
                script,
                header: Some(header),
            }
        }
    }

    impl Drop for TempHelper {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.script);
            if let Some(header) = &self.header {
                let _ = fs::remove_file(header);
            }
        }
    }

    fn start_test_helper(
        helper: &TempHelper,
        running: Arc<AtomicBool>,
        reports: Arc<AtomicUsize>,
    ) -> Result<MacProcessTapStream> {
        start_test_helper_with_deadline(helper, running, reports, PROCESS_TAP_HEADER_DEADLINE)
    }

    fn start_test_helper_with_deadline(
        helper: &TempHelper,
        running: Arc<AtomicBool>,
        reports: Arc<AtomicUsize>,
        header_deadline: Duration,
    ) -> Result<MacProcessTapStream> {
        let (producer, _consumer) = HeapRb::<f32>::new(16).split();
        start_with_helper(
            &helper.script,
            ProcessTapStartConfig {
                mode: ReferenceChannels::Mono,
                target_rate: 48_000,
                header_deadline,
            },
            producer,
            Arc::new(AtomicU64::new(0)),
            running,
            move |_| {
                reports.fetch_add(1, Ordering::SeqCst);
            },
        )
    }

    #[test]
    fn current_dir_ancestors_includes_current_directory() {
        let cwd = env::current_dir().unwrap();
        let ancestors = current_dir_ancestors().unwrap();
        assert_eq!(ancestors.first(), Some(&cwd));
    }

    // 审计 B-05:流头声道数与请求不一致时以头为准,逐帧下混到请求布局。
    #[test]
    fn stream_header_channel_mismatch_downmixes_to_requested_layout() {
        let mut bytes = stream_header(48_000, 2); // tap 实际 stereo
        for s in [1.0f32, 0.0, 0.5, 0.25] {
            bytes.extend_from_slice(&s.to_le_bytes());
        }

        let (producer, mut consumer) = HeapRb::<f32>::new(16).split();
        let drops = Arc::new(AtomicU64::new(0));
        let running = Arc::new(AtomicBool::new(true));
        let (ready_tx, ready_rx) = sync_channel(1);
        let exit = read_pcm_stream(
            std::io::Cursor::new(bytes),
            1, // 管线请求 mono
            48_000,
            producer,
            drops.clone(),
            running,
            ready_tx,
        );

        assert_eq!(ready_rx.recv().unwrap(), Ok(()));
        assert!(matches!(exit, ReaderExit::Failed(message) if message.contains("EOF")));
        let out: Vec<f32> = std::iter::from_fn(|| consumer.try_pop()).collect();
        assert_eq!(out, vec![0.5, 0.375]);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn pcm_reader_scrubs_non_finite_samples_before_ring() {
        let mut bytes = stream_header(48_000, 1);
        for sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.25] {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        let (producer, mut consumer) = HeapRb::<f32>::new(8).split();
        let drops = Arc::new(AtomicU64::new(0));
        let (ready_tx, ready_rx) = sync_channel(1);
        let exit = read_pcm_stream(
            std::io::Cursor::new(bytes),
            1,
            48_000,
            producer,
            drops.clone(),
            Arc::new(AtomicBool::new(true)),
            ready_tx,
        );

        assert_eq!(ready_rx.recv().unwrap(), Ok(()));
        assert!(matches!(exit, ReaderExit::Failed(message) if message.contains("EOF")));
        let out: Vec<f32> = std::iter::from_fn(|| consumer.try_pop()).collect();
        assert_eq!(out, vec![0.0, 0.0, 0.0, 0.25]);
        assert!(out.iter().all(|sample| sample.is_finite()));
        assert_eq!(drops.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn process_tap_overflow_drops_a_whole_stereo_frame() {
        let mut bytes = stream_header(48_000, 2);
        for sample in [1.0f32, -1.0, 2.0, -2.0] {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        let (producer, mut consumer) = HeapRb::<f32>::new(3).split();
        let drops = Arc::new(AtomicU64::new(0));
        let (ready_tx, ready_rx) = sync_channel(1);
        let exit = read_pcm_stream(
            std::io::Cursor::new(bytes),
            2,
            48_000,
            producer,
            drops.clone(),
            Arc::new(AtomicBool::new(true)),
            ready_tx,
        );

        assert_eq!(ready_rx.recv().unwrap(), Ok(()));
        assert!(matches!(exit, ReaderExit::Failed(message) if message.contains("EOF")));
        let out: Vec<f32> = std::iter::from_fn(|| consumer.try_pop()).collect();
        assert_eq!(out, vec![1.0, -1.0]);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn exit_before_header_reports_startup_failure() {
        let (producer, _consumer) = HeapRb::<f32>::new(16).split();
        let (ready_tx, ready_rx) = sync_channel(1);
        let exit = read_pcm_stream(
            std::io::Cursor::new(Vec::<u8>::new()),
            1,
            48_000,
            producer,
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicBool::new(true)),
            ready_tx,
        );

        let startup = ready_rx.recv().unwrap().unwrap_err();
        assert!(startup.contains("could not read ELTP stream header"));
        assert!(matches!(exit, ReaderExit::StartupFailed(message) if message == startup));
    }

    #[test]
    fn start_rejects_helper_exit_before_header() {
        let helper = TempHelper::script("exit 7");
        let running = Arc::new(AtomicBool::new(true));
        let reports = Arc::new(AtomicUsize::new(0));
        let result = start_test_helper(&helper, running.clone(), reports.clone());

        let err = match result {
            Ok(_) => panic!("helper without an ELTP header unexpectedly became ready"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("failed to start"));
        assert!(!running.load(Ordering::SeqCst));
        assert_eq!(reports.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn header_deadline_kills_and_reaps_a_live_helper_that_never_writes() {
        let helper = TempHelper::script("exec /bin/sleep 30");
        let running = Arc::new(AtomicBool::new(true));
        let reports = Arc::new(AtomicUsize::new(0));
        let started = Instant::now();
        let result = start_test_helper_with_deadline(
            &helper,
            running.clone(),
            reports.clone(),
            Duration::from_millis(120),
        );

        let err = match result {
            Ok(_) => panic!("helper without an ELTP header unexpectedly became ready"),
            Err(err) => err,
        };
        let message = err.to_string();
        assert!(
            message.contains("header deadline exceeded after 120 ms"),
            "{err:#}"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!running.load(Ordering::SeqCst));
        assert_eq!(reports.load(Ordering::SeqCst), 0);

        let pid = message
            .rsplit_once("(pid ")
            .and_then(|(_, pid)| pid.strip_suffix(')'))
            .expect("deadline error should identify the direct helper pid");
        let still_alive = Command::new("/bin/kill")
            .args(["-0", pid])
            .status()
            .unwrap()
            .success();
        assert!(!still_alive, "timed-out helper {pid:?} was not reaped");
    }

    #[test]
    fn unexpected_eof_after_header_stops_run_and_reports_once() {
        let helper = TempHelper::with_header("exit 0");
        let running = Arc::new(AtomicBool::new(true));
        let reports = Arc::new(AtomicUsize::new(0));
        let result = start_test_helper(&helper, running.clone(), reports.clone());

        if let Ok(stream) = result {
            for _ in 0..50 {
                if !running.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            drop(stream);
        }
        assert!(!running.load(Ordering::SeqCst));
        assert_eq!(reports.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropping_ready_stream_does_not_report_disconnect() {
        let helper = TempHelper::with_header("exec /bin/sleep 30");
        let running = Arc::new(AtomicBool::new(true));
        let reports = Arc::new(AtomicUsize::new(0));
        let stream = start_test_helper(&helper, running.clone(), reports.clone()).unwrap();

        drop(stream);

        assert!(!running.load(Ordering::SeqCst));
        assert_eq!(reports.load(Ordering::SeqCst), 0);
    }

    struct HeaderThenError {
        header: std::io::Cursor<Vec<u8>>,
    }

    impl Read for HeaderThenError {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.header.position() < self.header.get_ref().len() as u64 {
                return self.header.read(buf);
            }
            Err(std::io::Error::other("injected pipe failure"))
        }
    }

    #[test]
    fn read_error_after_header_is_a_runtime_failure() {
        let (producer, _consumer) = HeapRb::<f32>::new(16).split();
        let (ready_tx, ready_rx) = sync_channel(1);
        let exit = read_pcm_stream(
            HeaderThenError {
                header: std::io::Cursor::new(stream_header(48_000, 1)),
            },
            1,
            48_000,
            producer,
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicBool::new(true)),
            ready_tx,
        );

        assert_eq!(ready_rx.recv().unwrap(), Ok(()));
        assert!(
            matches!(exit, ReaderExit::Failed(message) if message.contains("injected pipe failure"))
        );
    }

    #[test]
    fn unexpected_failure_stops_once_but_deliberate_stop_does_not_report() {
        let running = AtomicBool::new(true);
        let first =
            claim_reader_failure(ReaderExit::Failed("unexpected EOF".to_string()), &running);
        assert_eq!(first.as_deref(), Some("unexpected EOF"));
        assert!(!running.load(Ordering::SeqCst));

        let duplicate =
            claim_reader_failure(ReaderExit::Failed("duplicate EOF".to_string()), &running);
        assert_eq!(duplicate, None);

        let deliberate = AtomicBool::new(false);
        let ignored =
            claim_reader_failure(ReaderExit::Failed("shutdown EOF".to_string()), &deliberate);
        assert_eq!(ignored, None);
    }

    #[test]
    fn stream_header_rejects_invalid_contract_values() {
        let mut invalid_magic = stream_header(48_000, 1);
        invalid_magic[0] = b'X';
        assert!(read_stream_header(&mut std::io::Cursor::new(invalid_magic)).is_err());

        let mut invalid_version = stream_header(48_000, 1);
        invalid_version[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert!(read_stream_header(&mut std::io::Cursor::new(invalid_version)).is_err());
        assert!(read_stream_header(&mut std::io::Cursor::new(stream_header(0, 1))).is_err());
        assert!(read_stream_header(&mut std::io::Cursor::new(stream_header(48_000, 0))).is_err());
    }

    #[test]
    fn remap_interleaved_upmixes_mono_to_stereo_by_duplication() {
        let mut out = Vec::new();
        remap_interleaved(&[0.1, -0.2], 1, 2, &mut out);
        assert_eq!(out, vec![0.1, 0.1, -0.2, -0.2]);
    }
}
