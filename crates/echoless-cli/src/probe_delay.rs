use std::env;
use std::fs::{create_dir_all, remove_dir_all};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use anyhow::{bail, Context, Result};
use clap::Args;
use echoless_core::default_near_delay_ms;
use serde_json::json;

use crate::dsp::rms_dbfs;

#[derive(Args)]
pub(crate) struct ProbeDelayArgs {
    /// Near-end microphone device selector
    #[arg(long, default_value = "MacBook Pro麦克风")]
    mic: String,
    /// Reference selector; macOS defaults to system (Process Tap), Windows defaults to system (WASAPI loopback)
    #[arg(long, default_value = "system")]
    reference: String,
    /// Echoless output device; a virtual audio device is recommended to avoid routing processed voice back to speakers
    #[arg(long, default_value = "BlackHole 2ch")]
    output: String,
    /// Directory to keep the diagnostics session in; when unset, a temporary directory is used and cleaned up after analysis
    #[arg(long)]
    out_dir: Option<PathBuf>,
    /// Keep this diagnostics session even when --out-dir is not specified
    #[arg(long)]
    keep_session: bool,
    /// Seconds to wait for the realtime pipeline to stabilize before starting the beep train
    #[arg(long, default_value_t = 4.0)]
    startup_delay: f64,
    /// Number of beeps
    #[arg(long, default_value_t = 12)]
    beeps: u32,
    /// Beep volume (0.0-1.0)
    #[arg(long, default_value_t = 0.35)]
    volume: f32,
    /// Only analyze an existing diagnostics session
    #[arg(long)]
    analyze_only: Option<PathBuf>,
    /// Keep the generated beep WAV
    #[arg(long)]
    keep_beep: Option<PathBuf>,
    /// Emit machine-readable JSON
    #[arg(long)]
    json: bool,
}

pub(crate) fn cmd_probe_delay(a: ProbeDelayArgs) -> Result<()> {
    if !cfg!(feature = "realtime") {
        bail!("probe-delay requires the realtime feature (cpal)");
    }
    if !(cfg!(target_os = "macos") || cfg!(windows) || cfg!(target_os = "linux")) {
        bail!("probe-delay currently supports macOS / Windows / Linux only");
    }
    if !a.startup_delay.is_finite() || a.startup_delay < 0.0 {
        bail!("--startup-delay must be a non-negative finite number");
    }
    if !a.volume.is_finite() || !(0.0..=1.0).contains(&a.volume) {
        bail!("--volume must be within 0.0..=1.0");
    }
    if a.beeps == 0 {
        bail!("--beeps must be greater than 0");
    }
    let cancel = Arc::new(AtomicBool::new(false));
    ctrlc::set_handler({
        let cancel = Arc::clone(&cancel);
        move || cancel.store(true, Ordering::SeqCst)
    })
    .context("failed to install probe-delay cancel handler")?;

    let (result, cleanup_dirs, session_retained) = if let Some(session_dir) = &a.analyze_only {
        (analyze_probe_session(&a, session_dir)?, Vec::new(), true)
    } else {
        let (beep_path, temp_dir) = probe_beep_path(&a)?;
        let (probe_out_dir, probe_temp_dir, retain_session) = probe_output_dir(&a)?;
        let beep_duration = write_probe_beep_train(&a, &beep_path)?;
        probe_log(a.json, format!("beep_duration_s: {beep_duration:.2}"));
        let session_dir =
            run_native_delay_probe(&a, &probe_out_dir, &beep_path, beep_duration, &cancel)?;
        let result = analyze_probe_session(&a, &session_dir)?;
        let mut cleanup_dirs = Vec::new();
        if let Some(temp_dir) = temp_dir {
            cleanup_dirs.push(temp_dir);
        }
        if !retain_session {
            if let Some(probe_temp_dir) = probe_temp_dir {
                cleanup_dirs.push(probe_temp_dir);
            }
        }
        (result, cleanup_dirs, retain_session)
    };

    emit_probe_result(&result, a.json, session_retained)?;
    for dir in cleanup_dirs {
        let _ = remove_dir_all(dir);
    }
    if let Some(warning) = result.warnings.first() {
        bail!("near delay probe warning: {warning}");
    }
    Ok(())
}

const PROBE_SAMPLE_RATE: u32 = 48_000;
const PROBE_PRE_ROLL_S: f64 = 0.5;
const PROBE_POST_ROLL_S: f64 = 0.8;
const PROBE_BEEP_MS: f64 = 70.0;
const PROBE_GAP_MS: f64 = 650.0;
const PROBE_MAX_LAG_MS: f64 = 250.0;
const PROBE_SAFETY_MS: f64 = 8.0;
const PROBE_ENV_STEP_MS: f64 = 0.5;

