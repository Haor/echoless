//! cpal 实时管线。结构移植自上游 sonora-aec(BSD-3),处理换成 echoless 的 `ProcessorChain`。
//!
//! 三股 cpal 流 + 三个 ringbuf + 一个独立处理线程:
//! ```text
//! mic 设备 ──► mic_ring ──┐
//!                         ├─► 处理线程(每 10ms)─ chain.process(near, far) ─► out_ring ──► 输出设备
//! 系统 loopback ─► render_ring┘
//! ```
//! 设备 I/O 可用原生采样率打开,在设备边界固定比率重采样到 pipeline rate;
//! ProcessorChain 仍负责处理器节点边界重采样(如 LocalVQE 16k)。
//! 跨平台靠 cpal:Windows WASAPI(含 output loopback)/ macOS CoreAudio。
//! 系统声音参考 = output 设备做 loopback(Windows 原生;macOS 需 BlackHole 之类)。
//! 虚拟音频输出 = 选 VB-Cable / BlackHole 作 output 设备。

#[cfg(target_os = "macos")]
mod macos_process_tap;

use std::collections::VecDeque;
use std::fs::{create_dir, create_dir_all, rename, File};
use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, DeviceDescription, FromSample, Sample, SampleFormat, SizedSample, Stream,
    SupportedStreamConfig, SupportedStreamConfigRange,
};
use hound::{WavSpec, WavWriter};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;
use serde_json::{json, Value};

use echoless_core::{
    apply_output_level, apply_reference_channels_to_chain, output_level_gain_db, DiagnosticsConfig,
    PipelineConfig, ReferenceChannels, MAX_NEAR_DELAY_MS, MAX_OUTPUT_LEVEL,
};
use echoless_processors::{chain_from_nodes, ProcessorChain, ProcessorStats};

#[derive(Clone, Copy)]
enum DeviceKind {
    Input,
    Output,
}
impl DeviceKind {
    fn label(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputChannelMode {
    MonoDownmix,
    PreserveFirst(usize),
}

impl InputChannelMode {
    fn from_reference_channels(mode: ReferenceChannels) -> Self {
        match mode {
            ReferenceChannels::Mono => Self::MonoDownmix,
            ReferenceChannels::Stereo => Self::PreserveFirst(2),
        }
    }

    fn output_channels(self) -> usize {
        match self {
            Self::MonoDownmix => 1,
            Self::PreserveFirst(channels) => channels.max(1),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RuntimeOptions {
    /// None = quiet;Some(ms) = 每隔 ms 打印一行滚动状态。
    pub stats_interval_ms: Option<u64>,
    /// true = stdout 只输出 JSONL status;人类提示改走 stderr。
    pub status_json: bool,
}

struct SelectedDevice {
    index: Option<usize>,
    device: Device,
}

struct StreamConfigChoice {
    supported: SupportedStreamConfig,
    pipeline_sample_rate: u32,
}

impl StreamConfigChoice {
    fn new(supported: SupportedStreamConfig, pipeline_sample_rate: u32) -> Self {
        Self {
            supported,
            pipeline_sample_rate,
        }
    }

    fn stream_sample_rate(&self) -> u32 {
        self.supported.sample_rate()
    }

    fn requires_resampling(&self) -> bool {
        self.stream_sample_rate() != self.pipeline_sample_rate
    }

    fn channels(&self) -> u16 {
        self.supported.channels()
    }

    fn sample_format(&self) -> SampleFormat {
        self.supported.sample_format()
    }

    fn config(&self) -> cpal::StreamConfig {
        self.supported.config()
    }
}

enum ReferenceSource {
    None,
    Cpal {
        device: SelectedDevice,
        kind: DeviceKind,
    },
    #[cfg(target_os = "macos")]
    ProcessTap,
}

impl ReferenceSource {
    fn has_reference(&self) -> bool {
        !matches!(self, Self::None)
    }

    fn status_name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Cpal { kind, .. } => match kind {
                DeviceKind::Input => "cpal_input",
                DeviceKind::Output => "cpal_output",
            },
            #[cfg(target_os = "macos")]
            Self::ProcessTap => "macos_process_tap",
        }
    }
}

#[derive(Clone)]
struct RealtimeCounters {
    mic_input_drops: Arc<AtomicU64>,
    ref_input_drops: Arc<AtomicU64>,
    output_underruns: Arc<AtomicU64>,
}

impl RealtimeCounters {
    fn new() -> Self {
        Self {
            mic_input_drops: Arc::new(AtomicU64::new(0)),
            ref_input_drops: Arc::new(AtomicU64::new(0)),
            output_underruns: Arc::new(AtomicU64::new(0)),
        }
    }
}

struct ProcessRuntime {
    sample_rate: u32,
    frame_ms: u32,
    frame_size: usize,
    reference_channels: usize,
    near_delay_ms: u32,
    near_delay_samples: usize,
    output_level: u32,
    backend: String,
    algorithmic_latency_ms: f32,
    counters: RealtimeCounters,
    stats_interval: Option<Duration>,
    status_json: bool,
    diagnostics_session_dir: Option<String>,
    diagnostics_status: Option<DiagnosticsStatusHandle>,
    diagnostic: Option<DiagnosticRecorder>,
    control: Option<Receiver<RuntimeControlEvent>>,
}

#[derive(Debug)]
enum RuntimeControlCommand {
    StartDiagnostics {
        record_dir: String,
        max_seconds: Option<u32>,
    },
    StopDiagnostics,
    SetOutputLevel(u32),
}

#[derive(Debug)]
enum RuntimeControlEvent {
    Command(RuntimeControlCommand),
    Error(String),
}

pub fn run_with_options(cfg: &PipelineConfig, options: RuntimeOptions) -> Result<()> {
    let host = cpal::default_host();

    let mic_device = select_device(&host, DeviceKind::Input, mic_selector(&cfg.mic))
        .context("选择麦克风设备失败")?;
    let output_device = select_device(&host, DeviceKind::Output, output_selector(&cfg.output))
        .context("选择输出设备失败")?;
    // reference:"none" = 无参考(纯 NS);"system" = 平台原生系统音频参考。
    let reference_source =
        select_reference_source(&host, cfg.reference.as_str()).context("选择参考源失败")?;

    let sample_rate = cfg.sample_rate;
    if cfg.frame_ms == 0 {
        bail!("帧长必须大于 0ms");
    }
    let frame_samples = sample_rate as u64 * cfg.frame_ms as u64;
    if !frame_samples.is_multiple_of(1000) {
        bail!(
            "采样率与帧长必须产生整数样本: sample_rate={sample_rate}, frame_ms={}",
            cfg.frame_ms
        );
    }
    let frame_size = (frame_samples / 1000) as usize;
    if cfg.near_delay_ms > MAX_NEAR_DELAY_MS {
        bail!("near_delay_ms 必须 <= {MAX_NEAR_DELAY_MS}");
    }
    if cfg.output_level > MAX_OUTPUT_LEVEL {
        bail!("output_level 必须 <= {MAX_OUTPUT_LEVEL}");
    }
    let near_delay_samples = delay_ms_to_samples(cfg.near_delay_ms, sample_rate);
    let ring_size = frame_size * 12 + near_delay_samples; // ~120ms plus explicit near delay
    if is_macos_process_tap(&reference_source) && sample_rate != macos_process_tap_sample_rate() {
        bail!(
            "macOS Process Tap 当前仅支持 {} Hz,当前 sample_rate={sample_rate}",
            macos_process_tap_sample_rate()
        );
    }

    let reference_channels = if reference_source.has_reference() {
        usize::from(cfg.reference_channels.channel_count())
    } else {
        1
    };

    let mic_config = pick_config(&mic_device.device, DeviceKind::Input, sample_rate)
        .context("麦克风不支持该采样率")?;
    let output_config = pick_config(&output_device.device, DeviceKind::Output, sample_rate)
        .context("输出设备不支持该采样率")?;
    let render_config = match &reference_source {
        ReferenceSource::Cpal { device, kind } => Some(
            pick_config(&device.device, *kind, sample_rate).context("参考设备不支持该采样率")?,
        ),
        ReferenceSource::None => None,
        #[cfg(target_os = "macos")]
        ReferenceSource::ProcessTap => None,
    };
    if cfg.reference_channels == ReferenceChannels::Stereo
        && render_config.as_ref().is_some_and(|c| c.channels() < 2)
    {
        bail!("reference_channels=stereo 需要参考设备至少 2ch");
    }

    print_human(
        options.status_json,
        format!(
            "Mic:    {} ({})",
            selected_device_label(&mic_device),
            config_choice_summary(&mic_config)
        ),
    );
    match (&reference_source, &render_config) {
        (ReferenceSource::Cpal { device, kind }, Some(c)) => print_human(
            options.status_json,
            format!(
                "Ref:    {} {} ({})",
                kind.label(),
                selected_device_label(device),
                config_choice_summary(c)
            ),
        ),
        #[cfg(target_os = "macos")]
        (ReferenceSource::ProcessTap, _) => print_human(
            options.status_json,
            format!(
                "Ref:    macOS Process Tap system audio ({} Hz, {}ch)",
                macos_process_tap::SAMPLE_RATE,
                reference_channels
            ),
        ),
        (ReferenceSource::None, _) => print_human(
            options.status_json,
            "Ref:    无 —— AEC 缺少参考,仅 NS 等单端处理有效",
        ),
        _ => {}
    }
    print_human(
        options.status_json,
        format!(
            "Output: {} ({})",
            selected_device_label(&output_device),
            config_choice_summary(&output_config)
        ),
    );

    let mut chain_cfg = cfg.chain.clone();
    apply_reference_channels_to_chain(&mut chain_cfg, cfg.reference_channels);
    let backend = if chain_cfg.is_empty() {
        "passthrough".to_string()
    } else {
        chain_cfg
            .iter()
            .map(|n| n.kind.clone())
            .collect::<Vec<_>>()
            .join("+")
    };
    let chain_desc = if chain_cfg.is_empty() {
        "直通".to_string()
    } else {
        chain_cfg
            .iter()
            .map(|n| n.kind.clone())
            .collect::<Vec<_>>()
            .join(" → ")
    };
    print_human(
        options.status_json,
        format!(
            "采样率 {sample_rate} Hz · 帧 {} ms / {frame_size} 样本 · near_delay={} ms · output_level={} · reference={} · 链: {chain_desc}",
            cfg.frame_ms,
            cfg.near_delay_ms,
            cfg.output_level,
            cfg.reference_channels.as_str()
        ),
    );

    let running = Arc::new(AtomicBool::new(true));
    ctrlc::set_handler({
        let running = running.clone();
        move || running.store(false, Ordering::SeqCst)
    })?;

    let counters = RealtimeCounters::new();

    let (mic_prod, mic_cons) = HeapRb::<f32>::new(ring_size).split();
    let (out_prod, out_cons) = HeapRb::<f32>::new(ring_size).split();
    let (render_prod, render_cons) = if reference_source.has_reference() {
        let (p, c) = HeapRb::<f32>::new(ring_size * reference_channels).split();
        (Some(p), Some(c))
    } else {
        (None, None)
    };

    let mic_stream = build_input_stream(
        &mic_device.device,
        &mic_config,
        mic_prod,
        "mic",
        InputChannelMode::MonoDownmix,
        counters.mic_input_drops.clone(),
    )?;
    let output_stream = build_output_stream(
        &output_device.device,
        &output_config,
        out_cons,
        counters.output_underruns.clone(),
    )?;
    let mut render_prod = render_prod;
    let render_stream = match (&reference_source, render_config.as_ref()) {
        (ReferenceSource::Cpal { device, .. }, Some(c)) => {
            let p = render_prod.take().context("参考 ring producer 未初始化")?;
            Some(build_input_stream(
                &device.device,
                c,
                p,
                "ref",
                InputChannelMode::from_reference_channels(cfg.reference_channels),
                counters.ref_input_drops.clone(),
            )?)
        }
        _ => None,
    };
    #[cfg(target_os = "macos")]
    let process_tap_stream = match &reference_source {
        ReferenceSource::ProcessTap => {
            let p = render_prod
                .take()
                .context("Process Tap ring producer 未初始化")?;
            Some(macos_process_tap::start(
                cfg.reference_channels,
                p,
                counters.ref_input_drops.clone(),
                running.clone(),
            )?)
        }
        _ => None,
    };

    // 处理线程:只碰 ring(Send),cpal Stream 留在本线程(!Send)。
    let proc_running = running.clone();
    let chain = chain_from_nodes(&chain_cfg, sample_rate, reference_channels as u16)?;
    let algorithmic_latency_ms = chain.total_latency_ms();
    let initial_node_stats = chain.stats();
    let stats_interval = options.stats_interval_ms.map(Duration::from_millis);
    let diagnostic = DiagnosticRecorder::new(
        &cfg.diagnostics,
        sample_rate,
        reference_channels as u16,
        cfg.frame_ms,
        cfg.near_delay_ms,
        cfg.output_level,
        &initial_node_stats,
        options.status_json,
    )?;
    let diagnostics_session_dir = diagnostic
        .as_ref()
        .map(|recorder| recorder.dir.display().to_string());
    let diagnostics_status = diagnostic.as_ref().map(DiagnosticRecorder::status_handle);
    let control = options.status_json.then(spawn_control_reader);
    let started_event = json!({
        "type": "started",
        "backend": backend.as_str(),
        "sample_rate": sample_rate,
        "frame_ms": cfg.frame_ms,
        "near_delay_ms": cfg.near_delay_ms,
        "near_delay_samples": near_delay_samples,
        "output_level": cfg.output_level,
        "output_gain_db": output_level_gain_db(cfg.output_level),
        "reference_channels": cfg.reference_channels.as_str(),
        "algorithmic_latency_ms": algorithmic_latency_ms,
        "reference_source": reference_source.status_name(),
        "diagnostics_session_dir": diagnostics_session_dir.as_deref(),
        "mic_device_sample_rate": mic_config.stream_sample_rate(),
        "output_device_sample_rate": output_config.stream_sample_rate(),
        "reference_device_sample_rate": render_config.as_ref().map(StreamConfigChoice::stream_sample_rate),
        "io_resampling": {
            "mic": mic_config.requires_resampling(),
            "reference": render_config.as_ref().is_some_and(StreamConfigChoice::requires_resampling),
            "output": output_config.requires_resampling(),
        },
    });
    let runtime = ProcessRuntime {
        sample_rate,
        frame_ms: cfg.frame_ms,
        frame_size,
        reference_channels,
        near_delay_ms: cfg.near_delay_ms,
        near_delay_samples,
        output_level: cfg.output_level,
        backend,
        algorithmic_latency_ms,
        counters,
        stats_interval,
        status_json: options.status_json,
        diagnostics_session_dir,
        diagnostics_status,
        diagnostic,
        control,
    };
    let proc = thread::spawn(move || {
        process_loop(
            proc_running,
            chain,
            mic_cons,
            render_cons,
            out_prod,
            runtime,
        );
    });

    mic_stream.play()?;
    if let Some(s) = &render_stream {
        s.play()?;
    }
    output_stream.play()?;
    if options.status_json {
        println!("{}", serde_json::to_string(&started_event)?);
    }

    print_human(
        options.status_json,
        "运行中。macOS 首次需授予麦克风/系统音频录制权限。Ctrl+C 停止。",
    );
    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
    }

    drop(mic_stream);
    drop(render_stream);
    #[cfg(target_os = "macos")]
    drop(process_tap_stream);
    drop(output_stream);
    proc.join().ok();
    print_human(options.status_json, "已停止。");
    Ok(())
}

fn print_human(status_json: bool, message: impl AsRef<str>) {
    if status_json {
        eprintln!("{}", message.as_ref());
    } else {
        println!("{}", message.as_ref());
    }
}

fn spawn_control_reader() -> Receiver<RuntimeControlEvent> {
    let (sender, receiver) = channel();
    thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let event = match line {
                Ok(line) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match parse_runtime_control_command(trimmed) {
                        Ok(command) => RuntimeControlEvent::Command(command),
                        Err(err) => RuntimeControlEvent::Error(format!(
                            "invalid runtime control JSON: {err}; line={trimmed}"
                        )),
                    }
                }
                Err(err) => {
                    RuntimeControlEvent::Error(format!("runtime control stdin read failed: {err}"))
                }
            };
            if sender.send(event).is_err() {
                break;
            }
        }
    });
    receiver
}

