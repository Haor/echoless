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
#[cfg(windows)]
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
    config_choice_summary, is_macos_process_tap, macos_process_tap_sample_rate, mic_selector,
    output_selector, pick_config, select_device, select_reference_source, selected_device_label,
    DeviceKind, ReferenceSource, StreamConfigChoice,
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
    let mut out = vec![0.0f32; frame_size];
    let mut near_delay = VecDeque::from(vec![0.0f32; runtime.near_delay_samples]);
    let mut stats = runtime.stats_interval.map(|interval| {
        RealtimeStats::new(RealtimeStatsConfig {
            interval,
            sample_rate: runtime.sample_rate,
            frame_ms: runtime.frame_ms,
            near_delay_ms: runtime.near_delay_ms,
            output_level: runtime.output_level,
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
                status_json: runtime.status_json,
            },
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

#[cfg(windows)]
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

#[cfg(windows)]
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

#[cfg(windows)]
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