#[derive(Clone, Debug)]
struct ProbeLag {
    index: usize,
    time_s: f64,
    lag_ms: f64,
    corr: f64,
}

#[derive(Clone, Debug)]
struct ProbeResult {
    session_dir: PathBuf,
    ref_dbfs: f64,
    mic_dbfs: f64,
    global_lag_ms: f64,
    global_corr: f64,
    event_count: usize,
    event_detected: usize,
    event_lag_mean_ms: f64,
    event_lag_stddev_ms: f64,
    event_lag_drift_ms: f64,
    recommended_near_delay_ms: u32,
    per_beep_lags: Vec<ProbeLag>,
    warnings: Vec<String>,
}

fn probe_beep_path(a: &ProbeDelayArgs) -> Result<(PathBuf, Option<PathBuf>)> {
    if let Some(path) = &a.keep_beep {
        return Ok((path.clone(), None));
    }
    let dir = env::temp_dir().join(format!(
        "echoless-beep-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system time is before UNIX_EPOCH")?
            .as_nanos()
    ));
    create_dir_all(&dir)
        .with_context(|| format!("failed to create probe temp directory: {}", dir.display()))?;
    Ok((dir.join("near-delay-beeps.wav"), Some(dir)))
}

fn probe_output_dir(a: &ProbeDelayArgs) -> Result<(PathBuf, Option<PathBuf>, bool)> {
    if let Some(out_dir) = &a.out_dir {
        return Ok((out_dir.clone(), None, true));
    }
    let dir = env::temp_dir().join(format!(
        "echoless-near-delay-probe-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system time is before UNIX_EPOCH")?
            .as_nanos()
    ));
    Ok((dir.clone(), Some(dir), a.keep_session))
}

fn write_probe_beep_train(a: &ProbeDelayArgs, path: &Path) -> Result<f64> {
    let beep_frames = frames_for_ms(PROBE_BEEP_MS).max(1);
    let gap_frames = frames_for_ms(PROBE_GAP_MS).max(1);
    let pre_frames = (PROBE_SAMPLE_RATE as f64 * PROBE_PRE_ROLL_S).round() as usize;
    let post_frames = (PROBE_SAMPLE_RATE as f64 * PROBE_POST_ROLL_S).round() as usize;
    let ramp_frames = frames_for_ms(4.0).max(1);
    let freqs = [880.0, 1320.0, 1760.0, 1100.0];

    if let Some(parent) = path.parent() {
        create_dir_all(parent).with_context(|| {
            format!("failed to create beep WAV directory: {}", parent.display())
        })?;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: PROBE_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("failed to create {}", path.display()))?;

    let mut total_frames = 0usize;
    for _ in 0..pre_frames {
        writer.write_sample(0i16)?;
    }
    total_frames += pre_frames;

    for i in 0..a.beeps as usize {
        let freq = freqs[i % freqs.len()];
        for n in 0..beep_frames {
            let ramp = if n < ramp_frames {
                n as f64 / ramp_frames as f64
            } else if n >= beep_frames.saturating_sub(ramp_frames) {
                (beep_frames - n - 1) as f64 / ramp_frames as f64
            } else {
                1.0
            };
            let sample = f64::from(a.volume)
                * ramp
                * (2.0 * std::f64::consts::PI * freq * n as f64 / PROBE_SAMPLE_RATE as f64).sin();
            writer.write_sample(f32_to_i16(sample as f32))?;
        }
        for _ in 0..gap_frames {
            writer.write_sample(0i16)?;
        }
        total_frames += beep_frames + gap_frames;
    }
    for _ in 0..post_frames {
        writer.write_sample(0i16)?;
    }
    total_frames += post_frames;
    writer.finalize()?;
    Ok(total_frames as f64 / PROBE_SAMPLE_RATE as f64)
}

fn frames_for_ms(ms: f64) -> usize {
    (PROBE_SAMPLE_RATE as f64 * ms / 1000.0).round() as usize
}

fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

