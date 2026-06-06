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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, FromSample, Sample, SampleFormat, SizedSample, Stream, SupportedStreamConfig,
    SupportedStreamConfigRange,
};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};

use echoless_core::PipelineConfig;
use echoless_processors::{ProcessorChain, chain_from_nodes};

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

/// 实时运行整条管线直到 Ctrl+C。
pub fn run(cfg: &PipelineConfig) -> Result<()> {
    let host = cpal::default_host();

    let mic_device = select_device(&host, DeviceKind::Input, mic_selector(&cfg.mic))
        .context("选择麦克风设备失败")?;
    let output_device = select_device(&host, DeviceKind::Output, output_selector(&cfg.output))
        .context("选择输出设备失败")?;
    // reference:"none" = 无参考(纯 NS);"system" = 默认输出做 loopback;否则按名。
    let render_device = match cfg.reference.as_str() {
        "none" | "" => None,
        "system" | "default" => Some((
            host.default_output_device().context("无默认输出设备可作系统 loopback")?,
            DeviceKind::Output,
        )),
        sel => Some(select_render_device(&host, sel).context("选择参考设备失败")?),
    };

    let sample_rate = cfg.sample_rate;
    if !sample_rate.is_multiple_of(100) {
        bail!("采样率必须能整除 10ms 帧(sample_rate % 100 == 0):{sample_rate}");
    }
    let frame_size = (sample_rate / 100) as usize;
    let ring_size = frame_size * 12; // ~120ms

    let mic_config =
        pick_config(&mic_device, DeviceKind::Input, sample_rate).context("麦克风不支持该采样率")?;
    let output_config = pick_config(&output_device, DeviceKind::Output, sample_rate)
        .context("输出设备不支持该采样率")?;
    let render_config = match &render_device {
        Some((d, k)) => {
            Some(pick_config(d, *k, sample_rate).context("参考设备不支持该采样率")?)
        }
        None => None,
    };

    println!("Mic:    {} ({})", device_name(&mic_device), config_summary(&mic_config));
    match (&render_device, &render_config) {
        (Some((d, k)), Some(c)) => {
            println!("Ref:    {} {} ({})", k.label(), device_name(d), config_summary(c))
        }
        _ => println!("Ref:    无 —— AEC 缺少参考,仅 NS 等单端处理有效"),
    }
    println!("Output: {} ({})", device_name(&output_device), config_summary(&output_config));

    let chain_desc = if cfg.chain.is_empty() {
        "直通".to_string()
    } else {
        cfg.chain.iter().map(|n| n.kind.clone()).collect::<Vec<_>>().join(" → ")
    };
    println!("采样率 {sample_rate} Hz · 帧 {frame_size} 样本 · 链: {chain_desc}");

    let running = Arc::new(AtomicBool::new(true));
    ctrlc::set_handler({
        let running = running.clone();
        move || running.store(false, Ordering::SeqCst)
    })?;

    let (mic_prod, mic_cons) = HeapRb::<f32>::new(ring_size).split();
    let (out_prod, out_cons) = HeapRb::<f32>::new(ring_size).split();
    let (render_prod, render_cons) = if render_device.is_some() {
        let (p, c) = HeapRb::<f32>::new(ring_size).split();
        (Some(p), Some(c))
    } else {
        (None, None)
    };

    let mic_stream = build_input_stream(&mic_device, &mic_config, mic_prod, "mic")?;
    let render_stream = match (render_device.as_ref(), render_config.as_ref(), render_prod) {
        (Some((d, _)), Some(c), Some(p)) => Some(build_input_stream(d, c, p, "ref")?),
        _ => None,
    };
    let output_stream = build_output_stream(&output_device, &output_config, out_cons)?;

    // 处理线程:只碰 ring(Send),cpal Stream 留在本线程(!Send)。
    let proc_running = running.clone();
    let chain = chain_from_nodes(&cfg.chain, sample_rate, 1)?;
    let proc = thread::spawn(move || {
        process_loop(proc_running, chain, frame_size, mic_cons, render_cons, out_prod);
    });

    mic_stream.play()?;
    if let Some(s) = &render_stream {
        s.play()?;
    }
    output_stream.play()?;

    println!("运行中。macOS 首次需授予麦克风权限。Ctrl+C 停止。");
    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
    }

    drop(mic_stream);
    drop(render_stream);
    drop(output_stream);
    proc.join().ok();
    println!("已停止。");
    Ok(())
}

