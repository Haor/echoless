use std::env;
use std::fs::{create_dir_all, remove_dir_all};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::Args;
use serde_json::json;

#[derive(Args)]
pub(crate) struct ProbeDelayArgs {
    /// 近端麦克风设备 selector
    #[arg(long, default_value = "MacBook Pro麦克风")]
    mic: String,
    /// reference selector;macOS 默认 system(Process Tap),Windows 默认 system(WASAPI loopback)
    #[arg(long, default_value = "system")]
    reference: String,
    /// Echoless 输出设备;建议虚拟音频设备,避免把处理后人声送回外放
    #[arg(long, default_value = "BlackHole 2ch")]
    output: String,
    /// 保留 diagnostics session 的输出目录;不传则使用临时目录并在分析后清理
    #[arg(long)]
    out_dir: Option<PathBuf>,
    /// 即使未指定 --out-dir,也保留本次 diagnostics session
    #[arg(long)]
    keep_session: bool,
    /// 开始播放蜂鸣前等待实时管线稳定的秒数
    #[arg(long, default_value_t = 4.0)]
    startup_delay: f64,
    /// 蜂鸣个数
    #[arg(long, default_value_t = 12)]
    beeps: u32,
    /// 蜂鸣音量(0.0-1.0)
    #[arg(long, default_value_t = 0.35)]
    volume: f32,
    /// 仅分析已有 diagnostics session
    #[arg(long)]
    analyze_only: Option<PathBuf>,
    /// 保留生成的蜂鸣 WAV
    #[arg(long)]
    keep_beep: Option<PathBuf>,
    /// 输出机器可读 JSON
    #[arg(long)]
    json: bool,
}