fn run_native_delay_probe(
    a: &ProbeDelayArgs,
    out_dir: &Path,
    beep_path: &Path,
    beep_duration_s: f64,
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    create_dir_all(out_dir).with_context(|| {
        format!(
            "failed to create diagnostics output directory: {}",
            out_dir.display()
        )
    })?;
    let diagnostic_seconds = (a.startup_delay + beep_duration_s + 1.0).ceil().max(1.0) as u32;
    let current_exe = env::current_exe().context("failed to locate current echoless executable")?;
    let mut command = Command::new(current_exe);
    command
        .arg("run")
        .arg("--processor")
        .arg("passthrough")
        .arg("--mic")
        .arg(&a.mic)
        .arg("--reference")
        .arg(&a.reference)
        .arg("--output")
        .arg(&a.output)
        .arg("--sample-rate")
        .arg(PROBE_SAMPLE_RATE.to_string())
        .arg("--frame-ms")
        .arg("10")
        .arg("--reference-channels")
        .arg("mono")
        .arg("--near-delay-ms")
        .arg("0")
        .arg("--diagnostic-seconds")
        .arg(diagnostic_seconds.to_string())
        .arg("--verbose")
        .arg("--stats-interval-ms")
        .arg("1000")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        // control reader 无条件启动(审计 B-01):stdin EOF = 优雅停机。probe spawn 的 run
        // 子进程若继承已关闭的 stdin,会一起来就读到 EOF 自杀退出(exit 0),导致 probe 报
        // 「run 过早退出」。给它一个常开 piped stdin(父持有、不写不关),让 run 正常跑满
        // diagnostic 录制时长;结束时 stop_probe_child 用 SIGINT 停它。
        .stdin(Stdio::piped());
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .context("failed to spawn echoless run probe child process")?;

    let stdout = child
        .stdout
        .take()
        .context("probe child stdout not captured")?;
    // 持有 run 的 stdin write end:run 期间保持打开(control reader 阻塞等待、不 EOF,run 正常录);
    // beep 播完后关闭它 → run 收 stdin EOF 优雅停机(比进程组 SIGINT 可靠,dev/打包都稳)。
    let mut run_stdin = child.stdin.take();
    let rx = spawn_probe_line_reader(stdout);
    let mut session_dir: Option<PathBuf> = None;
    let mut saw_done = false;

    let startup_deadline = Instant::now() + Duration::from_secs_f64(a.startup_delay);
    while Instant::now() < startup_deadline {
        if cancel.load(Ordering::SeqCst) {
            stop_probe_child(&mut child)?;
            bail!("probe-delay cancelled");
        }
        drain_probe_output(&rx, a.json, &mut session_dir, &mut saw_done);
        if let Some(status) = child.try_wait()? {
            bail!("echoless run probe exited prematurely: {status}");
        }
        thread::sleep(Duration::from_millis(50));
    }

    probe_log(
        a.json,
        format!("playing beep train: {}", beep_path.display()),
    );
    // GUI 同步点:蜂鸣列车即将开播。stdout 只留最终 JSON,进度走 stderr JSONL,
    // 前端据此把 12 点进度灯对齐到真实播放时刻(此前按墙钟猜,音画不同步)。
    if a.json {
        eprintln!(
            "{}",
            serde_json::json!({
                "type": "probe_progress",
                "stage": "beep_train_start",
                "pre_roll_ms": PROBE_PRE_ROLL_S * 1000.0,
                "beep_ms": PROBE_BEEP_MS,
                "gap_ms": PROBE_GAP_MS,
                "beeps": a.beeps,
            })
        );
    }
    play_probe_beep(a, beep_path)?;

    // beep 播完 = run 已录够诊断。关闭 stdin 触发 run 优雅停机(flush diagnostic、输出
    // 「诊断录制完成」后退出),不再靠 finish loop 空等 deadline + SIGINT —— dev sidecar 下
    // 进程组 SIGINT 停不干净,run 会常驻 R 态、把 probe 拖到逼近 45s 超时(前端一直 PROBING)。
    drop(run_stdin.take());

    let finish_deadline = Instant::now() + Duration::from_secs(u64::from(diagnostic_seconds) + 2);
    while Instant::now() < finish_deadline {
        if cancel.load(Ordering::SeqCst) {
            stop_probe_child(&mut child)?;
            bail!("probe-delay cancelled");
        }
        drain_probe_output(&rx, a.json, &mut session_dir, &mut saw_done);
        if saw_done {
            break;
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!("echoless run probe failed: {status}");
            }
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    stop_probe_child(&mut child)?;
    drain_probe_output(&rx, a.json, &mut session_dir, &mut saw_done);

    session_dir
        .filter(|path| path.is_dir())
        .or_else(|| newest_probe_session(out_dir).ok())
        .with_context(|| format!("diagnostics session not found: {}", out_dir.display()))
}

#[cfg(all(feature = "realtime", target_os = "macos"))]
fn play_probe_beep(_a: &ProbeDelayArgs, beep_path: &Path) -> Result<()> {
    let status = Command::new("afplay")
        .arg(beep_path)
        .status()
        .context("failed to play beep WAV (requires macOS afplay)")?;
    if !status.success() {
        bail!("failed to play beep WAV: {status}");
    }
    Ok(())
}

#[cfg(all(feature = "realtime", windows))]
fn play_probe_beep(a: &ProbeDelayArgs, beep_path: &Path) -> Result<()> {
    let selector = reference_playback_output_selector(&a.reference)?;
    let samples = read_probe_beep_samples(beep_path)?;
    crate::realtime::play_mono_samples_to_output(selector, PROBE_SAMPLE_RATE, samples)
}

// Linux:参考是 PipeWire/Pulse 的 monitor source,把它映射回被监听的 sink 再播蜂鸣。
// 命名约定核实自 pipewire 源码(module-protocol-pulse/pulse-server.c):
//   节点名 = "<sink_name>.monitor"(:767,官方也用该后缀判定 :2124);
//   描述名 = "Monitor of <sink_desc>"(:3998)。
// 映射失败时回退默认输出(常见场景 monitor 的就是默认 sink)——beep 只要从被
// 监听的 sink 出声,分析就成立;彻底播错设备时 probe 会以 ref silent 告警收场。
#[cfg(all(feature = "realtime", target_os = "linux"))]
fn play_probe_beep(a: &ProbeDelayArgs, beep_path: &Path) -> Result<()> {
    let samples = read_probe_beep_samples(beep_path)?;
    if let Some(stem) = monitor_reference_output_stem(&a.reference)? {
        match crate::realtime::play_mono_samples_to_output(
            Some(&stem),
            PROBE_SAMPLE_RATE,
            samples.clone(),
        ) {
            Ok(()) => return Ok(()),
            Err(err) => {
                eprintln!("probe-delay: monitor->sink mapping playback failed ({stem}): {err}; falling back to default output")
            }
        }
    }
    crate::realtime::play_mono_samples_to_output(None, PROBE_SAMPLE_RATE, samples)
}

/// monitor 参考名 → 被监听 sink 的名字片段(cpal 输出设备 selector)。
/// None = 用默认输出。纯字符串逻辑,单测在所有平台跑。
#[cfg_attr(not(all(feature = "realtime", target_os = "linux")), allow(dead_code))]
fn monitor_reference_output_stem(reference: &str) -> Result<Option<String>> {
    let reference = reference.trim();
    match reference {
        "" | "default" | "system" => Ok(None),
        "none" => bail!("probe-delay requires a playable reference; current reference=none"),
        value => {
            let name = value.strip_prefix("input:").unwrap_or(value).trim();
            let stem = name
                .strip_prefix("Monitor of ")
                .unwrap_or(name)
                .trim_end_matches(".monitor")
                .trim();
            Ok((!stem.is_empty()).then(|| stem.to_string()))
        }
    }
}

#[cfg(all(
    feature = "realtime",
    not(any(target_os = "macos", windows, target_os = "linux"))
))]
fn play_probe_beep(_a: &ProbeDelayArgs, _beep_path: &Path) -> Result<()> {
    bail!("no probe-delay beep playback implementation for the current platform");
}