fn parse_runtime_control_command(line: &str) -> Result<RuntimeControlCommand> {
    let value: Value = serde_json::from_str(line)?;
    let cmd = value
        .get("cmd")
        .and_then(Value::as_str)
        .context("missing string field `cmd`")?;
    match cmd {
        "start_diagnostics" => {
            let record_dir = value
                .get("record_dir")
                .and_then(Value::as_str)
                .context("start_diagnostics requires string field `record_dir`")?
                .to_string();
            let max_seconds =
                match value.get("max_seconds") {
                    None | Some(Value::Null) => None,
                    Some(v) => {
                        let seconds = v.as_u64().context(
                            "start_diagnostics `max_seconds` must be a positive integer",
                        )?;
                        Some(u32::try_from(seconds).context(
                            "start_diagnostics `max_seconds` is too large for this backend",
                        )?)
                    }
                };
            Ok(RuntimeControlCommand::StartDiagnostics {
                record_dir,
                max_seconds,
            })
        }
        "stop_diagnostics" => Ok(RuntimeControlCommand::StopDiagnostics),
        "set_output_level" => {
            let level = value
                .get("level")
                .and_then(Value::as_u64)
                .context("set_output_level requires integer field `level`")?;
            if level > u64::from(MAX_OUTPUT_LEVEL) {
                bail!("set_output_level `level` must be <= {MAX_OUTPUT_LEVEL}");
            }
            Ok(RuntimeControlCommand::SetOutputLevel(level as u32))
        }
        other => bail!("unknown runtime control command `{other}`"),
    }
}

fn process_loop<M, R, O>(
    running: Arc<AtomicBool>,
    mut chain: ProcessorChain,
    mut mic_cons: M,
    mut render_cons: Option<R>,
    mut out_prod: O,
    mut runtime: ProcessRuntime,
) where
    M: Consumer<Item = f32>,
    R: Consumer<Item = f32>,
    O: Producer<Item = f32>,
{
    let frame_size = runtime.frame_size;
    let far_samples_per_frame = frame_size * runtime.reference_channels;
    let mut mic_frame = vec![0.0f32; frame_size];
    let mut near = vec![0.0f32; frame_size];
    let mut far = vec![0.0f32; far_samples_per_frame];
    let mut out = vec![0.0f32; frame_size];
    let mut near_delay = VecDeque::from(vec![0.0f32; runtime.near_delay_samples]);
    let mut stats = runtime.stats_interval.map(|interval| {
        RealtimeStats::new(
            interval,
            runtime.sample_rate,
            runtime.frame_ms,
            runtime.near_delay_ms,
            runtime.output_level,
            runtime.backend.clone(),
            runtime.algorithmic_latency_ms,
            runtime.status_json,
            runtime.diagnostics_session_dir.clone(),
            runtime.diagnostics_status.clone(),
        )
    });
    let mut diagnostic = runtime.diagnostic;
    let mut control = runtime.control;

    while running.load(Ordering::SeqCst) {
        handle_runtime_controls(
            &mut control,
            &mut diagnostic,
            stats.as_mut(),
            &chain,
            runtime.sample_rate,
            runtime.reference_channels as u16,
            runtime.frame_ms,
            runtime.near_delay_ms,
            &mut runtime.output_level,
            runtime.status_json,
        );

        if mic_cons.occupied_len() < frame_size {
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        // 控制积压(简单 drift/堆积处理):超 4 帧丢旧的。
        let mut stale_drops = skip_stale(&mut mic_cons, frame_size);
        mic_cons.pop_slice(&mut mic_frame);
        let near_delay_buffered_samples = apply_near_delay(
            &mut near_delay,
            &mic_frame,
            &mut near,
            runtime.near_delay_samples,
        );

        let mut ref_underrun = 0;
        if let Some(rc) = render_cons.as_mut() {
            stale_drops += skip_stale(rc, far_samples_per_frame);
            if rc.occupied_len() >= far_samples_per_frame {
                rc.pop_slice(&mut far);
            } else {
                far.fill(0.0); // 参考欠载 → 填静音
                ref_underrun = 1;
            }
        } else {
            far.fill(0.0);
        }

        chain.process(&near, &far, &mut out, frame_size as u32);
        apply_output_level(&mut out, runtime.output_level);
        let node_stats = chain.stats();
        let pushed = out_prod.push_slice(&out);
        let output_overruns = out.len().saturating_sub(pushed) as u64;
        let ref_q = render_cons
            .as_ref()
            .map(|rc| rc.occupied_len())
            .unwrap_or(0);
        let sample = StatsSample {
            algorithmic_latency_ms: runtime.algorithmic_latency_ms,
            near_delay_ms: runtime.near_delay_ms,
            near_delay_buffered_samples,
            frame_size,
            near: &near,
            far: &far,
            out: &out,
            mic_q: mic_cons.occupied_len(),
            ref_q,
            out_q: out_prod.occupied_len(),
            mic_input_drops: runtime.counters.mic_input_drops.swap(0, Ordering::Relaxed),
            ref_input_drops: runtime.counters.ref_input_drops.swap(0, Ordering::Relaxed),
            stale_drops: stale_drops as u64,
            ref_underruns: ref_underrun,
            output_overruns,
            output_underruns: runtime.counters.output_underruns.swap(0, Ordering::Relaxed),
            node_stats: &node_stats,
        };
        if let Some(recorder) = diagnostic.as_mut() {
            match recorder.write_frame(&sample) {
                Ok(true) => {}
                Ok(false) => {}
                Err(err) => {
                    eprintln!("诊断录制失败,已停用: {err:#}");
                    diagnostic = None;
                }
            }
        }
        if let Some(stats) = stats.as_mut() {
            stats.observe(&sample);
        }
    }
}

fn handle_runtime_controls(
    control: &mut Option<Receiver<RuntimeControlEvent>>,
    diagnostic: &mut Option<DiagnosticRecorder>,
    stats: Option<&mut RealtimeStats>,
    chain: &ProcessorChain,
    sample_rate: u32,
    reference_channels: u16,
    frame_ms: u32,
    near_delay_ms: u32,
    output_level: &mut u32,
    status_json: bool,
) {
    let Some(receiver) = control.as_mut() else {
        return;
    };
    let mut stats = stats;
    loop {
        match receiver.try_recv() {
            Ok(RuntimeControlEvent::Command(command)) => handle_runtime_control_command(
                command,
                diagnostic,
                stats.as_mut().map(|stats| &mut **stats),
                chain,
                sample_rate,
                reference_channels,
                frame_ms,
                near_delay_ms,
                output_level,
                status_json,
            ),
            Ok(RuntimeControlEvent::Error(message)) => {
                emit_control_error(status_json, None, message);
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                *control = None;
                break;
            }
        }
    }
}

fn handle_runtime_control_command(
    command: RuntimeControlCommand,
    diagnostic: &mut Option<DiagnosticRecorder>,
    stats: Option<&mut RealtimeStats>,
    chain: &ProcessorChain,
    sample_rate: u32,
    reference_channels: u16,
    frame_ms: u32,
    near_delay_ms: u32,
    output_level: &mut u32,
    status_json: bool,
) {
    match command {
        RuntimeControlCommand::StartDiagnostics {
            record_dir,
            max_seconds,
        } => {
            if record_dir.trim().is_empty() {
                emit_control_error(
                    status_json,
                    Some("start_diagnostics"),
                    "record_dir must not be empty",
                );
                return;
            }
            if matches!(max_seconds, Some(0)) {
                emit_control_error(
                    status_json,
                    Some("start_diagnostics"),
                    "max_seconds must be greater than 0",
                );
                return;
            }
            if diagnostic
                .as_ref()
                .is_some_and(DiagnosticRecorder::is_recording)
            {
                emit_control_error(
                    status_json,
                    Some("start_diagnostics"),
                    "diagnostics is already recording",
                );
                return;
            }

            let _previous = diagnostic.take();
            let cfg = DiagnosticsConfig {
                record_dir: Some(record_dir),
                max_seconds,
            };
            let node_stats = chain.stats();
            match DiagnosticRecorder::new(
                &cfg,
                sample_rate,
                reference_channels,
                frame_ms,
                near_delay_ms,
                *output_level,
                &node_stats,
                status_json,
            ) {
                Ok(Some(recorder)) => {
                    let session_dir = recorder.session_dir_string();
                    let status = recorder.status_handle();
                    if let Some(stats) = stats {
                        stats.set_diagnostics(Some(session_dir.clone()), Some(status));
                    }
                    *diagnostic = Some(recorder);
                    emit_runtime_json(
                        status_json,
                        json!({
                            "type": "diagnostics_started",
                            "session_dir": session_dir,
                            "max_seconds": max_seconds,
                            "recording": true,
                        }),
                    );
                }
                Ok(None) => emit_control_error(
                    status_json,
                    Some("start_diagnostics"),
                    "record_dir did not create a diagnostics recorder",
                ),
                Err(err) => emit_control_error(
                    status_json,
                    Some("start_diagnostics"),
                    format!("failed to start diagnostics: {err:#}"),
                ),
            }
        }
        RuntimeControlCommand::StopDiagnostics => {
            let Some(recorder) = diagnostic.as_mut() else {
                emit_control_error(
                    status_json,
                    Some("stop_diagnostics"),
                    "diagnostics is not active",
                );
                return;
            };
            if !recorder.is_recording() {
                emit_control_error(
                    status_json,
                    Some("stop_diagnostics"),
                    "diagnostics is already stopping or stopped",
                );
                return;
            }
            let session_dir = recorder.session_dir_string();
            recorder.request_finish(DiagnosticDoneReason::Stopped);
            emit_runtime_json(
                status_json,
                json!({
                    "type": "diagnostics_stopping",
                    "session_dir": session_dir,
                }),
            );
        }
        RuntimeControlCommand::SetOutputLevel(level) => {
            *output_level = level;
            if let Some(stats) = stats {
                stats.set_output_level(level);
            }
            emit_runtime_json(
                status_json,
                json!({
                    "type": "output_level_changed",
                    "output_level": level,
                    "output_gain_db": output_level_gain_db(level),
                }),
            );
        }
    }
}

fn emit_control_error(
    status_json: bool,
    command: Option<&'static str>,
    message: impl Into<String>,
) {
    emit_runtime_json(
        status_json,
        json!({
            "type": "control_error",
            "cmd": command,
            "message": message.into(),
        }),
    );
}

fn emit_runtime_json(status_json: bool, value: Value) {
    if status_json {
        println!("{value}");
    } else {
        eprintln!("{value}");
    }
}

fn skip_stale<C: Consumer<Item = f32>>(consumer: &mut C, frame_size: usize) -> usize {
    let max_queued = frame_size * 4;
    let queued = consumer.occupied_len();
    if queued > max_queued {
        let dropped = queued - max_queued;
        consumer.skip(dropped);
        dropped
    } else {
        0
    }
}

fn delay_ms_to_samples(ms: u32, sample_rate: u32) -> usize {
    ((u64::from(ms) * u64::from(sample_rate) + 500) / 1000) as usize
}

fn apply_near_delay(
    delay: &mut VecDeque<f32>,
    input: &[f32],
    output: &mut [f32],
    delay_samples: usize,
) -> usize {
    if delay_samples == 0 {
        output.copy_from_slice(input);
        return 0;
    }

    delay.extend(input.iter().copied());
    for sample in output.iter_mut() {
        *sample = delay.pop_front().unwrap_or(0.0);
    }
    delay.len()
}

struct StatsSample<'a> {
    algorithmic_latency_ms: f32,
    near_delay_ms: u32,
    near_delay_buffered_samples: usize,
    frame_size: usize,
    near: &'a [f32],
    far: &'a [f32],
    out: &'a [f32],
    mic_q: usize,
    ref_q: usize,
    out_q: usize,
    mic_input_drops: u64,
    ref_input_drops: u64,
    stale_drops: u64,
    ref_underruns: u64,
    output_overruns: u64,
    output_underruns: u64,
    node_stats: &'a [ProcessorStats],
}

