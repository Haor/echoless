//! cpal 实时管线。结构移植自上游 aec3-aec(BSD-3),处理换成 echoless 的 `ProcessorChain`。
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

mod control;
mod devices;
mod diagnostics;
#[cfg(target_os = "macos")]
mod macos_process_tap;
mod resample;
mod stats;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
#[cfg(any(windows, target_os = "linux"))]
use std::time::Instant;

use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, FromSample, Sample, SampleFormat, SizedSample, Stream};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;
use serde_json::json;

use self::control::{
    delay_ms_to_samples, handle_runtime_controls, spawn_control_reader, RuntimeControlContext,
    RuntimeControlEvent, SUPPORTED_RUNTIME_CONTROLS,
};
pub use self::devices::{
    audio_doctor_json_with_options, devices_json, devices_json_with_options, print_devices,
    AudioDoctorOptions, DeviceListOptions,
};
use self::devices::{
    config_choice_summary, mic_selector, output_selector, pick_config, select_device,
    select_reference_source, selected_device_label, DeviceKind, ReferenceSource,
    StreamConfigChoice,
};
#[cfg(test)]
use self::devices::{
    format_device_description, is_virtual_audio_name, system_audio_permission_state,
};
use self::diagnostics::{DiagnosticRecorder, DiagnosticRecorderConfig, DiagnosticsStatusHandle};
use self::resample::{InterleavedLinearResampler, OutputLinearResampler};
use self::stats::{RealtimeStats, RealtimeStatsConfig, StatsSample};
use echoless_core::{
    apply_output_level, apply_reference_channels_to_chain, output_level_gain_db, PipelineConfig,
    ReferenceChannels, MAX_NEAR_DELAY_MS, MAX_OUTPUT_LEVEL,
};
use echoless_processors::{chain_from_nodes, ProcessorChain};

const BYPASS_KEEP_WARM: bool = true;
const BYPASS_CROSSFADE_MS: u32 = 15;

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
    bypassed: bool,
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
struct BypassCrossfade {
    total_samples: usize,
    position: usize,
    from_bypassed: bool,
    to_bypassed: bool,
}

impl BypassCrossfade {
    fn new(total_samples: usize) -> Self {
        Self {
            total_samples,
            position: total_samples,
            from_bypassed: false,
            to_bypassed: false,
        }
    }

    fn start(&mut self, from_bypassed: bool, to_bypassed: bool) {
        if from_bypassed == to_bypassed || self.total_samples == 0 {
            self.position = self.total_samples;
            self.from_bypassed = to_bypassed;
            self.to_bypassed = to_bypassed;
            return;
        }
        self.position = 0;
        self.from_bypassed = from_bypassed;
        self.to_bypassed = to_bypassed;
    }

    fn is_active(&self) -> bool {
        self.position < self.total_samples
    }

    fn target_bypassed(&self) -> Option<bool> {
        self.is_active().then_some(self.to_bypassed)
    }