#[cfg(not(feature = "realtime"))]
fn play_probe_beep(_a: &ProbeDelayArgs, _beep_path: &Path) -> Result<()> {
    bail!("probe-delay beep playback requires the realtime feature (cpal)");
}

#[cfg(any(windows, test))]
fn reference_playback_output_selector(reference: &str) -> Result<Option<&str>> {
    let reference = reference.trim();
    match reference {
        "" | "system" | "default" => Ok(None),
        "none" => bail!("probe-delay requires a playable reference; current reference=none"),
        value if value.starts_with("input:") => {
            bail!("probe-delay cannot play beeps into an input reference: {value}")
        }
        value => {
            if let Some(output) = value.strip_prefix("output:") {
                return Ok((!output.trim().is_empty()).then_some(output));
            }
            Ok(Some(value))
        }
    }
}

#[cfg(any(windows, target_os = "linux"))]
fn read_probe_beep_samples(beep_path: &Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(beep_path)
        .with_context(|| format!("failed to read beep WAV: {}", beep_path.display()))?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.sample_rate != PROBE_SAMPLE_RATE
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        bail!(
            "unsupported beep WAV format: channels={}, rate={}, bits={}, format={:?}",
            spec.channels,
            spec.sample_rate,
            spec.bits_per_sample,
            spec.sample_format
        );
    }
    reader
        .samples::<i16>()
        .map(|s| {
            s.map(|v| f32::from(v) / f32::from(i16::MAX))
                .context("failed to read beep WAV sample")
        })
        .collect()
}