const DIAGNOSTIC_QUEUE_FRAMES: usize = 128;

#[derive(Clone)]
struct DiagnosticsStatusHandle {
    inner: Arc<DiagnosticsSharedState>,
    sample_rate: u32,
}

impl DiagnosticsStatusHandle {
    fn new(sample_rate: u32) -> Self {
        Self {
            inner: Arc::new(DiagnosticsSharedState {
                recording: AtomicBool::new(true),
                frames: AtomicU64::new(0),
                drops: AtomicU64::new(0),
            }),
            sample_rate,
        }
    }

    fn is_recording(&self) -> bool {
        self.inner.recording.load(Ordering::Relaxed)
    }

    fn set_recording(&self, recording: bool) {
        self.inner.recording.store(recording, Ordering::Relaxed);
    }

    fn frames(&self) -> u64 {
        self.inner.frames.load(Ordering::Relaxed)
    }

    fn set_frames(&self, frames: u64) {
        self.inner.frames.store(frames, Ordering::Relaxed);
    }

    fn drops(&self) -> u64 {
        self.inner.drops.load(Ordering::Relaxed)
    }

    fn increment_drops(&self) {
        self.inner.drops.fetch_add(1, Ordering::Relaxed);
    }

    fn elapsed_s(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames() as f64 / self.sample_rate as f64
        }
    }
}

struct DiagnosticsSharedState {
    recording: AtomicBool,
    frames: AtomicU64,
    drops: AtomicU64,
}

#[derive(Clone, Copy, Debug)]
enum DiagnosticDoneReason {
    MaxSeconds,
    Stopped,
    RunExit,
    Error,
}

impl DiagnosticDoneReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::MaxSeconds => "max_seconds",
            Self::Stopped => "stopped",
            Self::RunExit => "run_exit",
            Self::Error => "error",
        }
    }
}

enum DiagnosticCommand {
    Frame(DiagnosticFrame),
    Finish(DiagnosticDoneReason),
}

struct DiagnosticFrame {
    frame_size: usize,
    algorithmic_latency_ms: f32,
    near_delay_ms: u32,
    near_delay_buffered_samples: usize,
    near: Vec<f32>,
    far: Vec<f32>,
    out: Vec<f32>,
    mic_q: usize,
    ref_q: usize,
    out_q: usize,
    mic_input_drops: u64,
    ref_input_drops: u64,
    stale_drops: u64,
    ref_underruns: u64,
    output_overruns: u64,
    output_underruns: u64,
    node_process_time_ms: f32,
    node_runtime_errors: u64,
    aec_estimated_delay_ms: i32,
    node_diverged: bool,
    node_last_error: Option<String>,
}

impl DiagnosticFrame {
    fn from_sample(sample: &StatsSample<'_>) -> Self {
        Self {
            frame_size: sample.frame_size,
            algorithmic_latency_ms: sample.algorithmic_latency_ms,
            near_delay_ms: sample.near_delay_ms,
            near_delay_buffered_samples: sample.near_delay_buffered_samples,
            near: sample.near.to_vec(),
            far: sample.far.to_vec(),
            out: sample.out.to_vec(),
            mic_q: sample.mic_q,
            ref_q: sample.ref_q,
            out_q: sample.out_q,
            mic_input_drops: sample.mic_input_drops,
            ref_input_drops: sample.ref_input_drops,
            stale_drops: sample.stale_drops,
            ref_underruns: sample.ref_underruns,
            output_overruns: sample.output_overruns,
            output_underruns: sample.output_underruns,
            node_process_time_ms: aggregate_process_time_ms(sample.node_stats),
            node_runtime_errors: aggregate_runtime_errors(sample.node_stats),
            aec_estimated_delay_ms: aggregate_estimated_delay_ms(sample.node_stats),
            node_diverged: aggregate_diverged(sample.node_stats),
            node_last_error: aggregate_last_error(sample.node_stats),
        }
    }
}

struct DiagnosticRecorder {
    dir: PathBuf,
    sender: Option<SyncSender<DiagnosticCommand>>,
    writer: Option<JoinHandle<()>>,
    status: DiagnosticsStatusHandle,
}

impl DiagnosticRecorder {
    fn new(
        cfg: &DiagnosticsConfig,
        sample_rate: u32,
        reference_channels: u16,
        frame_ms: u32,
        near_delay_ms: u32,
        output_level: u32,
        node_stats: &[ProcessorStats],
        status_json: bool,
    ) -> Result<Option<Self>> {
        let Some(record_dir) = cfg
            .record_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Ok(None);
        };
        let base = Path::new(record_dir);
        create_dir_all(base)
            .with_context(|| format!("创建诊断录制目录失败: {}", base.display()))?;
        let dir = make_session_dir(base)?;
        let spec = WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let ref_spec = WavSpec {
            channels: reference_channels.max(1),
            ..spec
        };
        let max_frames = cfg
            .max_seconds
            .map(|seconds| u64::from(seconds) * u64::from(sample_rate));

        write_diagnostic_metadata(
            &dir,
            sample_rate,
            frame_ms,
            near_delay_ms,
            output_level,
            reference_channels,
            max_frames,
            node_stats,
        )?;
        let stats_part_path = dir.join("stats.csv.part");
        let mut stats = BufWriter::new(
            File::create(&stats_part_path)
                .with_context(|| format!("创建诊断 stats.csv 失败: {}", dir.display()))?,
        );
        writeln!(
            stats,
            "frame_index,frames,near_delay_ms,near_delay_buffered_samples,mic_dbfs,ref_dbfs,out_dbfs,mic_q,ref_q,out_q,output_queue_latency_ms,estimated_user_latency_ms,aec_estimated_delay_ms,mic_input_drops,ref_input_drops,input_drops,stale_drops,ref_underruns,output_overruns,output_underruns,node_process_time_ms,node_runtime_errors,node_diverged,node_last_error"
        )?;

        let mic_part_path = dir.join("mic.wav.part");
        let ref_part_path = dir.join("ref.wav.part");
        let out_part_path = dir.join("out.wav.part");
        let status = DiagnosticsStatusHandle::new(sample_rate);
        let writer = DiagnosticWriter {
            dir: dir.clone(),
            mic: Some(WavWriter::create(&mic_part_path, spec)?),
            reference: Some(WavWriter::create(&ref_part_path, ref_spec)?),
            out: Some(WavWriter::create(&out_part_path, spec)?),
            stats: Some(stats),
            mic_part_path,
            ref_part_path,
            out_part_path,
            stats_part_path,
            mic_path: dir.join("mic.wav"),
            ref_path: dir.join("ref.wav"),
            out_path: dir.join("out.wav"),
            stats_path: dir.join("stats.csv"),
            sample_rate,
            frame_ms,
            max_frames,
            written_frames: 0,
            frame_index: 0,
            human_to_stderr: status_json,
            status_json,
            status: status.clone(),
        };
        let (sender, receiver) = sync_channel(DIAGNOSTIC_QUEUE_FRAMES);
        let writer = thread::spawn(move || writer.run(receiver));

        print_human(status_json, format!("诊断录制目录: {}", dir.display()));
        Ok(Some(Self {
            dir,
            sender: Some(sender),
            writer: Some(writer),
            status,
        }))
    }

    fn status_handle(&self) -> DiagnosticsStatusHandle {
        self.status.clone()
    }

    fn is_recording(&self) -> bool {
        self.status.is_recording()
    }

    fn session_dir_string(&self) -> String {
        self.dir.display().to_string()
    }

    fn write_frame(&mut self, sample: &StatsSample<'_>) -> Result<bool> {
        if !self.status.is_recording() {
            return Ok(false);
        }
        let Some(sender) = self.sender.as_ref() else {
            return Ok(false);
        };
        match sender.try_send(DiagnosticCommand::Frame(DiagnosticFrame::from_sample(
            sample,
        ))) {
            Ok(()) => Ok(true),
            Err(TrySendError::Full(_)) => {
                self.status.increment_drops();
                Ok(true)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.status.set_recording(false);
                bail!("诊断 writer 线程已退出")
            }
        }
    }

    fn request_finish(&mut self, reason: DiagnosticDoneReason) {
        self.status.set_recording(false);
        if let Some(sender) = self.sender.take() {
            thread::spawn(move || {
                let _ = sender.send(DiagnosticCommand::Finish(reason));
            });
        }
        let _ = self.writer.take();
    }

    fn finish(&mut self, reason: DiagnosticDoneReason) {
        if let Some(sender) = self.sender.take() {
            match sender.try_send(DiagnosticCommand::Finish(reason)) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => {}
                Err(TrySendError::Full(_)) => {}
            }
        }
        if let Some(writer) = self.writer.take() {
            if let Err(err) = writer.join() {
                eprintln!("诊断 writer 线程退出异常: {err:?}");
            }
        }
    }
}

impl Drop for DiagnosticRecorder {
    fn drop(&mut self) {
        self.finish(DiagnosticDoneReason::RunExit);
    }
}

struct DiagnosticWriter {
    dir: PathBuf,
    mic: Option<WavWriter<BufWriter<File>>>,
    reference: Option<WavWriter<BufWriter<File>>>,
    out: Option<WavWriter<BufWriter<File>>>,
    stats: Option<BufWriter<File>>,
    mic_part_path: PathBuf,
    ref_part_path: PathBuf,
    out_part_path: PathBuf,
    stats_part_path: PathBuf,
    mic_path: PathBuf,
    ref_path: PathBuf,
    out_path: PathBuf,
    stats_path: PathBuf,
    sample_rate: u32,
    frame_ms: u32,
    max_frames: Option<u64>,
    written_frames: u64,
    frame_index: u64,
    human_to_stderr: bool,
    status_json: bool,
    status: DiagnosticsStatusHandle,
}

