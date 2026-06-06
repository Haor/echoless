//! cpal 实时管线。结构移植自上游 sonora-aec(BSD-3),处理换成 echoless 的 `ProcessorChain`。
//!
//! 三股 cpal 流 + 三个 ringbuf + 一个独立处理线程:
//! ```text
//! mic 设备 ──► mic_ring ──┐
//!                         ├─► 处理线程(每 10ms)─ chain.process(near, far) ─► out_ring ──► 输出设备
//! 系统 loopback ─► render_ring┘
//! ```
//! 全程 mono、同采样率 → 链上零重采样(rubato 仅 LocalVQE 进来才需要)。
//! 跨平台靠 cpal:Windows WASAPI(含 output loopback)/ macOS CoreAudio。
//! 系统声音参考 = output 设备做 loopback(Windows 原生;macOS 需 BlackHole 之类)。
//! 虚拟麦输出 = 选 VB-Cable / BlackHole 作 output 设备。

use std::fs::{create_dir, create_dir_all, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
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
    apply_reference_channels_to_chain, DiagnosticsConfig, PipelineConfig, ReferenceChannels,
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
    backend: String,
    algorithmic_latency_ms: f32,
    counters: RealtimeCounters,
    stats_interval: Option<Duration>,
    status_json: bool,
    diagnostics_session_dir: Option<String>,
    diagnostic: Option<DiagnosticRecorder>,
}