fn spawn_probe_line_reader<R>(reader: R) -> Receiver<String>
where
    R: Read + Send + 'static,
{
    let (sender, receiver) = channel();
    thread::spawn(move || {
        // 显式处理读取错误(ROB-4):不要用 lines().flatten()/map_while(Result::ok)
        // 静默吞掉 IO 错误/非 UTF-8 行——出错时打印一条 stderr 警告再停止读取,
        // 以免 probe 子进程输出异常时无声丢失。
        for line in BufReader::new(reader).lines() {
            match line {
                Ok(line) => {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    eprintln!("probe-delay: failed to read child output, stopping: {err}");
                    break;
                }
            }
        }
    });
    receiver
}

fn drain_probe_output(
    rx: &Receiver<String>,
    json_mode: bool,
    session_dir: &mut Option<PathBuf>,
    saw_done: &mut bool,
) {
    while let Ok(line) = rx.try_recv() {
        if !json_mode {
            println!("{line}");
        }
        if let Some((_, dir)) = line.split_once("diagnostics recording directory:") {
            *session_dir = Some(PathBuf::from(dir.trim()));
        }
        if let Some((_, dir)) = line.split_once("diagnostics recording complete") {
            if let Some((_, path)) = dir.rsplit_once(": ") {
                *session_dir = Some(PathBuf::from(path.trim()));
            }
            *saw_done = true;
        }
    }
}

fn stop_probe_child(child: &mut std::process::Child) -> Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    let _ = Command::new("kill")
        .arg("-INT")
        .arg(probe_child_signal_target(child))
        .status();

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(probe_child_signal_target(child))
            .status();
    }
    #[cfg(not(unix))]
    child.kill().context("failed to stop probe child process")?;
    let _ = child.wait();
    Ok(())
}

fn probe_child_signal_target(child: &std::process::Child) -> String {
    #[cfg(unix)]
    {
        format!("-{}", child.id())
    }
    #[cfg(not(unix))]
    {
        child.id().to_string()
    }
}

fn newest_probe_session(out_dir: &Path) -> Result<PathBuf> {
    let mut sessions = std::fs::read_dir(out_dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let is_session = path
                .file_name()
                .map(|name| name.to_string_lossy().starts_with("session-"))
                .unwrap_or(false);
            if !path.is_dir() || !is_session {
                return None;
            }
            entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|modified| (modified, path))
        })
        .collect::<Vec<_>>();
    sessions.sort_by_key(|(modified, _)| *modified);
    sessions
        .pop()
        .map(|(_, path)| path)
        .with_context(|| format!("no session-* found under {}", out_dir.display()))
}

fn analyze_probe_session(a: &ProbeDelayArgs, session_dir: &Path) -> Result<ProbeResult> {
    let ref_path = session_dir.join("ref.wav");
    let mic_path = session_dir.join("mic.wav");
    let (ref_rate, reference) = read_wav_mono(&ref_path)?;
    let (mic_rate, mic) = read_wav_mono(&mic_path)?;
    if ref_rate != PROBE_SAMPLE_RATE || mic_rate != PROBE_SAMPLE_RATE {
        bail!(
            "probe only supports 48k diagnostics; got ref={} mic={}",
            ref_rate,
            mic_rate
        );
    }

    let step_frames = frames_for_ms(PROBE_ENV_STEP_MS).max(1);
    let ref_env = envelope(&reference, step_frames);
    let mic_env = envelope(&mic, step_frames);
    let (global_lag_ms, global_corr) =
        estimate_probe_lag(&ref_env, &mic_env, PROBE_ENV_STEP_MS, PROBE_MAX_LAG_MS);
    let events = find_ref_events(&ref_env, PROBE_ENV_STEP_MS, a.beeps as usize);
    let event_lags = per_event_lags(
        &ref_env,
        &mic_env,
        &events,
        PROBE_ENV_STEP_MS,
        PROBE_MAX_LAG_MS,
    );
    let valid_lags = event_lags
        .iter()
        .filter(|(_, _, corr)| corr.abs() > 0.15)
        .map(|(_, lag, _)| *lag)
        .collect::<Vec<_>>();
    let (mean, stddev, drift) = if valid_lags.is_empty() {
        (global_lag_ms, 0.0, 0.0)
    } else {
        let mean = valid_lags.iter().sum::<f64>() / valid_lags.len() as f64;
        let variance =
            valid_lags.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / valid_lags.len() as f64;
        let drift = valid_lags.last().unwrap() - valid_lags.first().unwrap();
        (mean, variance.sqrt(), drift)
    };
    let ref_dbfs = rms_dbfs(&reference);
    let mic_dbfs = rms_dbfs(&mic);
    let mut warnings = Vec::new();
    if ref_dbfs < -45.0 {
        warnings.push("ref is very quiet; play the beep through the system output".to_string());
    }

    Ok(ProbeResult {
        session_dir: session_dir.to_path_buf(),
        ref_dbfs,
        mic_dbfs,
        global_lag_ms,
        global_corr,
        event_count: valid_lags.len(),
        event_detected: events.len(),
        event_lag_mean_ms: mean,
        event_lag_stddev_ms: stddev,
        event_lag_drift_ms: drift,
        recommended_near_delay_ms: recommended_near_delay_ms(mean, PROBE_SAFETY_MS),
        per_beep_lags: event_lags
            .into_iter()
            .enumerate()
            .map(|(index, (event_index, lag_ms, corr))| ProbeLag {
                index: index + 1,
                time_s: event_index as f64 * PROBE_ENV_STEP_MS / 1000.0,
                lag_ms,
                corr,
            })
            .collect(),
        warnings,
    })
}