impl DiagnosticWriter {
    fn run(mut self, receiver: Receiver<DiagnosticCommand>) {
        let mut reason = DiagnosticDoneReason::RunExit;
        let mut ok = true;
        while let Ok(command) = receiver.recv() {
            match command {
                DiagnosticCommand::Frame(frame) => match self.write_frame(&frame) {
                    Ok(true) => {}
                    Ok(false) => {
                        reason = DiagnosticDoneReason::MaxSeconds;
                        break;
                    }
                    Err(err) => {
                        eprintln!("诊断写入失败: {err:#}");
                        reason = DiagnosticDoneReason::Error;
                        ok = false;
                        break;
                    }
                },
                DiagnosticCommand::Finish(done_reason) => {
                    reason = done_reason;
                    break;
                }
            }
        }
        self.finish(reason, ok);
    }

    fn write_frame(&mut self, frame: &DiagnosticFrame) -> Result<bool> {
        if self
            .max_frames
            .is_some_and(|max_frames| self.written_frames >= max_frames)
        {
            return Ok(false);
        }

        if let Some(writer) = self.mic.as_mut() {
            for v in &frame.near {
                writer.write_sample(*v)?;
            }
        }
        if let Some(writer) = self.reference.as_mut() {
            for v in &frame.far {
                writer.write_sample(*v)?;
            }
        }
        if let Some(writer) = self.out.as_mut() {
            for v in &frame.out {
                writer.write_sample(*v)?;
            }
        }

        let Some(stats) = self.stats.as_mut() else {
            bail!("诊断 stats writer 已关闭");
        };
        writeln!(
            stats,
            "{},{},{},{},{:.2},{:.2},{:.2},{},{},{},{:.2},{:.2},{},{},{},{},{},{},{},{},{:.3},{},{},{}",
            self.frame_index,
            frame.frame_size,
            frame.near_delay_ms,
            frame.near_delay_buffered_samples,
            rms_dbfs(sum_squares(&frame.near), frame.near.len() as u64),
            rms_dbfs(sum_squares(&frame.far), frame.far.len() as u64),
            rms_dbfs(sum_squares(&frame.out), frame.out.len() as u64),
            frame.mic_q,
            frame.ref_q,
            frame.out_q,
            output_queue_latency_ms(frame.out_q, self.sample_rate),
            estimate_user_latency_ms(
                self.frame_ms,
                frame.near_delay_ms,
                frame.algorithmic_latency_ms,
                frame.out_q,
                self.sample_rate
            ),
            frame.aec_estimated_delay_ms,
            frame.mic_input_drops,
            frame.ref_input_drops,
            frame.mic_input_drops + frame.ref_input_drops,
            frame.stale_drops,
            frame.ref_underruns,
            frame.output_overruns,
            frame.output_underruns,
            frame.node_process_time_ms,
            frame.node_runtime_errors,
            frame.node_diverged,
            csv_escape(&frame.node_last_error.clone().unwrap_or_default()),
        )?;
        self.frame_index += 1;
        self.written_frames += frame.frame_size as u64;
        self.status.set_frames(self.written_frames);

        Ok(!self
            .max_frames
            .is_some_and(|max_frames| self.written_frames >= max_frames))
    }

    fn finish(&mut self, reason: DiagnosticDoneReason, ok_so_far: bool) {
        let mut ok = ok_so_far;
        ok &= self.finalize_wav("mic.wav", DiagnosticWavKind::Mic);
        ok &= self.finalize_wav("ref.wav", DiagnosticWavKind::Ref);
        ok &= self.finalize_wav("out.wav", DiagnosticWavKind::Out);
        ok &= self.finalize_stats();

        self.status.set_frames(self.written_frames);
        self.status.set_recording(false);
        self.emit_done(reason, ok);
    }

    fn finalize_wav(&mut self, label: &str, kind: DiagnosticWavKind) -> bool {
        let (writer, part_path, final_path) = match kind {
            DiagnosticWavKind::Mic => (&mut self.mic, &self.mic_part_path, &self.mic_path),
            DiagnosticWavKind::Ref => (&mut self.reference, &self.ref_part_path, &self.ref_path),
            DiagnosticWavKind::Out => (&mut self.out, &self.out_part_path, &self.out_path),
        };
        let Some(writer) = writer.take() else {
            return true;
        };
        if let Err(err) = writer.finalize() {
            eprintln!("写入 {label} 尾部失败: {err}");
            return false;
        }
        if let Err(err) = rename(part_path, final_path) {
            eprintln!("提交 {label} 失败: {err}");
            return false;
        }
        true
    }

    fn finalize_stats(&mut self) -> bool {
        let Some(mut stats) = self.stats.take() else {
            return true;
        };
        let mut ok = true;
        if let Err(err) = stats.flush() {
            eprintln!("刷新诊断 stats.csv 失败: {err}");
            ok = false;
        }
        drop(stats);
        if let Err(err) = rename(&self.stats_part_path, &self.stats_path) {
            eprintln!("提交诊断 stats.csv 失败: {err}");
            ok = false;
        }
        ok
    }

    fn emit_done(&self, reason: DiagnosticDoneReason, ok: bool) {
        if self.status_json {
            let event = json!({
                "type": "diagnostics_done",
                "session_dir": self.dir.display().to_string(),
                "frames": self.written_frames,
                "seconds": self.status.elapsed_s(),
                "reason": reason.as_str(),
                "drops": self.status.drops(),
                "ok": ok,
            });
            println!("{event}");
        } else {
            print_human(
                self.human_to_stderr,
                format!(
                    "诊断录制完成(reason={}, ok={}, drops={}): {}",
                    reason.as_str(),
                    ok,
                    self.status.drops(),
                    self.dir.display()
                ),
            );
        }
    }
}

enum DiagnosticWavKind {
    Mic,
    Ref,
    Out,
}

fn make_session_dir(base: &Path) -> Result<PathBuf> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("系统时间早于 UNIX_EPOCH")?
        .as_secs();
    for attempt in 0..1000 {
        let name = if attempt == 0 {
            format!("session-{now}")
        } else {
            format!("session-{now}-{attempt}")
        };
        let dir = base.join(name);
        match create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("创建诊断 session 目录失败: {}", dir.display()));
            }
        }
    }
    bail!("创建诊断 session 目录失败: {} 下重名过多", base.display())
}

fn write_diagnostic_metadata(
    dir: &Path,
    sample_rate: u32,
    frame_ms: u32,
    near_delay_ms: u32,
    output_level: u32,
    reference_channels: u16,
    max_frames: Option<u64>,
    node_stats: &[ProcessorStats],
) -> Result<()> {
    let mut file = BufWriter::new(
        File::create(dir.join("metadata.txt"))
            .with_context(|| format!("创建诊断 metadata.txt 失败: {}", dir.display()))?,
    );
    writeln!(file, "version={}", env!("CARGO_PKG_VERSION"))?;
    writeln!(file, "sample_rate={sample_rate}")?;
    writeln!(file, "frame_ms={frame_ms}")?;
    writeln!(file, "near_delay_ms={near_delay_ms}")?;
    writeln!(file, "output_level={output_level}")?;
    match output_level_gain_db(output_level) {
        Some(gain_db) => writeln!(file, "output_gain_db={gain_db:.3}")?,
        None => writeln!(file, "output_gain_db=mute")?,
    }
    writeln!(file, "reference_channels={reference_channels}")?;
    if let Some(max_frames) = max_frames {
        writeln!(file, "max_frames={max_frames}")?;
    } else {
        writeln!(file, "max_frames=unbounded")?;
    }
    for (index, node) in node_stats.iter().enumerate() {
        writeln!(file, "node.{index}.name={}", node.name)?;
        if let Some(arch) = &node.selected_gpu_arch {
            writeln!(file, "node.{index}.selected_gpu_arch={arch}")?;
        }
        if let Some(model) = &node.selected_model {
            writeln!(file, "node.{index}.selected_model={model}")?;
        }
        if let Some(err) = &node.last_backend_error {
            writeln!(file, "node.{index}.last_backend_error={err}")?;
        }
    }
    file.flush()?;
    Ok(())
}

fn aggregate_process_time_ms(stats: &[ProcessorStats]) -> f32 {
    stats
        .iter()
        .map(|stat| stat.process_time_ms)
        .fold(0.0, f32::max)
}

fn aggregate_runtime_errors(stats: &[ProcessorStats]) -> u64 {
    stats.iter().map(|stat| stat.runtime_error_count).sum()
}

fn aggregate_estimated_delay_ms(stats: &[ProcessorStats]) -> i32 {
    stats
        .iter()
        .map(|stat| stat.estimated_delay_ms)
        .max()
        .unwrap_or(0)
}

fn aggregate_diverged(stats: &[ProcessorStats]) -> bool {
    stats.iter().any(|stat| stat.diverged)
}

fn aggregate_last_error(stats: &[ProcessorStats]) -> Option<String> {
    stats
        .iter()
        .find_map(|stat| stat.last_backend_error.as_ref())
        .cloned()
}

fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn output_queue_latency_ms(out_q_samples: usize, sample_rate: u32) -> f64 {
    if sample_rate == 0 {
        return 0.0;
    }
    out_q_samples as f64 / sample_rate as f64 * 1000.0
}

fn estimate_user_latency_ms(
    frame_ms: u32,
    near_delay_ms: u32,
    algorithmic_latency_ms: f32,
    out_q_samples: usize,
    sample_rate: u32,
) -> f64 {
    frame_ms as f64 / 2.0
        + near_delay_ms as f64
        + algorithmic_latency_ms as f64
        + output_queue_latency_ms(out_q_samples, sample_rate)
}

const STATUS_WAVE_BUCKETS: usize = 64;

fn peak_waveform(samples: &[f32], buckets: usize) -> Vec<f32> {
    if buckets == 0 {
        return Vec::new();
    }
    if samples.is_empty() {
        return vec![0.0; buckets];
    }
    (0..buckets)
        .map(|bucket| {
            let start = bucket * samples.len() / buckets;
            let end = ((bucket + 1) * samples.len() / buckets).max(start + 1);
            samples[start..end.min(samples.len())]
                .iter()
                .map(|sample| sample.abs().min(1.0))
                .fold(0.0, f32::max)
        })
        .collect()
}

struct RealtimeStats {
    interval: Duration,
    started: Instant,
    last_print: Instant,
    sample_rate: u32,
    frame_ms: u32,
    backend: String,
    near_delay_ms: u32,
    output_level: u32,
    output_gain_db: Option<f32>,
    near_delay_buffered_samples: usize,
    algorithmic_latency_ms: f32,
    status_json: bool,
    diagnostics_session_dir: Option<String>,
    diagnostics_status: Option<DiagnosticsStatusHandle>,
    total_frames: u64,
    near_samples: u64,
    far_samples: u64,
    out_samples: u64,
    near_sq: f64,
    far_sq: f64,
    out_sq: f64,
    near_wave_samples: Vec<f32>,
    far_wave_samples: Vec<f32>,
    out_wave_samples: Vec<f32>,
    mic_q: usize,
    ref_q: usize,
    out_q: usize,
    mic_input_drops: u64,
    ref_input_drops: u64,
    stale_drops: u64,
    ref_underruns: u64,
    output_overruns: u64,
    output_underruns: u64,
    node_process_time_ms: f32,
    node_runtime_errors: u64,
    aec_estimated_delay_ms: i32,
    node_diverged: bool,
    node_last_error: Option<String>,
}

impl RealtimeStats {
    fn new(
        interval: Duration,
        sample_rate: u32,
        frame_ms: u32,
        near_delay_ms: u32,
        output_level: u32,
        backend: String,
        algorithmic_latency_ms: f32,
        status_json: bool,
        diagnostics_session_dir: Option<String>,
        diagnostics_status: Option<DiagnosticsStatusHandle>,
    ) -> Self {
        let now = Instant::now();
        Self {
            interval,
            started: now,
            last_print: now,
            sample_rate,
            frame_ms,
            backend,
            near_delay_ms,
            output_level,
            output_gain_db: output_level_gain_db(output_level),
            near_delay_buffered_samples: 0,
            algorithmic_latency_ms,
            status_json,
            diagnostics_session_dir,
            diagnostics_status,
            total_frames: 0,
            near_samples: 0,
            far_samples: 0,
            out_samples: 0,
            near_sq: 0.0,
            far_sq: 0.0,
            out_sq: 0.0,
            near_wave_samples: Vec::new(),
            far_wave_samples: Vec::new(),
            out_wave_samples: Vec::new(),
            mic_q: 0,
            ref_q: 0,
            out_q: 0,
            mic_input_drops: 0,
            ref_input_drops: 0,
            stale_drops: 0,
            ref_underruns: 0,
            output_overruns: 0,
            output_underruns: 0,
            node_process_time_ms: 0.0,
            node_runtime_errors: 0,
            aec_estimated_delay_ms: 0,
            node_diverged: false,
            node_last_error: None,
        }
    }

    fn set_diagnostics(
        &mut self,
        session_dir: Option<String>,
        status: Option<DiagnosticsStatusHandle>,
    ) {
        self.diagnostics_session_dir = session_dir;
        self.diagnostics_status = status;
    }