fn process_loop<M, R, O>(
    running: Arc<AtomicBool>,
    mut chain: ProcessorChain,
    frame_size: usize,
    mut mic_cons: M,
    mut render_cons: Option<R>,
    mut out_prod: O,
) where
    M: Consumer<Item = f32>,
    R: Consumer<Item = f32>,
    O: Producer<Item = f32>,
{
    let mut near = vec![0.0f32; frame_size];
    let mut far = vec![0.0f32; frame_size];
    let mut out = vec![0.0f32; frame_size];

    while running.load(Ordering::SeqCst) {
        if mic_cons.occupied_len() < frame_size {
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        // 控制积压(简单 drift/堆积处理):超 4 帧丢旧的。
        skip_stale(&mut mic_cons, frame_size);
        mic_cons.pop_slice(&mut near);

        if let Some(rc) = render_cons.as_mut() {
            skip_stale(rc, frame_size);
            if rc.occupied_len() >= frame_size {
                rc.pop_slice(&mut far);
            } else {
                far.fill(0.0); // 参考欠载 → 填静音
            }
        } else {
            far.fill(0.0);
        }

        chain.process(&near, &far, &mut out, frame_size as u32);
        out_prod.push_slice(&out);
    }
}

fn skip_stale<C: Consumer<Item = f32>>(consumer: &mut C, frame_size: usize) {
    let max_queued = frame_size * 4;
    let queued = consumer.occupied_len();
    if queued > max_queued {
        consumer.skip(queued - max_queued);
    }
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

fn select_device(host: &cpal::Host, kind: DeviceKind, selector: Option<&str>) -> Result<Device> {
    if let Some(selector) = selector {
        let devices = devices_for(host, kind)?;
        if let Ok(index) = selector.parse::<usize>() {
            return devices
                .into_iter()
                .nth(index)
                .with_context(|| format!("无 {} 设备索引 {index}", kind.label()));
        }
        let needle = selector.to_lowercase();
        return devices
            .into_iter()
            .find(|d| device_name(d).to_lowercase().contains(&needle))
            .with_context(|| format!("无名称含 {selector:?} 的 {} 设备", kind.label()));
    }
    match kind {
        DeviceKind::Input => host.default_input_device(),
        DeviceKind::Output => host.default_output_device(),
    }
    .with_context(|| format!("无默认 {} 设备", kind.label()))
}

fn select_render_device(host: &cpal::Host, selector: &str) -> Result<(Device, DeviceKind)> {
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

fn pick_config(device: &Device, kind: DeviceKind, sample_rate: u32) -> Result<SupportedStreamConfig> {
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
            format!("{} 在 {sample_rate} Hz 无可用 {} 配置", device_name(device), kind.label())
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
) -> Result<Stream>
where
    P: Producer<Item = f32> + Send + 'static,
{
    dispatch_format!(config.sample_format(), build_input_stream_t, device, config, producer, label)
}

fn build_input_stream_t<T, P>(
    device: &Device,
    supported: &SupportedStreamConfig,
    mut producer: P,
    label: &'static str,
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
                    // 下混单声道
                    let sum = frame.iter().copied().map(f32::from_sample).sum::<f32>();
                    let _ = producer.try_push(sum / frame.len() as f32);
                }
            },
            move |err| eprintln!("{label} 流错误: {err}"),
            None,
        )
        .with_context(|| format!("构建 {label} 输入流失败"))
}

fn build_output_stream<C>(
    device: &Device,
    config: &SupportedStreamConfig,
    consumer: C,
) -> Result<Stream>
where
    C: Consumer<Item = f32> + Send + 'static,
{
    dispatch_format!(config.sample_format(), build_output_stream_t, device, config, consumer)
}

fn build_output_stream_t<T, C>(
    device: &Device,
    supported: &SupportedStreamConfig,
    mut consumer: C,
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
                    let sample = consumer.try_pop().unwrap_or(0.0).clamp(-1.0, 1.0);
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
            let summary = cfg.map(|c| config_summary(&c)).unwrap_or_else(|e| format!("无默认配置: {e}"));
            println!("  [{i}] {} ({summary})", device_name(d));
        }
    }
    println!("\n用法:--mic / reference / output 可填索引、名称片段;");
    println!("reference 还支持 'system'(默认输出 loopback)/ 'none' / 'output:<名>' / 'input:<名>'。");
    Ok(())
}

fn config_summary(c: &SupportedStreamConfig) -> String {
    format!("{} Hz, {} ch, {}", c.sample_rate(), c.channels(), c.sample_format())
}

fn device_name(device: &Device) -> String {
    device.description().map(|d| d.name().to_owned()).unwrap_or_else(|_| "<未知>".to_owned())
}