fn read_wav_mono(path: &Path) -> Result<(u32, Vec<f32>)> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("failed to read WAV: {}", path.display()))?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels.max(1));
    let values = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int if spec.bits_per_sample <= 16 => reader
            .samples::<i16>()
            .map(|sample| sample.map(|v| v as f32 / i16::MAX as f32))
            .collect::<std::result::Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int if spec.bits_per_sample <= 32 => {
            let scale = (1_i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|sample| sample.map(|v| v as f32 / scale))
                .collect::<std::result::Result<Vec<_>, _>>()?
        }
        _ => bail!(
            "{}: unsupported WAV format: {:?} {}bit",
            path.display(),
            spec.sample_format,
            spec.bits_per_sample
        ),
    };
    if channels == 1 {
        return Ok((spec.sample_rate, values));
    }
    let mono = values
        .chunks(channels)
        .filter(|frame| frame.len() == channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect();
    Ok((spec.sample_rate, mono))
}

fn envelope(samples: &[f32], step_frames: usize) -> Vec<f64> {
    samples
        .chunks(step_frames.max(1))
        .map(|chunk| {
            let energy = chunk
                .iter()
                .map(|sample| f64::from(*sample) * f64::from(*sample))
                .sum::<f64>();
            (energy / chunk.len().max(1) as f64).sqrt()
        })
        .collect()
}

fn standardize(values: &[f64]) -> Vec<f64> {
    if values.is_empty() {
        return Vec::new();
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let centered = values.iter().map(|v| v - mean).collect::<Vec<_>>();
    let energy = centered.iter().map(|v| v * v).sum::<f64>().sqrt().max(1.0);
    centered.into_iter().map(|v| v / energy).collect()
}

fn estimate_probe_lag(reference: &[f64], mic: &[f64], step_ms: f64, max_lag_ms: f64) -> (f64, f64) {
    let n = reference.len().min(mic.len());
    let reference = standardize(&reference[..n]);
    let mic = standardize(&mic[..n]);
    let max_lag = (max_lag_ms / step_ms).round().max(1.0) as isize;
    let mut best_lag = 0isize;
    let mut best_corr = 0.0f64;

    for lag in -max_lag..=max_lag {
        let (ref_start, mic_start, len) = if lag >= 0 {
            let lag = lag as usize;
            (0usize, lag, n.saturating_sub(lag))
        } else {
            let lag = (-lag) as usize;
            (lag, 0usize, n.saturating_sub(lag))
        };
        if len < 10 {
            continue;
        }
        let corr = (0..len)
            .map(|i| reference[ref_start + i] * mic[mic_start + i])
            .sum::<f64>();
        if corr.abs() > best_corr.abs() {
            best_corr = corr;
            best_lag = lag;
        }
    }
    (best_lag as f64 * step_ms, best_corr)
}

fn find_ref_events(reference: &[f64], step_ms: f64, expected: usize) -> Vec<usize> {
    if reference.is_empty() || expected == 0 {
        return Vec::new();
    }
    let mut sorted = reference.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];
    let peak = reference.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let threshold = median + (peak - median) * 0.35;
    let min_gap = (180.0 / step_ms).round().max(1.0) as usize;
    let mut events = Vec::new();
    let mut i = 0usize;
    while i < reference.len() {
        if reference[i] < threshold {
            i += 1;
            continue;
        }
        let mut best_i = i;
        let mut best_v = reference[i];
        while i < reference.len() && reference[i] >= threshold {
            if reference[i] > best_v {
                best_i = i;
                best_v = reference[i];
            }
            i += 1;
        }
        if events
            .last()
            .is_none_or(|last| best_i.saturating_sub(*last) >= min_gap)
        {
            events.push(best_i);
        } else if reference[best_i] > reference[*events.last().unwrap()] {
            *events.last_mut().unwrap() = best_i;
        }
        if events.len() >= expected {
            break;
        }
    }
    events
}