    fn set_output_level(&mut self, output_level: u32) {
        self.output_level = output_level;
        self.output_gain_db = output_level_gain_db(output_level);
    }

    fn observe(&mut self, sample: &StatsSample<'_>) {
        self.total_frames += sample.frame_size as u64;
        self.near_samples += sample.near.len() as u64;
        self.far_samples += sample.far.len() as u64;
        self.out_samples += sample.out.len() as u64;
        self.near_sq += sum_squares(sample.near);
        self.far_sq += sum_squares(sample.far);
        self.out_sq += sum_squares(sample.out);
        self.near_wave_samples.extend_from_slice(sample.near);
        self.far_wave_samples.extend_from_slice(sample.far);
        self.out_wave_samples.extend_from_slice(sample.out);
        self.mic_q = sample.mic_q;
        self.ref_q = sample.ref_q;
        self.out_q = sample.out_q;
        self.near_delay_buffered_samples = sample.near_delay_buffered_samples;
        self.mic_input_drops += sample.mic_input_drops;
        self.ref_input_drops += sample.ref_input_drops;
        self.stale_drops += sample.stale_drops;
        self.ref_underruns += sample.ref_underruns;
        self.output_overruns += sample.output_overruns;
        self.output_underruns += sample.output_underruns;
        self.node_process_time_ms = self
            .node_process_time_ms
            .max(aggregate_process_time_ms(sample.node_stats));
        self.node_runtime_errors = aggregate_runtime_errors(sample.node_stats);
        self.aec_estimated_delay_ms = aggregate_estimated_delay_ms(sample.node_stats);
        self.node_diverged = aggregate_diverged(sample.node_stats);
        self.node_last_error = aggregate_last_error(sample.node_stats);
        self.maybe_print();
    }

    fn maybe_print(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_print) < self.interval {
            return;
        }
        if self.status_json {
            println!("{}", self.status_json_line(now));
        } else {
            self.print_text(now);
        }
        self.last_print = now;
        self.near_samples = 0;
        self.far_samples = 0;
        self.out_samples = 0;
        self.near_sq = 0.0;
        self.far_sq = 0.0;
        self.out_sq = 0.0;
        self.near_wave_samples.clear();
        self.far_wave_samples.clear();
        self.out_wave_samples.clear();
        self.mic_input_drops = 0;
        self.ref_input_drops = 0;
        self.stale_drops = 0;
        self.ref_underruns = 0;
        self.output_overruns = 0;
        self.output_underruns = 0;
        self.node_process_time_ms = 0.0;
        self.node_diverged = false;
        self.node_last_error = None;
    }

    fn print_text(&self, now: Instant) {
        println!(
            "t={:.1}s frames={} mic={:.1}dB ref={:.1}dB out={:.1}dB mic_q={} ref_q={} out_q={} near_delay_ms={} out_q_ms={:.1} est_user_ms={:.1} aec_delay_ms={} ref_underrun={} out_underrun={} out_overrun={} input_drop={} stale_drop={} node_ms={:.2} runtime_errors={} diverged={}",
            now.duration_since(self.started).as_secs_f64(),
            self.total_frames,
            rms_dbfs(self.near_sq, self.near_samples),
            rms_dbfs(self.far_sq, self.far_samples),
            rms_dbfs(self.out_sq, self.out_samples),
            self.mic_q,
            self.ref_q,
            self.out_q,
            self.near_delay_ms,
            output_queue_latency_ms(self.out_q, self.sample_rate),
            estimate_user_latency_ms(
                self.frame_ms,
                self.near_delay_ms,
                self.algorithmic_latency_ms,
                self.out_q,
                self.sample_rate
            ),
            self.aec_estimated_delay_ms,
            self.ref_underruns,
            self.output_underruns,
            self.output_overruns,
            self.mic_input_drops + self.ref_input_drops,
            self.stale_drops,
            self.node_process_time_ms,
            self.node_runtime_errors,
            self.node_diverged,
        );
    }

    fn status_json_line(&self, now: Instant) -> String {
        serde_json::to_string(&self.status_value(now)).unwrap_or_else(|err| {
            json!({ "type": "error", "message": err.to_string() }).to_string()
        })
    }

    fn status_value(&self, now: Instant) -> Value {
        let output_queue_latency_ms = output_queue_latency_ms(self.out_q, self.sample_rate);
        let estimated_user_latency_ms = estimate_user_latency_ms(
            self.frame_ms,
            self.near_delay_ms,
            self.algorithmic_latency_ms,
            self.out_q,
            self.sample_rate,
        );
        let (recording, diagnostics_frames, diagnostics_elapsed_s, diagnostics_drops) = self
            .diagnostics_status
            .as_ref()
            .map(|status| {
                (
                    status.is_recording(),
                    status.frames(),
                    status.elapsed_s(),
                    status.drops(),
                )
            })
            .unwrap_or((false, 0, 0.0, 0));
        json!({
            "type": "status",
            "elapsed_s": now.duration_since(self.started).as_secs_f64(),
            "frames": self.total_frames,
            "sample_rate": self.sample_rate,
            "frame_ms": self.frame_ms,
            "backend": self.backend.as_str(),
            "near_delay_ms": self.near_delay_ms,
            "near_delay_buffered_samples": self.near_delay_buffered_samples,
            "output_level": self.output_level,
            "output_gain_db": self.output_gain_db,
            "mic_dbfs": rms_dbfs(self.near_sq, self.near_samples),
            "ref_dbfs": rms_dbfs(self.far_sq, self.far_samples),
            "out_dbfs": rms_dbfs(self.out_sq, self.out_samples),
            "mic_wave": peak_waveform(&self.near_wave_samples, STATUS_WAVE_BUCKETS),
            "ref_wave": peak_waveform(&self.far_wave_samples, STATUS_WAVE_BUCKETS),
            "out_wave": peak_waveform(&self.out_wave_samples, STATUS_WAVE_BUCKETS),
            "mic_q_samples": self.mic_q,
            "ref_q_samples": self.ref_q,
            "out_q_samples": self.out_q,
            "output_queue_latency_ms": output_queue_latency_ms,
            "algorithmic_latency_ms": self.algorithmic_latency_ms,
            "estimated_user_latency_ms": estimated_user_latency_ms,
            "aec_estimated_delay_ms": self.aec_estimated_delay_ms,
            "mic_input_drops": self.mic_input_drops,
            "ref_input_drops": self.ref_input_drops,
            "input_drops": self.mic_input_drops + self.ref_input_drops,
            "stale_drops": self.stale_drops,
            "ref_underruns": self.ref_underruns,
            "output_underruns": self.output_underruns,
            "output_overruns": self.output_overruns,
            "node_process_time_ms": self.node_process_time_ms,
            "runtime_errors": self.node_runtime_errors,
            "diverged": self.node_diverged,
            "last_backend_error": self.node_last_error.as_deref(),
            "diagnostics_session_dir": self.diagnostics_session_dir.as_deref(),
            "recording": recording,
            "diagnostics_frames": diagnostics_frames,
            "diagnostics_elapsed_s": diagnostics_elapsed_s,
            "diagnostics_drops": diagnostics_drops,
        })
    }
}

fn sum_squares(samples: &[f32]) -> f64 {
    samples.iter().map(|v| (*v as f64) * (*v as f64)).sum()
}

fn rms_dbfs(sum_sq: f64, samples: u64) -> f64 {
    if samples == 0 || sum_sq <= 0.0 {
        return -120.0;
    }
    let rms = (sum_sq / samples as f64).sqrt().max(1e-6);
    (20.0 * rms.log10()).max(-120.0)
}

// ── 设备选择 ──────────────────────────────────────────────────────────────────

fn mic_selector(s: &str) -> Option<&str> {
    match s {
        "default" | "" => None,
        other => Some(other),
    }
}
fn output_selector(s: &str) -> Option<&str> {
    match s {
        "default" | "" => None,
        other => Some(other),
    }
}

fn select_default_device(host: &cpal::Host, kind: DeviceKind) -> Result<SelectedDevice> {
    let device = match kind {
        DeviceKind::Input => host.default_input_device(),
        DeviceKind::Output => host.default_output_device(),
    }
    .with_context(|| format!("无默认 {} 设备", kind.label()))?;
    let devices = devices_for(host, kind).unwrap_or_default();
    let index = find_device_index(&devices, &device);
    Ok(SelectedDevice { index, device })
}

fn select_device(
    host: &cpal::Host,
    kind: DeviceKind,
    selector: Option<&str>,
) -> Result<SelectedDevice> {
    if let Some(selector) = selector {
        let devices = devices_for(host, kind)?;
        if let Ok(index) = selector.parse::<usize>() {
            let device = devices
                .get(index)
                .cloned()
                .with_context(|| format!("无 {} 设备索引 {index}", kind.label()))?;
            return Ok(SelectedDevice {
                index: Some(index),
                device,
            });
        }
        let needle = selector.to_lowercase();
        return devices
            .into_iter()
            .enumerate()
            .find(|(index, d)| device_matches_selector(d, kind, *index, selector, &needle))
            .map(|(index, device)| SelectedDevice {
                index: Some(index),
                device,
            })
            .with_context(|| format!("无名称含 {selector:?} 的 {} 设备", kind.label()));
    }
    select_default_device(host, kind)
}

fn select_reference_source(host: &cpal::Host, selector: &str) -> Result<ReferenceSource> {
    match selector {
        "none" | "" => Ok(ReferenceSource::None),
        "system" | "default" => select_system_reference_source(host),
        sel => {
            let (device, kind) = select_render_device(host, sel)?;
            Ok(ReferenceSource::Cpal { device, kind })
        }
    }
}

#[cfg(target_os = "macos")]
fn select_system_reference_source(_host: &cpal::Host) -> Result<ReferenceSource> {
    Ok(ReferenceSource::ProcessTap)
}

#[cfg(not(target_os = "macos"))]
fn select_system_reference_source(host: &cpal::Host) -> Result<ReferenceSource> {
    Ok(ReferenceSource::Cpal {
        device: select_default_device(host, DeviceKind::Output)
            .context("无默认输出设备可作系统 loopback")?,
        kind: DeviceKind::Output,
    })
}

fn is_macos_process_tap(source: &ReferenceSource) -> bool {
    #[cfg(target_os = "macos")]
    {
        matches!(source, ReferenceSource::ProcessTap)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = source;
        false
    }
}

fn macos_process_tap_sample_rate() -> u32 {
    #[cfg(target_os = "macos")]
    {
        macos_process_tap::SAMPLE_RATE
    }
    #[cfg(not(target_os = "macos"))]
    {
        48_000
    }
}

fn select_render_device(host: &cpal::Host, selector: &str) -> Result<(SelectedDevice, DeviceKind)> {
    if let Some((prefix, sel)) = selector.split_once(':') {
        let kind = match prefix.to_lowercase().as_str() {
            "input" | "in" => DeviceKind::Input,
            "output" | "out" => DeviceKind::Output,
            _ => bail!("参考设备前缀须为 input: 或 output:"),
        };
        return Ok((select_device(host, kind, Some(sel))?, kind));
    }
    if let Ok(d) = select_device(host, DeviceKind::Output, Some(selector)) {
        return Ok((d, DeviceKind::Output));
    }
    select_device(host, DeviceKind::Input, Some(selector)).map(|d| (d, DeviceKind::Input))
}

fn devices_for(host: &cpal::Host, kind: DeviceKind) -> Result<Vec<Device>> {
    match kind {
        DeviceKind::Input => Ok(host.input_devices()?.collect()),
        DeviceKind::Output => Ok(host.output_devices()?.collect()),
    }
}

fn pick_config(device: &Device, kind: DeviceKind, sample_rate: u32) -> Result<StreamConfigChoice> {
    let ranges: Vec<SupportedStreamConfigRange> = match kind {
        DeviceKind::Input => device.supported_input_configs()?.collect(),
        DeviceKind::Output => device.supported_output_configs()?.collect(),
    };
    let ranges = ranges
        .into_iter()
        .filter(|r| !r.sample_format().is_dsd())
        .collect::<Vec<_>>();

    if let Some(exact) = ranges
        .iter()
        .filter(|r| r.min_sample_rate() <= sample_rate && sample_rate <= r.max_sample_rate())
        .max_by(|a, b| a.cmp_default_heuristics(b))
    {
        return Ok(StreamConfigChoice::new(
            exact.clone().with_sample_rate(sample_rate),
            sample_rate,
        ));
    }

    if let Ok(default) = default_config(device, kind) {
        if !default.sample_format().is_dsd() {
            return Ok(StreamConfigChoice::new(default, sample_rate));
        }
    }

    ranges
        .into_iter()
        .map(|range| {
            let chosen_rate = sample_rate
                .max(range.min_sample_rate())
                .min(range.max_sample_rate());
            let distance = chosen_rate.abs_diff(sample_rate);
            (distance, range.with_sample_rate(chosen_rate))
        })
        .min_by(|(a_distance, a), (b_distance, b)| {
            a_distance
                .cmp(b_distance)
                .then_with(|| b.channels().cmp(&a.channels()))
        })
        .map(|(_, config)| StreamConfigChoice::new(config, sample_rate))
        .with_context(|| {
            format!(
                "{} 没有可用的非 DSD {} 配置",
                device_label(device),
                kind.label()
            )
        })
}