pub fn run_with_options(cfg: &PipelineConfig, options: RuntimeOptions) -> Result<()> {
    let host = cpal::default_host();

    let mic_device = select_device(&host, DeviceKind::Input, mic_selector(&cfg.mic))
        .context("选择麦克风设备失败")?;
    let output_device = select_device(&host, DeviceKind::Output, output_selector(&cfg.output))
        .context("选择输出设备失败")?;
    // reference:"none" = 无参考(纯 NS);"system" = 默认输出做 loopback;否则按名。
    let render_device = match cfg.reference.as_str() {
        "none" | "" => None,
        "system" | "default" => Some((
            select_default_device(&host, DeviceKind::Output)
                .context("无默认输出设备可作系统 loopback")?,
            DeviceKind::Output,
        )),
        sel => Some(select_render_device(&host, sel).context("选择参考设备失败")?),
    };

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
    let ring_size = frame_size * 12; // ~120ms
    let reference_channels = if render_device.is_some() {
        usize::from(cfg.reference_channels.channel_count())
    } else {
        1
    };

    let mic_config = pick_config(&mic_device.device, DeviceKind::Input, sample_rate)
        .context("麦克风不支持该采样率")?;
    let output_config = pick_config(&output_device.device, DeviceKind::Output, sample_rate)
        .context("输出设备不支持该采样率")?;
    let render_config = match &render_device {
        Some((d, k)) => {
            Some(pick_config(&d.device, *k, sample_rate).context("参考设备不支持该采样率")?)
        }
        None => None,
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
            config_summary(&mic_config)
        ),
    );
    match (&render_device, &render_config) {
        (Some((d, k)), Some(c)) => print_human(
            options.status_json,
            format!(
                "Ref:    {} {} ({})",
                k.label(),
                selected_device_label(d),
                config_summary(c)
            ),
        ),
        _ => print_human(
            options.status_json,
            "Ref:    无 —— AEC 缺少参考,仅 NS 等单端处理有效",
        ),
    }
    print_human(
        options.status_json,
        format!(
            "Output: {} ({})",
            selected_device_label(&output_device),
            config_summary(&output_config)
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
            "采样率 {sample_rate} Hz · 帧 {} ms / {frame_size} 样本 · reference={} · 链: {chain_desc}",
            cfg.frame_ms,
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
    let (render_prod, render_cons) = if render_device.is_some() {
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
    let render_stream = match (render_device.as_ref(), render_config.as_ref(), render_prod) {
        (Some((d, _)), Some(c), Some(p)) => Some(build_input_stream(
            &d.device,
            c,
            p,
            "ref",
            InputChannelMode::from_reference_channels(cfg.reference_channels),
            counters.ref_input_drops.clone(),
        )?),
        _ => None,
    };
    let output_stream = build_output_stream(
        &output_device.device,
        &output_config,
        out_cons,
        counters.output_underruns.clone(),
    )?;

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
        &initial_node_stats,
        options.status_json,
    )?;
    let diagnostics_session_dir = diagnostic
        .as_ref()
        .map(|recorder| recorder.dir.display().to_string());
    let runtime = ProcessRuntime {
        sample_rate,
        frame_ms: cfg.frame_ms,
        frame_size,
        reference_channels,
        backend,
        algorithmic_latency_ms,
        counters,
        stats_interval,
        status_json: options.status_json,
        diagnostics_session_dir,
        diagnostic,
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

    print_human(
        options.status_json,
        "运行中。macOS 首次需授予麦克风权限。Ctrl+C 停止。",
    );
    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
    }

    drop(mic_stream);
    drop(render_stream);
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
    runtime: ProcessRuntime,
) where
    M: Consumer<Item = f32>,
    R: Consumer<Item = f32>,
    O: Producer<Item = f32>,
{
    let frame_size = runtime.frame_size;
    let far_samples_per_frame = frame_size * runtime.reference_channels;
    let mut near = vec![0.0f32; frame_size];
    let mut far = vec![0.0f32; far_samples_per_frame];
    let mut out = vec![0.0f32; frame_size];
    let mut stats = runtime.stats_interval.map(|interval| {
        RealtimeStats::new(
            interval,
            runtime.sample_rate,
            runtime.frame_ms,
            runtime.backend.clone(),
            runtime.algorithmic_latency_ms,
            runtime.status_json,
            runtime.diagnostics_session_dir.clone(),
        )
    });
    let mut diagnostic = runtime.diagnostic;

    while running.load(Ordering::SeqCst) {
        if mic_cons.occupied_len() < frame_size {
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        // 控制积压(简单 drift/堆积处理):超 4 帧丢旧的。
        let mut stale_drops = skip_stale(&mut mic_cons, frame_size);
        mic_cons.pop_slice(&mut near);

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
        let node_stats = chain.stats();
        let pushed = out_prod.push_slice(&out);
        let output_overruns = out.len().saturating_sub(pushed) as u64;
        let ref_q = render_cons
            .as_ref()
            .map(|rc| rc.occupied_len())
            .unwrap_or(0);
        let sample = StatsSample {
            sample_rate: runtime.sample_rate,
            frame_ms: runtime.frame_ms,
            algorithmic_latency_ms: runtime.algorithmic_latency_ms,
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
                Ok(false) => diagnostic = None,
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

struct StatsSample<'a> {
    sample_rate: u32,
    frame_ms: u32,
    algorithmic_latency_ms: f32,
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

struct DiagnosticRecorder {
    dir: PathBuf,
    mic: Option<WavWriter<BufWriter<File>>>,
    reference: Option<WavWriter<BufWriter<File>>>,
    out: Option<WavWriter<BufWriter<File>>>,
    stats: BufWriter<File>,
    max_frames: Option<u64>,
    written_frames: u64,
    frame_index: u64,
    human_to_stderr: bool,
}

impl DiagnosticRecorder {
    fn new(
        cfg: &DiagnosticsConfig,
        sample_rate: u32,
        reference_channels: u16,
        frame_ms: u32,
        node_stats: &[ProcessorStats],
        human_to_stderr: bool,
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
            reference_channels,
            max_frames,
            node_stats,
        )?;
        let mut stats = BufWriter::new(
            File::create(dir.join("stats.csv"))
                .with_context(|| format!("创建诊断 stats.csv 失败: {}", dir.display()))?,
        );
        writeln!(
            stats,
            "frame_index,frames,mic_dbfs,ref_dbfs,out_dbfs,mic_q,ref_q,out_q,output_queue_latency_ms,estimated_user_latency_ms,aec_estimated_delay_ms,mic_input_drops,ref_input_drops,input_drops,stale_drops,ref_underruns,output_overruns,output_underruns,node_process_time_ms,node_runtime_errors,node_diverged,node_last_error"
        )?;

        print_human(human_to_stderr, format!("诊断录制目录: {}", dir.display()));
        Ok(Some(Self {
            mic: Some(WavWriter::create(dir.join("mic.wav"), spec)?),
            reference: Some(WavWriter::create(dir.join("ref.wav"), ref_spec)?),
            out: Some(WavWriter::create(dir.join("out.wav"), spec)?),
            stats,
            dir,
            max_frames,
            written_frames: 0,
            frame_index: 0,
            human_to_stderr,
        }))
    }

    fn write_frame(&mut self, sample: &StatsSample<'_>) -> Result<bool> {
        if self
            .max_frames
            .is_some_and(|max_frames| self.written_frames >= max_frames)
        {
            self.finish();
            return Ok(false);
        }

        if let Some(writer) = self.mic.as_mut() {
            for v in sample.near {
                writer.write_sample(*v)?;
            }
        }
        if let Some(writer) = self.reference.as_mut() {
            for v in sample.far {
                writer.write_sample(*v)?;
            }
        }
        if let Some(writer) = self.out.as_mut() {
            for v in sample.out {
                writer.write_sample(*v)?;
            }
        }

        writeln!(
            self.stats,
            "{},{},{:.2},{:.2},{:.2},{},{},{},{:.2},{:.2},{},{},{},{},{},{},{},{},{:.3},{},{},{}",
            self.frame_index,
            sample.frame_size,
            rms_dbfs(sum_squares(sample.near), sample.near.len() as u64),
            rms_dbfs(sum_squares(sample.far), sample.far.len() as u64),
            rms_dbfs(sum_squares(sample.out), sample.out.len() as u64),
            sample.mic_q,
            sample.ref_q,
            sample.out_q,
            output_queue_latency_ms(sample.out_q, sample.sample_rate),
            estimate_user_latency_ms(
                sample.frame_ms,
                sample.algorithmic_latency_ms,
                sample.out_q,
                sample.sample_rate
            ),
            aggregate_estimated_delay_ms(sample.node_stats),
            sample.mic_input_drops,
            sample.ref_input_drops,
            sample.mic_input_drops + sample.ref_input_drops,
            sample.stale_drops,
            sample.ref_underruns,
            sample.output_overruns,
            sample.output_underruns,
            aggregate_process_time_ms(sample.node_stats),
            aggregate_runtime_errors(sample.node_stats),
            aggregate_diverged(sample.node_stats),
            csv_escape(&aggregate_last_error(sample.node_stats).unwrap_or_default()),
        )?;
        self.frame_index += 1;
        self.written_frames += sample.frame_size as u64;

        if self
            .max_frames
            .is_some_and(|max_frames| self.written_frames >= max_frames)
        {
            print_human(
                self.human_to_stderr,
                format!("诊断录制达到上限,已关闭: {}", self.dir.display()),
            );
            self.finish();
            return Ok(false);
        }
        Ok(true)
    }

    fn finish(&mut self) {
        if let Some(writer) = self.mic.take() {
            if let Err(err) = writer.finalize() {
                eprintln!("写入 mic.wav 尾部失败: {err}");
            }
        }
        if let Some(writer) = self.reference.take() {
            if let Err(err) = writer.finalize() {
                eprintln!("写入 ref.wav 尾部失败: {err}");
            }
        }
        if let Some(writer) = self.out.take() {
            if let Err(err) = writer.finalize() {
                eprintln!("写入 out.wav 尾部失败: {err}");
            }
        }
        if let Err(err) = self.stats.flush() {
            eprintln!("刷新诊断 stats.csv 失败: {err}");
        }
    }
}

impl Drop for DiagnosticRecorder {
    fn drop(&mut self) {
        self.finish();
    }
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
    algorithmic_latency_ms: f32,
    out_q_samples: usize,
    sample_rate: u32,
) -> f64 {
    frame_ms as f64 / 2.0
        + algorithmic_latency_ms as f64
        + output_queue_latency_ms(out_q_samples, sample_rate)
}

struct RealtimeStats {
    interval: Duration,
    started: Instant,
    last_print: Instant,
    sample_rate: u32,
    frame_ms: u32,
    backend: String,
    algorithmic_latency_ms: f32,
    status_json: bool,
    diagnostics_session_dir: Option<String>,
    total_frames: u64,
    near_samples: u64,
    far_samples: u64,
    out_samples: u64,
    near_sq: f64,
    far_sq: f64,
    out_sq: f64,
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
        backend: String,
        algorithmic_latency_ms: f32,
        status_json: bool,
        diagnostics_session_dir: Option<String>,
    ) -> Self {
        let now = Instant::now();
        Self {
            interval,
            started: now,
            last_print: now,
            sample_rate,
            frame_ms,
            backend,
            algorithmic_latency_ms,
            status_json,
            diagnostics_session_dir,
            total_frames: 0,
            near_samples: 0,
            far_samples: 0,
            out_samples: 0,
            near_sq: 0.0,
            far_sq: 0.0,
            out_sq: 0.0,
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

    fn observe(&mut self, sample: &StatsSample<'_>) {
        self.total_frames += sample.frame_size as u64;
        self.near_samples += sample.near.len() as u64;
        self.far_samples += sample.far.len() as u64;
        self.out_samples += sample.out.len() as u64;
        self.near_sq += sum_squares(sample.near);
        self.far_sq += sum_squares(sample.far);
        self.out_sq += sum_squares(sample.out);
        self.mic_q = sample.mic_q;
        self.ref_q = sample.ref_q;
        self.out_q = sample.out_q;
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
            "t={:.1}s frames={} mic={:.1}dB ref={:.1}dB out={:.1}dB mic_q={} ref_q={} out_q={} out_q_ms={:.1} est_user_ms={:.1} aec_delay_ms={} ref_underrun={} out_underrun={} out_overrun={} input_drop={} stale_drop={} node_ms={:.2} runtime_errors={} diverged={}",
            now.duration_since(self.started).as_secs_f64(),
            self.total_frames,
            rms_dbfs(self.near_sq, self.near_samples),
            rms_dbfs(self.far_sq, self.far_samples),
            rms_dbfs(self.out_sq, self.out_samples),
            self.mic_q,
            self.ref_q,
            self.out_q,
            output_queue_latency_ms(self.out_q, self.sample_rate),
            estimate_user_latency_ms(
                self.frame_ms,
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
            self.algorithmic_latency_ms,
            self.out_q,
            self.sample_rate,
        );
        json!({
            "type": "status",
            "elapsed_s": now.duration_since(self.started).as_secs_f64(),
            "frames": self.total_frames,
            "sample_rate": self.sample_rate,
            "frame_ms": self.frame_ms,
            "backend": self.backend.as_str(),
            "mic_dbfs": rms_dbfs(self.near_sq, self.near_samples),
            "ref_dbfs": rms_dbfs(self.far_sq, self.far_samples),
            "out_dbfs": rms_dbfs(self.out_sq, self.out_samples),
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
            .find(|(_, d)| device_search_text(d).to_lowercase().contains(&needle))
            .map(|(index, device)| SelectedDevice {
                index: Some(index),
                device,
            })
            .with_context(|| format!("无名称含 {selector:?} 的 {} 设备", kind.label()));
    }
    select_default_device(host, kind)
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

fn pick_config(
    device: &Device,
    kind: DeviceKind,
    sample_rate: u32,
) -> Result<SupportedStreamConfig> {
    let ranges: Vec<SupportedStreamConfigRange> = match kind {
        DeviceKind::Input => device.supported_input_configs()?.collect(),
        DeviceKind::Output => device.supported_output_configs()?.collect(),
    };
    ranges
        .into_iter()
        .filter(|r| !r.sample_format().is_dsd())
        .filter(|r| r.min_sample_rate() <= sample_rate && sample_rate <= r.max_sample_rate())
        .max_by(|a, b| a.cmp_default_heuristics(b))
        .map(|r| r.with_sample_rate(sample_rate))
        .with_context(|| {
            format!(
                "{} 在 {sample_rate} Hz 无可用 {} 配置",
                device_label(device),
                kind.label()
            )
        })
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
    config: &SupportedStreamConfig,
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
    supported: &SupportedStreamConfig,
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
    let config = supported.config();
    let channels = usize::from(config.channels);
    device
        .build_input_stream(
            &config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                for frame in data.chunks(channels) {
                    push_input_frame(frame, channel_mode, &mut producer, &drops);
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
            let sum = frame.iter().copied().map(f32::from_sample).sum::<f32>();
            let sample = if frame.is_empty() {
                0.0
            } else {
                sum / frame.len() as f32
            };
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

fn build_output_stream<C>(
    device: &Device,
    config: &SupportedStreamConfig,
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
    supported: &SupportedStreamConfig,
    mut consumer: C,
    underruns: Arc<AtomicU64>,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32> + Copy + Send + 'static,
    C: Consumer<Item = f32> + Send + 'static,
{
    let config = supported.config();
    let channels = usize::from(config.channels);
    device
        .build_output_stream(
            &config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels) {
                    let sample = match consumer.try_pop() {
                        Some(v) => v.clamp(-1.0, 1.0),
                        None => {
                            underruns.fetch_add(1, Ordering::Relaxed);
                            0.0
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
        "reference 还支持 'system'(默认输出 loopback)/ 'none' / 'output:<名>' / 'input:<名>'。"
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
        json!({ "id": "system", "label": "System audio", "kind": "system" }),
        json!({ "id": "none", "label": "No reference", "kind": "none" }),
    ];
    reference_sources.extend(input_devices.iter().enumerate().map(|(index, device)| {
        json!({
            "id": format!("input:{index}"),
            "label": device_label(device),
            "kind": "input",
            "device_index": index,
        })
    }));
    reference_sources.extend(output_devices.iter().enumerate().map(|(index, device)| {
        json!({
            "id": format!("output:{index}"),
            "label": device_label(device),
            "kind": "output",
            "device_index": index,
        })
    }));

    Ok(json!({
        "ok": true,
        "inputs": inputs,
        "outputs": outputs,
        "reference_sources": reference_sources,
    }))
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
        "index": index,
        "name": device_label(device),
        "kind": kind.label(),
        "is_default": default_index == Some(index),
        "selector": index.to_string(),
        "default_sample_rate": default_sample_rate,
        "channels": channels,
        "sample_format": sample_format,
        "config_error": config_error,
    })
}

fn config_summary(c: &SupportedStreamConfig) -> String {
    format!(
        "{} Hz, {} ch, {}",
        c.sample_rate(),
        c.channels(),
        c.sample_format()
    )
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
            DiagnosticRecorder::new(&cfg, 48_000, 2, 10, &node_stats, false)?.unwrap();
        let dir = recorder.dir.clone();
        let near = [0.25f32, -0.25];
        let far = [0.1f32, -0.1, 0.2, -0.2];
        let out = [0.0f32, 0.1];
        let sample = StatsSample {
            sample_rate: 48_000,
            frame_ms: 10,
            algorithmic_latency_ms: 16.0,
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
        assert!(dir.join("out.wav").exists());
        assert!(dir.join("metadata.txt").exists());

        let _ = std::fs::remove_dir_all(base);
        Ok(())
    }

    #[test]
    fn user_latency_estimate_includes_half_frame_algorithm_and_output_queue() {
        let latency = estimate_user_latency_ms(10, 16.0, 2400, 48_000);

        assert_eq!(latency, 71.0);
    }

    #[test]
    fn runtime_status_json_exposes_frontend_latency_fields() {
        let mut stats = RealtimeStats::new(
            Duration::from_millis(1000),
            48_000,
            10,
            "localvqe".into(),
            16.0,
            true,
            Some("diagnostics/session-1".into()),
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
        assert_eq!(value["output_queue_latency_ms"], 50.0);
        assert_eq!(value["estimated_user_latency_ms"], 71.0);
        assert_eq!(value["aec_estimated_delay_ms"], 48);
        assert_eq!(value["diagnostics_session_dir"], "diagnostics/session-1");
    }
}