fn per_event_lags(
    reference: &[f64],
    mic: &[f64],
    events: &[usize],
    step_ms: f64,
    max_lag_ms: f64,
) -> Vec<(usize, f64, f64)> {
    let half_window = (160.0 / step_ms).round().max(20.0) as usize;
    events
        .iter()
        .map(|event| {
            let start = event.saturating_sub(half_window);
            let end = reference.len().min(event + half_window);
            let (lag_ms, corr) = estimate_probe_lag(
                &reference[start..end],
                &mic[start..end],
                step_ms,
                max_lag_ms,
            );
            (*event, lag_ms, corr)
        })
        .collect()
}

fn recommended_near_delay_ms(lag_ms: f64, safety_ms: f64) -> u32 {
    let default_bias_ms = default_near_delay_ms() as f64;
    let recommended_ms = (-lag_ms + safety_ms).max(default_bias_ms);
    (recommended_ms / 5.0).round().max(0.0) as u32 * 5
}

fn emit_probe_result(result: &ProbeResult, json_mode: bool, session_retained: bool) -> Result<()> {
    if json_mode {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "session_dir": result.session_dir.display().to_string(),
                "session_retained": session_retained,
                "ref_dbfs": round2(result.ref_dbfs),
                "mic_dbfs": round2(result.mic_dbfs),
                "global_lag_ms": round2(result.global_lag_ms),
                "global_corr": round4(result.global_corr),
                "event_count": result.event_count,
                "event_detected": result.event_detected,
                "event_lag_mean_ms": round2(result.event_lag_mean_ms),
                "event_lag_stddev_ms": round2(result.event_lag_stddev_ms),
                "event_lag_drift_ms": round2(result.event_lag_drift_ms),
                "recommended_near_delay_ms": result.recommended_near_delay_ms,
                "per_beep_lags": result.per_beep_lags.iter().map(|lag| json!({
                    "index": lag.index,
                    "time_s": round3(lag.time_s),
                    "lag_ms": round2(lag.lag_ms),
                    "corr": round4(lag.corr),
                })).collect::<Vec<_>>(),
                "warnings": result.warnings,
            }))?
        );
        return Ok(());
    }

    println!("\n=== near delay probe result ===");
    println!("session_dir: {}", result.session_dir.display());
    println!("session_retained: {}", session_retained);
    println!("ref_dbfs: {:.1}", result.ref_dbfs);
    println!("mic_dbfs: {:.1}", result.mic_dbfs);
    println!(
        "global_lag_ms: {:+.2}  corr={:+.3}",
        result.global_lag_ms, result.global_corr
    );
    println!(
        "event_count: {}/{}",
        result.event_count, result.event_detected
    );
    println!("event_lag_mean_ms: {:+.2}", result.event_lag_mean_ms);
    println!("event_lag_stddev_ms: {:.2}", result.event_lag_stddev_ms);
    println!("event_lag_drift_ms: {:+.2}", result.event_lag_drift_ms);
    println!(
        "recommended_near_delay_ms: {}",
        result.recommended_near_delay_ms
    );
    println!("\nper-beep lags:");
    for lag in &result.per_beep_lags {
        println!(
            "  {:02}  t={:6.2}s  lag={:+7.2}ms  corr={:+.3}",
            lag.index, lag.time_s, lag.lag_ms, lag.corr
        );
    }
    for warning in &result.warnings {
        println!("\nwarning: {warning}");
    }
    Ok(())
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn round4(value: f64) -> f64 {
    (value * 10000.0).round() / 10000.0
}