fn default_config(device: &Device, kind: DeviceKind) -> Result<SupportedStreamConfig> {
    match kind {
        DeviceKind::Input => device.default_input_config(),
        DeviceKind::Output => device.default_output_config(),
    }
    .with_context(|| format!("{} 没有默认 {} 配置", device_label(device), kind.label()))
}

// ── 流构建(多采样格式)────────────────────────────────────────────────────────

macro_rules! dispatch_format {
    ($fmt:expr, $build:ident, $($arg:expr),+) => {
        match $fmt {
            SampleFormat::I16 => $build::<i16, _>($($arg),+),
            SampleFormat::I32 => $build::<i32, _>($($arg),+),
            SampleFormat::F32 => $build::<f32, _>($($arg),+),
            SampleFormat::U16 => $build::<u16, _>($($arg),+),
            other => bail!("不支持的采样格式 {other}"),
        }
    };
}

struct InterleavedLinearResampler {
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    input_frames_seen: u64,
    next_output_source_pos: f64,
    prev_frame: Option<Vec<f32>>,
}

impl InterleavedLinearResampler {
    fn new(in_rate: u32, out_rate: u32, channels: usize) -> Self {
        Self {
            in_rate,
            out_rate,
            channels: channels.max(1),
            input_frames_seen: 0,
            next_output_source_pos: 0.0,
            prev_frame: None,
        }
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if self.in_rate == self.out_rate || input.is_empty() {
            return input.to_vec();
        }
        let frames = input.len() / self.channels;
        if frames == 0 {
            return Vec::new();
        }

        let start_abs = self.input_frames_seen;
        let end_abs = start_abs + frames as u64;
        let step = self.in_rate as f64 / self.out_rate as f64;
        let mut out = Vec::with_capacity(((frames as f64) / step).ceil() as usize * self.channels);

        while self.next_output_source_pos.floor() as u64 + 1 < end_abs {
            let pos = self.next_output_source_pos;
            let i0 = pos.floor() as u64;
            let i1 = i0 + 1;
            let frac = (pos - i0 as f64) as f32;
            for ch in 0..self.channels {
                let a = self.sample_at(input, start_abs, i0, ch).unwrap_or(0.0);
                let b = self.sample_at(input, start_abs, i1, ch).unwrap_or(a);
                out.push(a + (b - a) * frac);
            }
            self.next_output_source_pos += step;
        }

        self.input_frames_seen = end_abs;
        let last_start = (frames - 1) * self.channels;
        self.prev_frame = Some(input[last_start..last_start + self.channels].to_vec());
        out
    }

    fn sample_at(
        &self,
        input: &[f32],
        start_abs: u64,
        index_abs: u64,
        channel: usize,
    ) -> Option<f32> {
        if index_abs + 1 == start_abs {
            return self
                .prev_frame
                .as_ref()
                .and_then(|frame| frame.get(channel).copied());
        }
        if index_abs < start_abs {
            return None;
        }
        let local = (index_abs - start_abs) as usize;
        input.get(local * self.channels + channel).copied()
    }
}

struct OutputLinearResampler {
    step: f64,
    pos: f64,
    buffer: VecDeque<f32>,
}

impl OutputLinearResampler {
    fn new(in_rate: u32, out_rate: u32) -> Self {
        let step = if out_rate == 0 {
            1.0
        } else {
            in_rate as f64 / out_rate as f64
        };
        Self {
            step,
            pos: 0.0,
            buffer: VecDeque::new(),
        }
    }

    fn next_sample<C>(&mut self, consumer: &mut C, underruns: &AtomicU64) -> f32
    where
        C: Consumer<Item = f32>,
    {
        let needed = (self.pos.floor() as usize).saturating_add(2);
        while self.buffer.len() < needed {
            match consumer.try_pop() {
                Some(sample) => self.buffer.push_back(sample.clamp(-1.0, 1.0)),
                None => {
                    underruns.fetch_add(1, Ordering::Relaxed);
                    return 0.0;
                }
            }
        }

        let i0 = self.pos.floor() as usize;
        let frac = (self.pos - i0 as f64) as f32;
        let a = self.buffer.get(i0).copied().unwrap_or(0.0);
        let b = self.buffer.get(i0 + 1).copied().unwrap_or(a);
        let sample = (a + (b - a) * frac).clamp(-1.0, 1.0);

        self.pos += self.step;
        let consumed = self.pos.floor() as usize;
        for _ in 0..consumed {
            let _ = self.buffer.pop_front();
        }
        self.pos -= consumed as f64;
        sample
    }
}

fn build_input_stream<P>(
    device: &Device,
    config: &StreamConfigChoice,
    producer: P,
    label: &'static str,
    channel_mode: InputChannelMode,
    drops: Arc<AtomicU64>,
) -> Result<Stream>
where
    P: Producer<Item = f32> + Send + 'static,
{
    dispatch_format!(
        config.sample_format(),
        build_input_stream_t,
        device,
        config,
        producer,
        label,
        channel_mode,
        drops
    )
}