    fn next_sample(&mut self) -> Option<(bool, bool, f32)> {
        if !self.is_active() {
            return None;
        }
        self.position += 1;
        let alpha = (self.position as f32 / self.total_samples as f32).min(1.0);
        let sample = (self.from_bypassed, self.to_bypassed, alpha);
        if self.position >= self.total_samples {
            self.position = self.total_samples;
        }
        Some(sample)
    }
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
                                                          // A5:tap 采样率由 helper 流头上报,与管线不一致时 reader 侧线性重采样,
                                                          // 不再要求管线锁 48k。

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
                "Ref:    macOS Process Tap system audio (device rate auto-resampled to {} Hz, {}ch)",
                sample_rate, reference_channels
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
                sample_rate,
                p,
                counters.ref_input_drops.clone(),
                running.clone(),
            )?)
        }
        _ => None,
    };

    // 处理线程:只碰 ring(Send),cpal Stream 留在本线程(!Send)。
    let proc_running = running.clone();
    let mut chain = chain_from_nodes(&chain_cfg, sample_rate, reference_channels as u16)?;
    // 预热边界 SRC:跑一帧静音让 rubato resampler 按 frame_size 建立,
    // 使 total_latency_ms() 计入节点边界重采样延迟(warm_up 内部会 reset 清除预热影响)。
    chain.warm_up(frame_size);
    let algorithmic_latency_ms = chain.total_latency_ms();
    let initial_node_stats = chain.stats();
    let stats_interval = options.stats_interval_ms.map(Duration::from_millis);
    let diagnostic = DiagnosticRecorder::new(DiagnosticRecorderConfig {
        cfg: &cfg.diagnostics,
        sample_rate,
        reference_channels: reference_channels as u16,
        frame_ms: cfg.frame_ms,
        near_delay_ms: cfg.near_delay_ms,
        output_level: cfg.output_level,
        node_stats: &initial_node_stats,
        status_json: options.status_json,
    })?;
    let diagnostics_session_dir = diagnostic
        .as_ref()
        .map(DiagnosticRecorder::session_dir_string);
    let diagnostics_status = diagnostic.as_ref().map(DiagnosticRecorder::status_handle);
    let control = options.status_json.then(spawn_control_reader);
    let started_event = json!({
        "type": "started",
        "cli_version": env!("CARGO_PKG_VERSION"),
        "supported_controls": SUPPORTED_RUNTIME_CONTROLS,
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
        bypassed: cfg.bypass,
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
    let mut processed = vec![0.0f32; frame_size];
    let mut out = vec![0.0f32; frame_size];
    let mut near_delay = VecDeque::from(vec![0.0f32; runtime.near_delay_samples]);
    let mut output_bypassed = runtime.bypassed;
    let mut bypass_crossfade = BypassCrossfade::new(bypass_crossfade_samples(runtime.sample_rate));
    let mut stats = runtime.stats_interval.map(|interval| {
        RealtimeStats::new(RealtimeStatsConfig {
            interval,
            sample_rate: runtime.sample_rate,
            frame_ms: runtime.frame_ms,
            near_delay_ms: runtime.near_delay_ms,
            output_level: runtime.output_level,
            bypassed: runtime.bypassed,
            backend: runtime.backend.clone(),
            algorithmic_latency_ms: runtime.algorithmic_latency_ms,
            status_json: runtime.status_json,
            diagnostics_session_dir: runtime.diagnostics_session_dir.clone(),
            diagnostics_status: runtime.diagnostics_status.clone(),
        })
    });
    let mut diagnostic = runtime.diagnostic;
    let mut control = runtime.control;

    while running.load(Ordering::SeqCst) {
        handle_runtime_controls(
            &mut control,
            RuntimeControlContext {
                diagnostic: &mut diagnostic,
                stats: stats.as_mut(),
                chain: &mut chain,
                sample_rate: runtime.sample_rate,
                reference_channels: runtime.reference_channels as u16,
                frame_ms: runtime.frame_ms,
                near_delay_ms: &mut runtime.near_delay_ms,
                near_delay_samples: &mut runtime.near_delay_samples,
                near_delay_buffer: &mut near_delay,
                output_level: &mut runtime.output_level,
                bypassed: &mut runtime.bypassed,
                status_json: runtime.status_json,
            },
        );
        let current_bypass_target = bypass_crossfade
            .target_bypassed()
            .unwrap_or(output_bypassed);
        if runtime.bypassed != current_bypass_target {
            bypass_crossfade.start(current_bypass_target, runtime.bypassed);
            output_bypassed = runtime.bypassed;
        }

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

        process_bypass_frame(
            &mut chain,
            BypassFrameInputs {
                near: &near,
                far: &far,
                raw_near: &mic_frame,
            },
            BypassFrameOutputs {
                processed: &mut processed,
                out: &mut out,
            },
            BypassFrameConfig {
                bypassed: runtime.bypassed,
                keep_warm: BYPASS_KEEP_WARM,
            },
            &mut bypass_crossfade,
        );
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

#[derive(Clone, Copy, Debug)]
struct BypassFrameConfig {
    bypassed: bool,
    keep_warm: bool,
}

struct BypassFrameInputs<'a> {
    near: &'a [f32],
    far: &'a [f32],
    raw_near: &'a [f32],
}

struct BypassFrameOutputs<'a> {
    processed: &'a mut [f32],
    out: &'a mut [f32],
}

fn bypass_crossfade_samples(sample_rate: u32) -> usize {
    ((u64::from(sample_rate) * u64::from(BYPASS_CROSSFADE_MS) + 500) / 1000).max(1) as usize
}

fn process_bypass_frame(
    chain: &mut ProcessorChain,
    inputs: BypassFrameInputs<'_>,
    outputs: BypassFrameOutputs<'_>,
    config: BypassFrameConfig,
    crossfade: &mut BypassCrossfade,
) {
    if should_process_for_bypass(config.bypassed, config.keep_warm, crossfade.is_active()) {
        chain.process(
            inputs.near,
            inputs.far,
            outputs.processed,
            outputs.out.len() as u32,
        );
    }
    write_bypass_output(
        outputs.processed,
        inputs.raw_near,
        outputs.out,
        config.bypassed,
        crossfade,
    );
}

fn should_process_for_bypass(bypassed: bool, keep_warm: bool, crossfade_active: bool) -> bool {
    !bypassed || keep_warm || crossfade_active
}

fn write_bypass_output(
    processed: &[f32],
    raw_near: &[f32],
    out: &mut [f32],
    bypassed: bool,
    crossfade: &mut BypassCrossfade,
) {
    for (index, sample) in out.iter_mut().enumerate() {
        if let Some((from_bypassed, to_bypassed, alpha)) = crossfade.next_sample() {
            let from = bypass_source_sample(processed, raw_near, index, from_bypassed);
            let to = bypass_source_sample(processed, raw_near, index, to_bypassed);
            *sample = from + (to - from) * alpha;
        } else {
            *sample = bypass_source_sample(processed, raw_near, index, bypassed);
        }
    }
}

fn bypass_source_sample(processed: &[f32], raw_near: &[f32], index: usize, bypassed: bool) -> f32 {
    if bypassed {
        raw_near.get(index).copied().unwrap_or(0.0)
    } else {
        processed.get(index).copied().unwrap_or(0.0)
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

#[cfg(any(windows, target_os = "linux"))]
pub(crate) fn play_mono_samples_to_output(
    selector: Option<&str>,
    sample_rate: u32,
    samples: Vec<f32>,
) -> Result<()> {
    if samples.is_empty() {
        return Ok(());
    }

    let host = cpal::default_host();
    let selected = select_device(&host, DeviceKind::Output, selector)?;
    let choice = pick_config(&selected.device, DeviceKind::Output, sample_rate)?;
    let done = Arc::new(AtomicBool::new(false));
    let duration = Duration::from_secs_f64(samples.len() as f64 / f64::from(sample_rate));
    let samples = Arc::new(samples);
    let stream = match choice.sample_format() {
        SampleFormat::I16 => build_mono_sample_player_stream::<i16>(
            &selected.device,
            &choice,
            sample_rate,
            Arc::clone(&samples),
            Arc::clone(&done),
        ),
        SampleFormat::I32 => build_mono_sample_player_stream::<i32>(
            &selected.device,
            &choice,
            sample_rate,
            Arc::clone(&samples),
            Arc::clone(&done),
        ),
        SampleFormat::F32 => build_mono_sample_player_stream::<f32>(
            &selected.device,
            &choice,
            sample_rate,
            Arc::clone(&samples),
            Arc::clone(&done),
        ),
        SampleFormat::U16 => build_mono_sample_player_stream::<u16>(
            &selected.device,
            &choice,
            sample_rate,
            Arc::clone(&samples),
            Arc::clone(&done),
        ),
        other => bail!("不支持的采样格式 {other}"),
    }?;
    stream.play().context("启动蜂鸣输出流失败")?;

    let deadline = Instant::now() + duration + Duration::from_secs(2);
    while !done.load(Ordering::Relaxed) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if !done.load(Ordering::Relaxed) {
        bail!("蜂鸣播放超时:输出设备没有及时消耗测试音");
    }
    Ok(())
}

#[cfg(any(windows, target_os = "linux"))]
fn build_mono_sample_player_stream<T>(
    device: &Device,
    choice: &StreamConfigChoice,
    source_sample_rate: u32,
    samples: Arc<Vec<f32>>,
    done: Arc<AtomicBool>,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32> + Copy + Send + 'static,
{
    let config = choice.config();
    let channels = usize::from(config.channels);
    let stream_sample_rate = choice.stream_sample_rate();
    let mut stream_frame = 0u64;
    device
        .build_output_stream(
            &config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels) {
                    let source_pos = stream_frame as f64 * f64::from(source_sample_rate)
                        / f64::from(stream_sample_rate);
                    let sample = if source_pos < samples.len() as f64 {
                        interpolated_sample(&samples, source_pos).clamp(-1.0, 1.0)
                    } else {
                        done.store(true, Ordering::Relaxed);
                        0.0
                    };
                    stream_frame += 1;
                    let out_sample = T::from_sample(sample);
                    for out in frame {
                        *out = out_sample;
                    }
                }
            },
            |err| eprintln!("蜂鸣输出流错误: {err}"),
            None,
        )
        .context("构建蜂鸣输出流失败")
}

#[cfg(any(windows, target_os = "linux"))]
fn interpolated_sample(samples: &[f32], position: f64) -> f32 {
    let i = position.floor() as usize;
    let frac = (position - i as f64) as f32;
    let a = samples.get(i).copied().unwrap_or(0.0);
    let b = samples.get(i + 1).copied().unwrap_or(a);
    a + (b - a) * frac
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::sync::atomic::AtomicUsize;

    use cpal::{DeviceDescriptionBuilder, DeviceType, InterfaceType};
    use echoless_processors::{EchoProcessor, IoSpec, ProcessorStats};

    struct CountingAllocator;

    static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

    thread_local! {
        static COUNT_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    }

    #[global_allocator]
    static GLOBAL: CountingAllocator = CountingAllocator;

    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            count_allocation();
            System.alloc(layout)
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            count_allocation();
            System.alloc_zeroed(layout)
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            count_allocation();
            System.realloc(ptr, layout, new_size)
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            System.dealloc(ptr, layout)
        }
    }

    fn count_allocation() {
        COUNT_ALLOCATIONS.with(|enabled| {
            if enabled.get() {
                ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
            }
        });
    }

    fn allocation_count_during(f: impl FnOnce()) -> usize {
        ALLOCATIONS.store(0, Ordering::SeqCst);
        COUNT_ALLOCATIONS.with(|enabled| enabled.set(true));
        f();
        COUNT_ALLOCATIONS.with(|enabled| enabled.set(false));
        ALLOCATIONS.load(Ordering::SeqCst)
    }

    struct InvertingProcessor;

    impl EchoProcessor for InvertingProcessor {
        fn name(&self) -> &'static str {
            "invert"
        }

        fn io_spec(&self) -> IoSpec {
            IoSpec {
                sample_rate: 48_000,
                near_channels: 1,
                far_channels: 1,
                algorithmic_latency_ms: 0.0,
            }
        }

        fn configure(&mut self, _params: &toml::Table) -> anyhow::Result<()> {
            Ok(())
        }

        fn process(&mut self, near: &[f32], _far: &[f32], out: &mut [f32], _frames: u32) {
            for (input, output) in near.iter().copied().zip(out.iter_mut()) {
                *output = -input;
            }
        }

        fn stats(&self) -> ProcessorStats {
            ProcessorStats::empty("invert")
        }

        fn reset(&mut self) {}
    }

    struct AdaptiveEchoSuppressor {
        estimate: f32,
        adaptation_rate: f32,
    }

    impl EchoProcessor for AdaptiveEchoSuppressor {
        fn name(&self) -> &'static str {
            "adaptive_echo_suppressor"
        }

        fn io_spec(&self) -> IoSpec {
            IoSpec {
                sample_rate: 48_000,
                near_channels: 1,
                far_channels: 1,
                algorithmic_latency_ms: 0.0,
            }
        }

        fn configure(&mut self, _params: &toml::Table) -> anyhow::Result<()> {
            Ok(())
        }

        fn process(&mut self, near: &[f32], far: &[f32], out: &mut [f32], _frames: u32) {
            let mut far_sq = 0.0f32;
            let mut near_far = 0.0f32;
            for (near_sample, far_sample) in near.iter().copied().zip(far.iter().copied()) {
                far_sq += far_sample * far_sample;
                near_far += near_sample * far_sample;
            }
            if far_sq > 1.0e-9 {
                let observed_gain = near_far / far_sq;
                self.estimate += self.adaptation_rate * (observed_gain - self.estimate);
            }
            for ((near_sample, far_sample), output) in near
                .iter()
                .copied()
                .zip(far.iter().copied())
                .zip(out.iter_mut())
            {
                *output = near_sample - self.estimate * far_sample;
            }
        }

        fn stats(&self) -> ProcessorStats {
            ProcessorStats::empty("adaptive_echo_suppressor")
        }

        fn reset(&mut self) {}
    }

    #[test]
    fn bypass_outputs_raw_near_without_near_delay_and_keeps_output_level() {
        let mut chain = ProcessorChain::new(48_000, 1);
        chain.push(Box::new(InvertingProcessor));
        let near_after_delay = [0.9, -0.9, 0.5, -0.5];
        let raw_near = [0.1, -0.2, 0.25, -0.25];
        let far = [0.0; 4];
        let mut processed = [0.0; 4];
        let mut out = [0.0; 4];
        let mut crossfade = BypassCrossfade::new(4);

        process_bypass_frame(
            &mut chain,
            BypassFrameInputs {
                near: &near_after_delay,
                far: &far,
                raw_near: &raw_near,
            },
            BypassFrameOutputs {
                processed: &mut processed,
                out: &mut out,
            },
            BypassFrameConfig {
                bypassed: true,
                keep_warm: true,
            },
            &mut crossfade,
        );
        apply_output_level(&mut out, MAX_OUTPUT_LEVEL);

        assert_eq!(processed, [-0.9, 0.9, -0.5, 0.5]);
        approx_slice(&out, &[0.3, -0.6, 0.75, -0.75], 0.001);
    }

    #[test]
    fn keep_warm_adapts_processor_during_bypass_before_restore() {
        let warm = restore_rms_after_bypass(true);
        let cold = restore_rms_after_bypass(false);

        assert!(
            warm < 0.03,
            "keep-warm restore residual too high: {warm:.4}"
        );
        assert!(
            cold > warm * 5.0,
            "non-warm path should show a clear reconvergence gap: warm={warm:.4} cold={cold:.4}"
        );
    }

    #[test]
    fn bypass_crossfade_is_linear_across_adjacent_buffers() {
        let processed = [0.0f32; 8];
        let raw = [1.0f32; 8];
        let mut out_a = [0.0f32; 4];
        let mut out_b = [0.0f32; 4];
        let mut crossfade = BypassCrossfade::new(8);
        crossfade.start(false, true);

        write_bypass_output(&processed, &raw, &mut out_a, true, &mut crossfade);
        write_bypass_output(&processed, &raw, &mut out_b, true, &mut crossfade);

        approx_eq(out_a[0], 0.125, 0.001);
        approx_eq(out_a[3], 0.5, 0.001);
        approx_eq(out_b[3], 1.0, 0.001);
        let mut previous = 0.0;
        for sample in out_a.into_iter().chain(out_b) {
            assert!(
                (sample - previous).abs() <= 0.126,
                "crossfade adjacent step too large: previous={previous} sample={sample}"
            );
            previous = sample;
        }
    }

    #[test]
    fn bypass_selection_and_crossfade_allocate_no_heap() {
        let near = [0.1f32; 16];
        let far = [0.0f32; 16];
        let raw = [0.2f32; 16];
        let mut processed = [0.0f32; 16];
        let mut out = [0.0f32; 16];
        let mut chain = ProcessorChain::new(48_000, 1);
        let mut inactive_crossfade = BypassCrossfade::new(16);
        let mut active_crossfade = BypassCrossfade::new(16);
        active_crossfade.start(false, true);

        let allocations = allocation_count_during(|| {
            process_bypass_frame(
                &mut chain,
                BypassFrameInputs {
                    near: &near,
                    far: &far,
                    raw_near: &raw,
                },
                BypassFrameOutputs {
                    processed: &mut processed,
                    out: &mut out,
                },
                BypassFrameConfig {
                    bypassed: true,
                    keep_warm: false,
                },
                &mut inactive_crossfade,
            );
            process_bypass_frame(
                &mut chain,
                BypassFrameInputs {
                    near: &near,
                    far: &far,
                    raw_near: &raw,
                },
                BypassFrameOutputs {
                    processed: &mut processed,
                    out: &mut out,
                },
                BypassFrameConfig {
                    bypassed: true,
                    keep_warm: false,
                },
                &mut active_crossfade,
            );
            apply_output_level(&mut out, MAX_OUTPUT_LEVEL);
        });

        assert_eq!(allocations, 0);
    }

    fn restore_rms_after_bypass(keep_warm: bool) -> f32 {
        const FRAME: usize = 480;
        let mut chain = ProcessorChain::new(48_000, 1);
        chain.push(Box::new(AdaptiveEchoSuppressor {
            estimate: 0.0,
            adaptation_rate: 0.25,
        }));
        let mut near = [0.0f32; FRAME];
        let mut far = [0.0f32; FRAME];
        let mut processed = [0.0f32; FRAME];
        let mut out = [0.0f32; FRAME];
        let mut crossfade = BypassCrossfade::new(FRAME);
        let mut frame_index = 0;

        for _ in 0..80 {
            fill_echo_frame(frame_index, 0.3, &mut far, &mut near);
            process_bypass_frame(
                &mut chain,
                BypassFrameInputs {
                    near: &near,
                    far: &far,
                    raw_near: &near,
                },
                BypassFrameOutputs {
                    processed: &mut processed,
                    out: &mut out,
                },
                BypassFrameConfig {
                    bypassed: false,
                    keep_warm,
                },
                &mut crossfade,
            );
            frame_index += 1;
        }

        crossfade.start(false, true);
        for _ in 0..300 {
            fill_echo_frame(frame_index, 0.8, &mut far, &mut near);
            process_bypass_frame(
                &mut chain,
                BypassFrameInputs {
                    near: &near,
                    far: &far,
                    raw_near: &near,
                },
                BypassFrameOutputs {
                    processed: &mut processed,
                    out: &mut out,
                },
                BypassFrameConfig {
                    bypassed: true,
                    keep_warm,
                },
                &mut crossfade,
            );
            frame_index += 1;
        }

        crossfade.start(true, false);
        fill_echo_frame(frame_index, 0.8, &mut far, &mut near);
        process_bypass_frame(
            &mut chain,
            BypassFrameInputs {
                near: &near,
                far: &far,
                raw_near: &near,
            },
            BypassFrameOutputs {
                processed: &mut processed,
                out: &mut out,
            },
            BypassFrameConfig {
                bypassed: false,
                keep_warm,
            },
            &mut crossfade,
        );
        frame_index += 1;

        fill_echo_frame(frame_index, 0.8, &mut far, &mut near);
        process_bypass_frame(
            &mut chain,
            BypassFrameInputs {
                near: &near,
                far: &far,
                raw_near: &near,
            },
            BypassFrameOutputs {
                processed: &mut processed,
                out: &mut out,
            },
            BypassFrameConfig {
                bypassed: false,
                keep_warm,
            },
            &mut crossfade,
        );

        rms(&out)
    }

    fn fill_echo_frame(frame_index: usize, gain: f32, far: &mut [f32], near: &mut [f32]) {
        let start = frame_index * far.len();
        for (index, (far_sample, near_sample)) in far.iter_mut().zip(near.iter_mut()).enumerate() {
            let n = start + index;
            let phase = n as f32 * 440.0 * std::f32::consts::TAU / 48_000.0;
            *far_sample = 0.5 * phase.sin();
            *near_sample = gain * *far_sample;
        }
    }

    fn rms(samples: &[f32]) -> f32 {
        let sum = samples.iter().map(|sample| sample * sample).sum::<f32>();
        (sum / samples.len().max(1) as f32).sqrt()
    }

    fn approx_slice(actual: &[f32], expected: &[f32], epsilon: f32) {
        assert_eq!(actual.len(), expected.len());
        for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
            assert!(
                (actual - expected).abs() <= epsilon,
                "sample {index}: actual={actual} expected={expected}"
            );
        }
    }

    fn approx_eq(actual: f32, expected: f32, epsilon: f32) {
        assert!(
            (actual - expected).abs() <= epsilon,
            "actual={actual} expected={expected}"
        );
    }

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
}