pub(crate) fn cmd_probe_delay(a: ProbeDelayArgs) -> Result<()> {
    if !cfg!(feature = "realtime") {
        bail!("probe-delay 需 realtime 特性(cpal)");
    }
    if !(cfg!(target_os = "macos") || cfg!(windows)) {
        bail!("probe-delay 当前只支持 macOS Process Tap reference 与 Windows WASAPI loopback reference");
    }
    if !a.startup_delay.is_finite() || a.startup_delay < 0.0 {
        bail!("--startup-delay 必须是非负有限数");
    }
    if !a.volume.is_finite() || !(0.0..=1.0).contains(&a.volume) {
        bail!("--volume 必须在 0.0..=1.0");
    }
    if a.beeps == 0 {
        bail!("--beeps 必须大于 0");
    }

    let (result, cleanup_dirs, session_retained) = if let Some(session_dir) = &a.analyze_only {
        (analyze_probe_session(&a, session_dir)?, Vec::new(), true)
    } else {
        let (beep_path, temp_dir) = probe_beep_path(&a)?;
        let (probe_out_dir, probe_temp_dir, retain_session) = probe_output_dir(&a)?;
        let beep_duration = write_probe_beep_train(&a, &beep_path)?;
        probe_log(a.json, format!("beep_duration_s: {beep_duration:.2}"));
        let session_dir = run_native_delay_probe(&a, &probe_out_dir, &beep_path, beep_duration)?;
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
            .context("系统时间早于 UNIX_EPOCH")?
            .as_nanos()
    ));
    create_dir_all(&dir).with_context(|| format!("创建 probe 临时目录失败: {}", dir.display()))?;
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
            .context("系统时间早于 UNIX_EPOCH")?
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
        create_dir_all(parent)
            .with_context(|| format!("创建蜂鸣 WAV 目录失败: {}", parent.display()))?;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: PROBE_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("创建 {} 失败", path.display()))?;

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
) -> Result<PathBuf> {
    create_dir_all(out_dir)
        .with_context(|| format!("创建 diagnostics 输出目录失败: {}", out_dir.display()))?;
    let diagnostic_seconds = (a.startup_delay + beep_duration_s + 1.0).ceil().max(1.0) as u32;
    let current_exe = env::current_exe().context("定位当前 echoless 可执行文件失败")?;
    let mut child = Command::new(current_exe)
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
        .arg("--diagnostic-dir")
        .arg(out_dir)
        .arg("--diagnostic-seconds")
        .arg(diagnostic_seconds.to_string())
        .arg("--verbose")
        .arg("--stats-interval-ms")
        .arg("1000")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("启动 echoless run probe 子进程失败")?;

    let stdout = child.stdout.take().context("probe 子进程 stdout 未捕获")?;
    let rx = spawn_probe_line_reader(stdout);
    let mut session_dir: Option<PathBuf> = None;
    let mut saw_done = false;

    let startup_deadline = Instant::now() + Duration::from_secs_f64(a.startup_delay);
    while Instant::now() < startup_deadline {
        drain_probe_output(&rx, a.json, &mut session_dir, &mut saw_done);
        if let Some(status) = child.try_wait()? {
            bail!("echoless run probe 过早退出: {status}");
        }
        thread::sleep(Duration::from_millis(50));
    }

    probe_log(
        a.json,
        format!("playing beep train: {}", beep_path.display()),
    );
    let play_status = play_probe_beep(beep_path)?;
    if !play_status.success() {
        bail!("播放蜂鸣 WAV 失败: {play_status}");
    }

    let finish_deadline = Instant::now() + Duration::from_secs(u64::from(diagnostic_seconds) + 2);
    while Instant::now() < finish_deadline {
        drain_probe_output(&rx, a.json, &mut session_dir, &mut saw_done);
        if saw_done {
            break;
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!("echoless run probe 失败: {status}");
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
        .with_context(|| format!("未找到 diagnostics session: {}", out_dir.display()))
}

fn play_probe_beep(beep_path: &Path) -> Result<std::process::ExitStatus> {
    if cfg!(target_os = "macos") {
        return Command::new("afplay")
            .arg(beep_path)
            .status()
            .context("播放蜂鸣 WAV 失败(需要 macOS afplay)");
    }
    if cfg!(windows) {
        return Command::new("powershell.exe")
            .arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-Command")
            .arg("$player = New-Object System.Media.SoundPlayer $args[0]; $player.Load(); $player.PlaySync()")
            .arg(beep_path)
            .status()
            .context("播放蜂鸣 WAV 失败(需要 Windows PowerShell SoundPlayer)");
    }
    bail!("当前平台没有 probe-delay 蜂鸣播放实现");
}

fn spawn_probe_line_reader<R>(reader: R) -> Receiver<String>
where
    R: Read + Send + 'static,
{
    let (sender, receiver) = channel();
    thread::spawn(move || {
        for line in BufReader::new(reader)
            .lines()
            .map_while(std::result::Result::ok)
        {
            if sender.send(line).is_err() {
                break;
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
        if let Some((_, dir)) = line.split_once("诊断录制目录:") {
            *session_dir = Some(PathBuf::from(dir.trim()));
        }
        if let Some((_, dir)) = line.split_once("诊断录制完成") {
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
        .arg(child.id().to_string())
        .status();

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    child.kill().context("停止 probe 子进程失败")?;
    let _ = child.wait();
    Ok(())
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
        .with_context(|| format!("{} 下没有 session-*", out_dir.display()))
}

fn analyze_probe_session(a: &ProbeDelayArgs, session_dir: &Path) -> Result<ProbeResult> {
    let ref_path = session_dir.join("ref.wav");
    let mic_path = session_dir.join("mic.wav");
    let (ref_rate, reference) = read_wav_mono(&ref_path)?;
    let (mic_rate, mic) = read_wav_mono(&mic_path)?;
    if ref_rate != PROBE_SAMPLE_RATE || mic_rate != PROBE_SAMPLE_RATE {
        bail!(
            "probe 只支持 48k diagnostics,实际 ref={} mic={}",
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
    let ref_dbfs = rms_db(&reference);
    let mic_dbfs = rms_db(&mic);
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
        .with_context(|| format!("读取 WAV 失败: {}", path.display()))?;
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
            "{} 不支持的 WAV 格式: {:?} {}bit",
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

fn rms_db(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return -120.0;
    }
    let energy = samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum::<f64>();
    20.0 * ((energy / samples.len() as f64).sqrt() + 1e-12).log10()
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
    if lag_ms >= 0.0 {
        return 0;
    }
    ((-lag_ms + safety_ms) / 5.0).round().max(0.0) as u32 * 5
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
    fn probe_recommendation_uses_only_mic_lead() {
        assert_eq!(recommended_near_delay_ms(-18.5, 8.0), 25);
        assert_eq!(recommended_near_delay_ms(-2.0, 8.0), 10);
        assert_eq!(recommended_near_delay_ms(12.0, 8.0), 0);
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