fn build_input_stream_t<T, P>(
    device: &Device,
    choice: &StreamConfigChoice,
    mut producer: P,
    label: &'static str,
    channel_mode: InputChannelMode,
    drops: Arc<AtomicU64>,
) -> Result<Stream>
where
    T: SizedSample + Copy + Send + 'static,
    f32: FromSample<T>,
    P: Producer<Item = f32> + Send + 'static,
{
    let config = choice.config();
    let channels = usize::from(config.channels);
    let pipeline_channels = channel_mode.output_channels();
    let mut resampler = InterleavedLinearResampler::new(
        choice.stream_sample_rate(),
        choice.pipeline_sample_rate,
        pipeline_channels,
    );
    let needs_resampling = choice.requires_resampling();
    device
        .build_input_stream(
            &config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                if needs_resampling {
                    let mut mapped =
                        Vec::with_capacity((data.len() / channels) * pipeline_channels);
                    for frame in data.chunks(channels) {
                        map_input_frame(frame, channel_mode, &mut mapped);
                    }
                    for sample in resampler.process(&mapped) {
                        if producer.try_push(sample).is_err() {
                            drops.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                } else {
                    for frame in data.chunks(channels) {
                        push_input_frame(frame, channel_mode, &mut producer, &drops);
                    }
                }
            },
            move |err| eprintln!("{label} 流错误: {err}"),
            None,
        )
        .with_context(|| format!("构建 {label} 输入流失败"))
}

fn push_input_frame<T, P>(
    frame: &[T],
    channel_mode: InputChannelMode,
    producer: &mut P,
    drops: &AtomicU64,
) where
    T: Copy,
    f32: FromSample<T>,
    P: Producer<Item = f32>,
{
    match channel_mode {
        InputChannelMode::MonoDownmix => {
            let sample = downmix_frame(frame);
            if producer.try_push(sample).is_err() {
                drops.fetch_add(1, Ordering::Relaxed);
            }
        }
        InputChannelMode::PreserveFirst(channels) => {
            for ch in 0..channels {
                let sample = frame.get(ch).copied().map(f32::from_sample).unwrap_or(0.0);
                if producer.try_push(sample).is_err() {
                    drops.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

fn map_input_frame<T>(frame: &[T], channel_mode: InputChannelMode, out: &mut Vec<f32>)
where
    T: Copy,
    f32: FromSample<T>,
{
    match channel_mode {
        InputChannelMode::MonoDownmix => out.push(downmix_frame(frame)),
        InputChannelMode::PreserveFirst(channels) => {
            for ch in 0..channels {
                out.push(frame.get(ch).copied().map(f32::from_sample).unwrap_or(0.0));
            }
        }
    }
}

fn downmix_frame<T>(frame: &[T]) -> f32
where
    T: Copy,
    f32: FromSample<T>,
{
    let sum = frame.iter().copied().map(f32::from_sample).sum::<f32>();
    if frame.is_empty() {
        0.0
    } else {
        sum / frame.len() as f32
    }
}

fn build_output_stream<C>(
    device: &Device,
    config: &StreamConfigChoice,
    consumer: C,
    underruns: Arc<AtomicU64>,
) -> Result<Stream>
where
    C: Consumer<Item = f32> + Send + 'static,
{
    dispatch_format!(
        config.sample_format(),
        build_output_stream_t,
        device,
        config,
        consumer,
        underruns
    )
}

fn build_output_stream_t<T, C>(
    device: &Device,
    choice: &StreamConfigChoice,
    mut consumer: C,
    underruns: Arc<AtomicU64>,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32> + Copy + Send + 'static,
    C: Consumer<Item = f32> + Send + 'static,
{
    let config = choice.config();
    let channels = usize::from(config.channels);
    let mut resampler =
        OutputLinearResampler::new(choice.pipeline_sample_rate, choice.stream_sample_rate());
    let needs_resampling = choice.requires_resampling();
    device
        .build_output_stream(
            &config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels) {
                    let sample = if needs_resampling {
                        resampler.next_sample(&mut consumer, &underruns)
                    } else {
                        match consumer.try_pop() {
                            Some(v) => v.clamp(-1.0, 1.0),
                            None => {
                                underruns.fetch_add(1, Ordering::Relaxed);
                                0.0
                            }
                        }
                    };
                    let s = T::from_sample(sample);
                    for out in frame {
                        *out = s; // 单声道铺到所有输出声道
                    }
                }
            },
            |err| eprintln!("输出流错误: {err}"),
            None,
        )
        .context("构建输出流失败")
}

// ── 设备列表 ──────────────────────────────────────────────────────────────────

pub fn print_devices() -> Result<()> {
    let host = cpal::default_host();
    for kind in [DeviceKind::Input, DeviceKind::Output] {
        println!("{} 设备:", kind.label());
        for (i, d) in devices_for(&host, kind)?.iter().enumerate() {
            let cfg = match kind {
                DeviceKind::Input => d.default_input_config(),
                DeviceKind::Output => d.default_output_config(),
            };
            let summary = cfg
                .map(|c| config_summary(&c))
                .unwrap_or_else(|e| format!("无默认配置: {e}"));
            println!(
                "  {} ({summary})",
                format_indexed_label(Some(i), device_label(d))
            );
        }
    }
    println!(
        "\n用法:run --config 配置文件;也可用 --mic / --reference / --output 覆盖配置里的设备选择。"
    );
    println!(
        "reference 还支持 'system'(Win=默认输出 loopback,mac=Process Tap)/ 'none' / 'output:<名>' / 'input:<名>'。"
    );
    Ok(())
}

pub fn devices_json() -> Result<Value> {
    let host = cpal::default_host();
    let input_devices = devices_for(&host, DeviceKind::Input)?;
    let output_devices = devices_for(&host, DeviceKind::Output)?;
    let default_input_index = host
        .default_input_device()
        .as_ref()
        .and_then(|device| find_device_index(&input_devices, device));
    let default_output_index = host
        .default_output_device()
        .as_ref()
        .and_then(|device| find_device_index(&output_devices, device));

    let inputs = input_devices
        .iter()
        .enumerate()
        .map(|(index, device)| {
            device_json_entry(device, DeviceKind::Input, index, default_input_index)
        })
        .collect::<Vec<_>>();
    let outputs = output_devices
        .iter()
        .enumerate()
        .map(|(index, device)| {
            device_json_entry(device, DeviceKind::Output, index, default_output_index)
        })
        .collect::<Vec<_>>();

    let mut reference_sources = vec![
        system_reference_source(default_output_index.is_some()),
        json!({
            "id": "none",
            "stable_id": "none",
            "label": "No reference",
            "kind": "none",
            "available": true,
            "hint": "No far-end reference; AEC will behave like single-ended processing."
        }),
    ];
    reference_sources.extend(input_devices.iter().enumerate().map(|(index, device)| {
        json!({
            "id": format!("input:{index}"),
            "stable_id": format!("input:{}", stable_device_id(device, DeviceKind::Input, index)),
            "label": device_label(device),
            "kind": "input",
            "device_index": index,
            "selector": format!("input:{}", stable_device_id(device, DeviceKind::Input, index)),
            "available": true,
        })
    }));
    if !cfg!(target_os = "macos") {
        reference_sources.extend(output_devices.iter().enumerate().map(|(index, device)| {
            json!({
                "id": format!("output:{index}"),
                "stable_id": format!("output:{}", stable_device_id(device, DeviceKind::Output, index)),
                "label": device_label(device),
                "kind": "output",
                "device_index": index,
                "selector": format!("output:{}", stable_device_id(device, DeviceKind::Output, index)),
                "available": true,
            })
        }));
    }

    Ok(json!({
        "ok": true,
        "inputs": inputs,
        "outputs": outputs,
        "reference_sources": reference_sources,
    }))
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AudioDoctorOptions {
    pub request_system_audio: bool,
}

pub fn audio_doctor_json_with_options(options: AudioDoctorOptions) -> Result<Value> {
    let devices = devices_json()?;
    let inputs = devices["inputs"].as_array().cloned().unwrap_or_default();
    let outputs = devices["outputs"].as_array().cloned().unwrap_or_default();
    let candidate_inputs = inputs
        .iter()
        .filter(|entry| is_virtual_audio_name(entry["name"].as_str().unwrap_or_default()))
        .map(audio_candidate_json)
        .collect::<Vec<_>>();
    let candidate_outputs = outputs
        .iter()
        .filter(|entry| is_virtual_audio_name(entry["name"].as_str().unwrap_or_default()))
        .map(audio_candidate_json)
        .collect::<Vec<_>>();
    let virtual_output_detected = !candidate_outputs.is_empty();
    let install_status = match (candidate_inputs.is_empty(), candidate_outputs.is_empty()) {
        (false, false) => "installed",
        (true, true) => "missing",
        _ => "unknown",
    };
    let system_audio_probe = options
        .request_system_audio
        .then(request_system_audio_permission);
    let system_audio_permission = system_audio_probe
        .as_ref()
        .map(|probe| probe.state)
        .unwrap_or_else(system_audio_permission_state);

    Ok(json!({
        "ok": true,
        "platform": std::env::consts::OS,
        "virtual_output_detected": virtual_output_detected,
        "candidate_outputs": candidate_outputs,
        "candidate_inputs": candidate_inputs,
        "recommended_driver": recommended_audio_driver(),
        "install_status": install_status,
        "needs_reboot": false,
        "permission_state": audio_permission_state(&inputs),
        "system_audio_permission": system_audio_permission,
        "system_audio_permission_probe": system_audio_probe.as_ref().map(SystemAudioPermissionProbe::to_json),
        "reference_sources": devices["reference_sources"].clone(),
        "hint": audio_doctor_hint(install_status),
    }))
}

struct SystemAudioPermissionProbe {
    state: &'static str,
    ok: bool,
    detail: String,
}

impl SystemAudioPermissionProbe {
    fn to_json(&self) -> Value {
        json!({
            "requested": true,
            "ok": self.ok,
            "state": self.state,
            "detail": self.detail,
        })
    }
}

fn device_json_entry(
    device: &Device,
    kind: DeviceKind,
    index: usize,
    default_index: Option<usize>,
) -> Value {
    let cfg = match kind {
        DeviceKind::Input => device.default_input_config(),
        DeviceKind::Output => device.default_output_config(),
    };
    let (default_sample_rate, channels, sample_format, config_error) = match cfg {
        Ok(cfg) => (
            Some(cfg.sample_rate()),
            Some(cfg.channels()),
            Some(cfg.sample_format().to_string()),
            None,
        ),
        Err(err) => (None, None, None, Some(err.to_string())),
    };
    json!({
        "id": index.to_string(),
        "stable_id": stable_device_id(device, kind, index),
        "index": index,
        "name": device_label(device),
        "kind": kind.label(),
        "is_default": default_index == Some(index),
        "selector": index.to_string(),
        "default_sample_rate": default_sample_rate,
        "channels": channels,
        "sample_format": sample_format,
        "supported_sample_rates": supported_sample_rates_json(device, kind),
        "config_error": config_error,
    })
}

fn supported_sample_rates_json(device: &Device, kind: DeviceKind) -> Value {
    match kind {
        DeviceKind::Input => match device.supported_input_configs() {
            Ok(ranges) => supported_ranges_json(ranges),
            Err(err) => json!({ "error": err.to_string() }),
        },
        DeviceKind::Output => match device.supported_output_configs() {
            Ok(ranges) => supported_ranges_json(ranges),
            Err(err) => json!({ "error": err.to_string() }),
        },
    }
}

fn supported_ranges_json(ranges: impl Iterator<Item = SupportedStreamConfigRange>) -> Value {
    Value::Array(
        ranges
            .filter(|range| !range.sample_format().is_dsd())
            .map(|range| {
                json!({
                    "min": range.min_sample_rate(),
                    "max": range.max_sample_rate(),
                    "channels": range.channels(),
                    "sample_format": range.sample_format().to_string(),
                })
            })
            .collect(),
    )
}

fn system_reference_source(has_default_output: bool) -> Value {
    let available = if cfg!(windows) {
        has_default_output
    } else if cfg!(target_os = "macos") {
        macos_process_tap_helper_available()
    } else {
        false
    };
    let hint = if cfg!(windows) {
        if has_default_output {
            "Windows default render endpoint loopback is available."
        } else {
            "No default output device is available for system loopback."
        }
    } else if cfg!(target_os = "macos") {
        if available {
            "macOS Process Tap helper is available; first use may request System Audio Recording permission."
        } else {
            "macOS Process Tap helper is not bundled/built; use BlackHole/VB-CABLE MAC fallback or build the helper."
        }
    } else {
        "System loopback availability depends on the platform backend; use an explicit routed reference source if unavailable."
    };
    json!({
        "id": "system",
        "stable_id": "system",
        "label": if cfg!(target_os = "macos") { "System Audio (Process Tap)" } else { "System audio" },
        "kind": "system",
        "available": available,
        "hint": hint,
    })
}

fn macos_process_tap_helper_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos_process_tap::helper_available()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn audio_candidate_json(entry: &Value) -> Value {
    json!({
        "name": entry["name"].as_str().unwrap_or_default(),
        "selector": entry["selector"].as_str().unwrap_or_default(),
        "stable_id": entry["stable_id"].as_str().unwrap_or_default(),
        "index": entry["index"].as_u64(),
        "kind": entry["kind"].as_str().unwrap_or_default(),
    })
}

fn recommended_audio_driver() -> &'static str {
    if cfg!(windows) {
        "vb-cable"
    } else if cfg!(target_os = "macos") {
        "blackhole-2ch"
    } else {
        "unknown"
    }
}

fn audio_permission_state(inputs: &[Value]) -> &'static str {
    if cfg!(target_os = "macos") {
        if inputs.is_empty() {
            "undetermined"
        } else {
            "granted"
        }
    } else {
        "unknown"
    }
}

fn system_audio_permission_state() -> &'static str {
    if cfg!(target_os = "macos") {
        if macos_process_tap_helper_available() {
            "undetermined"
        } else {
            "unknown"
        }
    } else {
        "unknown"
    }
}

fn request_system_audio_permission() -> SystemAudioPermissionProbe {
    #[cfg(target_os = "macos")]
    {
        match macos_process_tap::probe_permission() {
            Ok(detail) => SystemAudioPermissionProbe {
                state: "granted",
                ok: true,
                detail,
            },
            Err(err) => {
                let detail = format!("{err:#}");
                let state = if detail.contains("未找到 macOS Process Tap helper")
                    || detail.contains("Process Tap availability")
                {
                    "unknown"
                } else {
                    "denied"
                };
                SystemAudioPermissionProbe {
                    state,
                    ok: false,
                    detail,
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        SystemAudioPermissionProbe {
            state: "unknown",
            ok: false,
            detail: "System Audio Recording permission is macOS-only.".to_string(),
        }
    }
}

fn audio_doctor_hint(install_status: &str) -> &'static str {
    match install_status {
        "installed" => "Virtual audio input/output candidates were detected.",
        "missing" => {
            if cfg!(target_os = "macos") {
                "No virtual audio candidates detected; install BlackHole 2ch or VB-CABLE MAC, then refresh devices."
            } else if cfg!(windows) {
                "No virtual audio candidates detected; install VB-Audio VB-CABLE, then refresh devices."
            } else {
                "No virtual audio candidates detected; configure a platform virtual audio route, then refresh devices."
            }
        }
        _ => "Only one side of a virtual audio route was detected; verify the driver and refresh devices.",
    }
}

fn is_virtual_audio_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("vb-audio")
        || name.contains("vb-cable")
        || name.contains("vbcable")
        || name.contains("cable input")
        || name.contains("cable output")
        || name.contains("blackhole")
        || name.contains("virtual desktop")
}

fn stable_device_id(device: &Device, kind: DeviceKind, index: usize) -> String {
    if let Ok(id) = device.id() {
        let id = id.to_string();
        if !id.trim().is_empty() {
            return id;
        }
    }
    if let Ok(desc) = device.description() {
        if let Some(address) = desc.address().filter(|value| !value.trim().is_empty()) {
            return format!("{}:{}", kind.label(), address.trim());
        }
    }
    format!(
        "{}:name:{}",
        kind.label(),
        normalize_device_id_part(&device_label(device), index)
    )
}

fn normalize_device_id_part(label: &str, index: usize) -> String {
    let mut out = String::new();
    for ch in label.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        format!("device-{index}")
    } else {
        out.to_string()
    }
}

fn config_summary(c: &SupportedStreamConfig) -> String {
    format!(
        "{} Hz, {} ch, {}",
        c.sample_rate(),
        c.channels(),
        c.sample_format()
    )
}

fn config_choice_summary(c: &StreamConfigChoice) -> String {
    let base = config_summary(&c.supported);
    if c.requires_resampling() {
        format!("{base}, resample to {} Hz pipeline", c.pipeline_sample_rate)
    } else {
        base
    }
}

fn selected_device_label(selected: &SelectedDevice) -> String {
    format_indexed_label(selected.index, device_label(&selected.device))
}

fn format_indexed_label(index: Option<usize>, label: String) -> String {
    match index {
        Some(index) => format!("[{index}] {label}"),
        None => label,
    }
}

fn device_label(device: &Device) -> String {
    device
        .description()
        .map(|d| format_device_description(&d))
        .unwrap_or_else(|_| "<未知>".to_owned())
}

fn format_device_description(desc: &DeviceDescription) -> String {
    let name = desc.name().trim();
    let detail = desc
        .driver()
        .filter(|v| distinct_label(name, v))
        .or_else(|| {
            desc.extended()
                .iter()
                .map(String::as_str)
                .find(|v| distinct_label(name, v))
        })
        .or_else(|| desc.manufacturer().filter(|v| distinct_label(name, v)));

    match detail {
        Some(detail) => format!("{name} / {}", detail.trim()),
        None => {
            let display = desc.to_string();
            let display = display.trim();
            if display.is_empty() {
                name.to_owned()
            } else {
                display.to_owned()
            }
        }
    }
}

fn distinct_label(primary: &str, candidate: &str) -> bool {
    let primary = primary.trim();
    let candidate = candidate.trim();
    !candidate.is_empty() && !candidate.eq_ignore_ascii_case(primary)
}

fn device_search_text(device: &Device) -> String {
    let mut parts = Vec::new();
    if let Ok(desc) = device.description() {
        parts.push(desc.name().to_owned());
        parts.extend(desc.manufacturer().map(str::to_owned));
        parts.extend(desc.driver().map(str::to_owned));
        parts.extend(desc.address().map(str::to_owned));
        parts.extend(desc.extended().iter().cloned());
        parts.push(desc.to_string());
    }
    if let Ok(id) = device.id() {
        parts.push(id.to_string());
    }
    parts.join(" ")
}

fn device_matches_selector(
    device: &Device,
    kind: DeviceKind,
    index: usize,
    selector: &str,
    lower_selector: &str,
) -> bool {
    stable_device_id(device, kind, index).eq_ignore_ascii_case(selector)
        || device_search_text(device)
            .to_lowercase()
            .contains(lower_selector)
}

fn find_device_index(devices: &[Device], selected: &Device) -> Option<usize> {
    selected.id().ok().and_then(|id| {
        devices
            .iter()
            .position(|device| device.id().ok().as_ref() == Some(&id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::{DeviceDescriptionBuilder, DeviceType, InterfaceType};

    #[test]
    fn device_description_label_prefers_driver_detail() {
        let desc = DeviceDescriptionBuilder::new("麦克风")
            .driver("USB Condenser Microphone")
            .device_type(DeviceType::Microphone)
            .interface_type(InterfaceType::Usb)
            .build();

        assert_eq!(
            format_device_description(&desc),
            "麦克风 / USB Condenser Microphone"
        );
    }

    #[test]
    fn rms_dbfs_reports_silence_and_full_scale() {
        assert_eq!(rms_dbfs(0.0, 480), -120.0);
        assert_eq!(rms_dbfs(480.0, 480), 0.0);
    }

    #[test]
    fn input_channel_mode_downmixes_or_preserves_stereo() {
        let drops = AtomicU64::new(0);
        let (mut mono_prod, mut mono_cons) = HeapRb::<f32>::new(4).split();
        push_input_frame(
            &[0.0f32, 0.5, 1.0],
            InputChannelMode::MonoDownmix,
            &mut mono_prod,
            &drops,
        );
        let mut mono = [0.0f32; 1];
        mono_cons.pop_slice(&mut mono);
        assert_eq!(mono[0], 0.5);

        let (mut stereo_prod, mut stereo_cons) = HeapRb::<f32>::new(4).split();
        push_input_frame(
            &[0.25f32, -0.75, 0.5],
            InputChannelMode::PreserveFirst(2),
            &mut stereo_prod,
            &drops,
        );
        let mut stereo = [0.0f32; 2];
        stereo_cons.pop_slice(&mut stereo);
        assert_eq!(stereo, [0.25, -0.75]);
    }

    #[test]
    fn input_resampler_upsamples_and_preserves_state() {
        let mut resampler = InterleavedLinearResampler::new(24_000, 48_000, 1);

        let first = resampler.process(&[0.0, 1.0, 2.0, 3.0]);
        let second = resampler.process(&[4.0, 5.0]);

        assert_eq!(first, vec![0.0, 0.5, 1.0, 1.5, 2.0, 2.5]);
        assert_eq!(second, vec![3.0, 3.5, 4.0, 4.5]);
    }

    #[test]
    fn input_resampler_downsamples_fixed_ratio() {
        let mut resampler = InterleavedLinearResampler::new(48_000, 24_000, 1);

        let out = resampler.process(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);

        assert_eq!(out, vec![0.0, 2.0, 4.0]);
    }

    #[test]
    fn output_resampler_pulls_pipeline_samples_at_device_rate() {
        let drops = AtomicU64::new(0);
        let (mut prod, mut cons) = HeapRb::<f32>::new(8).split();
        assert_eq!(prod.push_slice(&[0.0, 0.25, 0.5, 0.75]), 4);
        let mut resampler = OutputLinearResampler::new(48_000, 24_000);
        assert_eq!(resampler.step, 2.0);

        let first = resampler.next_sample(&mut cons, &drops);
        assert_eq!(resampler.pos, 0.0);
        assert_eq!(resampler.buffer.len(), 0);
        let second = resampler.next_sample(&mut cons, &drops);

        assert_eq!(first, 0.0);
        assert_eq!(second, 0.5);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn diagnostic_recorder_writes_audio_and_stats() -> Result<()> {
        let base = std::env::temp_dir().join(format!(
            "echoless-diagnostic-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cfg = DiagnosticsConfig {
            record_dir: Some(base.to_string_lossy().to_string()),
            max_seconds: Some(1),
        };
        let node_stats = [ProcessorStats::empty("test")];
        let mut recorder =
            DiagnosticRecorder::new(&cfg, 48_000, 2, 10, 25, 75, &node_stats, false)?.unwrap();
        let dir = recorder.dir.clone();
        let near = [0.25f32, -0.25];
        let far = [0.1f32, -0.1, 0.2, -0.2];
        let out = [0.0f32, 0.1];
        let sample = StatsSample {
            algorithmic_latency_ms: 16.0,
            near_delay_ms: 25,
            near_delay_buffered_samples: 1200,
            frame_size: near.len(),
            near: &near,
            far: &far,
            out: &out,
            mic_q: 0,
            ref_q: 0,
            out_q: 0,
            mic_input_drops: 0,
            ref_input_drops: 0,
            stale_drops: 0,
            ref_underruns: 0,
            output_overruns: 0,
            output_underruns: 0,
            node_stats: &node_stats,
        };

        assert!(recorder.write_frame(&sample)?);
        drop(recorder);

        let mic_reader = hound::WavReader::open(dir.join("mic.wav"))?;
        assert_eq!(mic_reader.spec().channels, 1);
        assert_eq!(mic_reader.spec().sample_rate, 48_000);
        let ref_reader = hound::WavReader::open(dir.join("ref.wav"))?;
        assert_eq!(ref_reader.spec().channels, 2);
        let stats = std::fs::read_to_string(dir.join("stats.csv"))?;
        assert_eq!(stats.lines().count(), 2);
        assert!(stats.contains("node_process_time_ms"));
        assert!(stats.contains("estimated_user_latency_ms"));
        assert!(stats.contains("near_delay_ms"));
        assert!(dir.join("out.wav").exists());
        let metadata = std::fs::read_to_string(dir.join("metadata.txt"))?;
        assert!(metadata.contains("near_delay_ms=25"));
        assert!(metadata.contains("output_level=75"));

        let _ = std::fs::remove_dir_all(base);
        Ok(())
    }

    #[test]
    fn diagnostic_recorder_auto_finishes_at_max_seconds() -> Result<()> {
        let base = std::env::temp_dir().join(format!(
            "echoless-diagnostic-max-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cfg = DiagnosticsConfig {
            record_dir: Some(base.to_string_lossy().to_string()),
            max_seconds: Some(1),
        };
        let node_stats = [ProcessorStats::empty("test")];
        let mut recorder =
            DiagnosticRecorder::new(&cfg, 2, 1, 1000, 0, 50, &node_stats, false)?.unwrap();
        let dir = recorder.dir.clone();
        let status = recorder.status_handle();
        let near = [0.25f32, -0.25];
        let far = [0.1f32, -0.1];
        let out = [0.0f32, 0.1];
        let sample = StatsSample {
            algorithmic_latency_ms: 16.0,
            near_delay_ms: 0,
            near_delay_buffered_samples: 0,
            frame_size: near.len(),
            near: &near,
            far: &far,
            out: &out,
            mic_q: 0,
            ref_q: 0,
            out_q: 0,
            mic_input_drops: 0,
            ref_input_drops: 0,
            stale_drops: 0,
            ref_underruns: 0,
            output_overruns: 0,
            output_underruns: 0,
            node_stats: &node_stats,
        };

        assert!(recorder.write_frame(&sample)?);
        for _ in 0..50 {
            if !status.is_recording() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert!(!status.is_recording());
        assert_eq!(status.frames(), 2);
        assert_eq!(status.elapsed_s(), 1.0);
        assert!(dir.join("mic.wav").exists());
        assert!(dir.join("ref.wav").exists());
        assert!(dir.join("out.wav").exists());
        assert!(dir.join("stats.csv").exists());
        assert!(!dir.join("mic.wav.part").exists());
        assert!(!dir.join("stats.csv.part").exists());

        let _ = std::fs::remove_dir_all(base);
        Ok(())
    }

    #[test]
    fn diagnostic_recorder_async_stop_finalizes_files() -> Result<()> {
        let base = std::env::temp_dir().join(format!(
            "echoless-diagnostic-stop-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cfg = DiagnosticsConfig {
            record_dir: Some(base.to_string_lossy().to_string()),
            max_seconds: None,
        };
        let node_stats = [ProcessorStats::empty("test")];
        let mut recorder =
            DiagnosticRecorder::new(&cfg, 48_000, 1, 10, 0, 50, &node_stats, false)?.unwrap();
        let dir = recorder.dir.clone();
        let status = recorder.status_handle();
        let near = [0.25f32, -0.25];
        let far = [0.1f32, -0.1];
        let out = [0.0f32, 0.1];
        let sample = StatsSample {
            algorithmic_latency_ms: 16.0,
            near_delay_ms: 0,
            near_delay_buffered_samples: 0,
            frame_size: near.len(),
            near: &near,
            far: &far,
            out: &out,
            mic_q: 0,
            ref_q: 0,
            out_q: 0,
            mic_input_drops: 0,
            ref_input_drops: 0,
            stale_drops: 0,
            ref_underruns: 0,
            output_overruns: 0,
            output_underruns: 0,
            node_stats: &node_stats,
        };

        assert!(recorder.write_frame(&sample)?);
        recorder.request_finish(DiagnosticDoneReason::Stopped);
        assert!(!status.is_recording());
        for _ in 0..100 {
            if dir.join("stats.csv").exists() && dir.join("mic.wav").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert!(dir.join("mic.wav").exists());
        assert!(dir.join("ref.wav").exists());
        assert!(dir.join("out.wav").exists());
        assert!(dir.join("stats.csv").exists());

        let _ = std::fs::remove_dir_all(base);
        Ok(())
    }

    #[test]
    fn runtime_control_command_parses_frontend_json() {
        let start = parse_runtime_control_command(
            r#"{"cmd":"start_diagnostics","record_dir":"/tmp/diag","max_seconds":10}"#,
        )
        .unwrap();
        match start {
            RuntimeControlCommand::StartDiagnostics {
                record_dir,
                max_seconds,
            } => {
                assert_eq!(record_dir, "/tmp/diag");
                assert_eq!(max_seconds, Some(10));
            }
            other => panic!("expected start_diagnostics, got {other:?}"),
        }

        let stop = parse_runtime_control_command(r#"{"cmd":"stop_diagnostics"}"#).unwrap();
        assert!(matches!(stop, RuntimeControlCommand::StopDiagnostics));

        let set_level =
            parse_runtime_control_command(r#"{"cmd":"set_output_level","level":75}"#).unwrap();
        assert!(matches!(
            set_level,
            RuntimeControlCommand::SetOutputLevel(75)
        ));

        let err =
            parse_runtime_control_command(r#"{"cmd":"set_output_level","level":101}"#).unwrap_err();
        assert!(err.to_string().contains("<= 100"));
    }

    #[test]
    fn user_latency_estimate_includes_half_frame_near_delay_algorithm_and_output_queue() {
        let latency = estimate_user_latency_ms(10, 25, 16.0, 2400, 48_000);

        assert_eq!(latency, 96.0);
    }

    #[test]
    fn peak_waveform_returns_fixed_peak_buckets() {
        let wave = peak_waveform(&[0.0, -0.5, 0.25, 1.5], 2);

        assert_eq!(wave, vec![0.5, 1.0]);
        assert_eq!(peak_waveform(&[], 3), vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn virtual_audio_name_detection_covers_common_drivers() {
        assert!(is_virtual_audio_name(
            "CABLE Input (VB-Audio Virtual Cable)"
        ));
        assert!(is_virtual_audio_name("BlackHole 2ch"));
        assert!(is_virtual_audio_name("Virtual Desktop Mic"));
        assert!(!is_virtual_audio_name("MacBook Pro Speakers"));
    }

    #[test]
    fn system_audio_permission_state_uses_frontend_contract_values() {
        assert!(matches!(
            system_audio_permission_state(),
            "granted" | "denied" | "undetermined" | "unknown"
        ));
    }

    #[test]
    fn runtime_status_json_exposes_frontend_latency_fields() {
        let mut stats = RealtimeStats::new(
            Duration::from_millis(1000),
            48_000,
            10,
            25,
            75,
            "localvqe".into(),
            16.0,
            true,
            Some("diagnostics/session-1".into()),
            None,
        );
        stats.total_frames = 480;
        stats.near_samples = 480;
        stats.far_samples = 480;
        stats.out_samples = 480;
        stats.near_sq = 120.0;
        stats.far_sq = 30.0;
        stats.out_sq = 100.0;
        stats.out_q = 2400;
        stats.mic_input_drops = 1;
        stats.ref_input_drops = 2;
        stats.aec_estimated_delay_ms = 48;

        let value = stats.status_value(stats.started + Duration::from_secs(1));

        assert_eq!(value["type"], "status");
        assert_eq!(value["backend"], "localvqe");
        assert_eq!(value["input_drops"], 3);
        assert_eq!(value["near_delay_ms"], 25);
        assert_eq!(value["output_level"], 75);
        assert_eq!(value["output_gain_db"], output_level_gain_db(75).unwrap());
        assert_eq!(value["output_queue_latency_ms"], 50.0);
        assert_eq!(value["estimated_user_latency_ms"], 96.0);
        assert_eq!(value["aec_estimated_delay_ms"], 48);
        assert_eq!(value["diagnostics_session_dir"], "diagnostics/session-1");
        assert_eq!(value["recording"], false);
        assert_eq!(value["diagnostics_frames"], 0);
        assert_eq!(value["diagnostics_drops"], 0);
        assert_eq!(
            value["mic_wave"].as_array().unwrap().len(),
            STATUS_WAVE_BUCKETS
        );
        assert_eq!(
            value["ref_wave"].as_array().unwrap().len(),
            STATUS_WAVE_BUCKETS
        );
        assert_eq!(
            value["out_wave"].as_array().unwrap().len(),
            STATUS_WAVE_BUCKETS
        );

        stats.set_output_level(0);
        let value = stats.status_value(stats.started + Duration::from_secs(2));
        assert_eq!(value["output_level"], 0);
        assert!(value["output_gain_db"].is_null());
    }
}