fn probe_log(json_mode: bool, message: impl AsRef<str>) {
    if !json_mode {
        println!("{}", message.as_ref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_delay_args() -> ProbeDelayArgs {
        ProbeDelayArgs {
            mic: "MacBook Pro麦克风".to_string(),
            reference: "system".to_string(),
            output: "BlackHole 2ch".to_string(),
            out_dir: None,
            keep_session: false,
            startup_delay: 4.0,
            beeps: 12,
            volume: 0.35,
            analyze_only: None,
            keep_beep: None,
            json: true,
        }
    }

    #[test]
    fn probe_recommendation_preserves_default_bias() {
        // 大负 lag(-18.5 → -(-18.5)+8=26.5,round 到 5 = 25)超过任何平台默认 → 按实测。
        assert_eq!(recommended_near_delay_ms(-18.5, 8.0), 25);
        // 小负 lag / 正 lag → 回落平台默认:mac 25(负方向窗)、win/linux 0(不设近端延迟)。
        if cfg!(target_os = "macos") {
            assert_eq!(recommended_near_delay_ms(-2.0, 8.0), 25);
            assert_eq!(recommended_near_delay_ms(12.0, 8.0), 25);
        } else {
            assert_eq!(recommended_near_delay_ms(-2.0, 8.0), 10); // max(10, 0)=10
            assert_eq!(recommended_near_delay_ms(12.0, 8.0), 0); // max(-4, 0)=0
        }
    }

    #[test]
    fn probe_output_dir_is_temporary_by_default() {
        let args = probe_delay_args();
        let (out_dir, temp_dir, retained) = probe_output_dir(&args).unwrap();
        assert!(!retained);
        assert_eq!(temp_dir.as_ref(), Some(&out_dir));
        assert!(out_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("echoless-near-delay-probe-"));

        let mut keep_args = probe_delay_args();
        keep_args.keep_session = true;
        let (out_dir, temp_dir, retained) = probe_output_dir(&keep_args).unwrap();
        assert!(retained);
        assert_eq!(temp_dir.as_ref(), Some(&out_dir));

        let mut explicit_args = probe_delay_args();
        explicit_args.out_dir = Some(PathBuf::from("/tmp/echoless-probe-explicit"));
        let (out_dir, temp_dir, retained) = probe_output_dir(&explicit_args).unwrap();
        assert!(retained);
        assert!(temp_dir.is_none());
        assert_eq!(out_dir, PathBuf::from("/tmp/echoless-probe-explicit"));
    }

    #[test]
    fn reference_playback_selector_targets_output_sources() {
        assert_eq!(reference_playback_output_selector("system").unwrap(), None);
        assert_eq!(reference_playback_output_selector("default").unwrap(), None);
        assert_eq!(
            reference_playback_output_selector("output:wasapi-device-id").unwrap(),
            Some("wasapi-device-id")
        );
        assert_eq!(
            reference_playback_output_selector("plain-output-selector").unwrap(),
            Some("plain-output-selector")
        );
        assert!(reference_playback_output_selector("none").is_err());
        assert!(reference_playback_output_selector("input:mic-id").is_err());
    }

    #[test]
    fn monitor_reference_maps_back_to_sink_stem() {
        // PipeWire/Pulse 两种命名(pulse-server.c :767 / :3998)都要能剥回 sink。
        assert_eq!(
            monitor_reference_output_stem("input:alsa_output.pci-0000.analog-stereo.monitor")
                .unwrap()
                .as_deref(),
            Some("alsa_output.pci-0000.analog-stereo")
        );
        assert_eq!(
            monitor_reference_output_stem("input:Monitor of Built-in Audio Analog Stereo")
                .unwrap()
                .as_deref(),
            Some("Built-in Audio Analog Stereo")
        );
        // 无前后缀的普通名字原样透传;default/system/空串 → 默认输出。
        assert_eq!(
            monitor_reference_output_stem("some-sink")
                .unwrap()
                .as_deref(),
            Some("some-sink")
        );
        assert_eq!(monitor_reference_output_stem("default").unwrap(), None);
        assert_eq!(monitor_reference_output_stem("system").unwrap(), None);
        assert_eq!(monitor_reference_output_stem("").unwrap(), None);
        assert!(monitor_reference_output_stem("none").is_err());
    }

    #[test]
    fn native_probe_lag_estimator_detects_mic_leading_reference() {
        let mut reference = vec![0.0; 1_100];
        let mut mic = vec![0.0; 1_100];
        for index in [100usize, 500, 900] {
            reference[index] = 1.0;
            mic[index - 4] = 1.0;
        }

        let events = find_ref_events(&reference, 0.5, 3);
        let event_lags = per_event_lags(&reference, &mic, &events, 0.5, 20.0);
        let (global_lag_ms, corr) = estimate_probe_lag(&reference, &mic, 0.5, 20.0);

        assert_eq!(events, vec![100, 500, 900]);
        assert_eq!(global_lag_ms, -2.0);
        assert!(corr > 0.95);
        assert!(event_lags
            .iter()
            .all(|(_, lag_ms, corr)| *lag_ms == -2.0 && *corr > 0.95));
    }
}
