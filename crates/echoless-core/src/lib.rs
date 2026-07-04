//! echoless-core — 管线配置 + 离线编排 + 共享 DSP 边界工具。**不依赖任何平台 crate**。
//!
//! 实时主路径当前由 `echoless-cli` 的 cpal sidecar runtime 提供;本 crate 不暴露未实现的
//! realtime 控制面,只保留 CLI/GUI 共用的配置、离线路径与输出电平/声道策略。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use echoless_audio_io::{AudioFormat, AudioSink, AudioSource};
use echoless_processors::{chain_from_nodes, NodeConfig, ProcessorStats};

pub use echoless_processors::{
    EchoProcessor, NodeConfig as ChainNode, ProcessorChain, ProcessorStats as NodeStats,
};

pub const MAX_NEAR_DELAY_MS: u32 = 500;
pub const MAX_INITIAL_DELAY_MS: u32 = 500;
pub const MIN_OUTPUT_LEVEL: u32 = 0;
pub const MAX_OUTPUT_LEVEL: u32 = 100;
pub const DEFAULT_OUTPUT_LEVEL: u32 = 50;
pub const UNITY_OUTPUT_LEVEL: u32 = 50;
pub const OUTPUT_LEVEL_CURVE_EXPONENT: f32 = 1.584_962_5; // log2(3)
pub const OUTPUT_LEVEL_MAX_BOOST_DB: f32 = 9.542_425;
pub const OUTPUT_LEVEL_MAX_GAIN: f32 = 3.0;
pub const OUTPUT_SOFT_LIMIT_THRESHOLD: f32 = 0.95;

fn default_sample_rate() -> u32 {
    48000
}
fn default_frame_ms() -> u32 {
    10
}
fn default_reference_channels() -> ReferenceChannels {
    ReferenceChannels::Mono
}
pub fn default_near_delay_ms() -> u32 {
    if cfg!(target_os = "macos") {
        25
    } else {
        0
    }
}
fn default_mic() -> String {
    "default".into()
}
fn default_reference() -> String {
    "system".into()
}
fn default_output() -> String {
    "default".into()
}
pub fn default_output_level() -> u32 {
    DEFAULT_OUTPUT_LEVEL
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReferenceChannels {
    Mono,
    Stereo,
}

impl ReferenceChannels {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mono => "mono",
            Self::Stereo => "stereo",
        }
    }

    pub fn channel_count(self) -> u16 {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
        }
    }
}

/// Diagnostic capture settings for realtime evidence collection.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct DiagnosticsConfig {
    /// Directory where timestamped diagnostic sessions are written.
    #[serde(default)]
    pub record_dir: Option<String>,
    /// Optional maximum recording duration. None means record until stop.
    #[serde(default)]
    pub max_seconds: Option<u32>,
}

/// 整条管线配置(设备选择 + 处理链)。TOML/JSON 都映射到它。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PipelineConfig {
    /// 麦克风设备(near):"default" 或设备名/ID。
    #[serde(default = "default_mic")]
    pub mic: String,
    /// far-end 参考源:Win="system"(loopback),mac="system"(Process Tap),或设备名。
    #[serde(default = "default_reference")]
    pub reference: String,
    /// 虚拟音频输出:Win=VB-Cable 名,mac=BlackHole 名。
    #[serde(default = "default_output")]
    pub output: String,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_frame_ms")]
    pub frame_ms: u32,
    #[serde(default = "default_reference_channels")]
    pub reference_channels: ReferenceChannels,
    /// Delay near/mic before it enters the processor to align late-arriving references.
    #[serde(default = "default_near_delay_ms")]
    pub near_delay_ms: u32,
    /// Final output level after all processors. 0=mute, 50=unity, 100=3x gain.
    #[serde(default = "default_output_level")]
    pub output_level: u32,
    /// Optional realtime diagnostic recordings.
    #[serde(default)]
    pub diagnostics: DiagnosticsConfig,
    /// 处理链:有序节点;空 = 直通。可单开/串联/组合。
    #[serde(default)]
    pub chain: Vec<NodeConfig>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            mic: default_mic(),
            reference: default_reference(),
            output: default_output(),
            sample_rate: default_sample_rate(),
            frame_ms: default_frame_ms(),
            reference_channels: default_reference_channels(),
            near_delay_ms: default_near_delay_ms(),
            output_level: default_output_level(),
            diagnostics: DiagnosticsConfig::default(),
            chain: Vec::new(),
        }
    }
}

impl PipelineConfig {
    pub fn frame_size(&self) -> u32 {
        let frames = (u64::from(self.sample_rate) * u64::from(self.frame_ms)) / 1000;
        frames.clamp(1, u64::from(u32::MAX)) as u32
    }
}

/// 离线运行结果。
#[derive(Clone, Debug)]
pub struct RunReport {
    pub frames: u64,
    pub seconds: f64,
    pub chain: Vec<&'static str>,
    pub total_latency_ms: f32,
    pub node_stats: Vec<ProcessorStats>,
}

/// 离线跑通整条链:mic 源 + ref 源 → 处理链 → sink。当前可用(P1 离线评测)。
pub fn run_offline<M, R, S>(
    cfg: &PipelineConfig,
    mut mic: M,
    mut reference: R,
    mut sink: S,
) -> anyhow::Result<RunReport>
where
    M: AudioSource,
    R: AudioSource,
    S: AudioSink,
{
    let mic_fmt = mic.start()?;
    let ref_fmt = reference.start()?;
    if mic_fmt.sample_rate != cfg.sample_rate || ref_fmt.sample_rate != cfg.sample_rate {
        anyhow::bail!(
            "离线骨架要求 mic/ref 采样率 == cfg.sample_rate ({});实际 mic={}, ref={}。\
             音频 I/O 边界重采样属 TODO,请先按 {} 录制。",
            cfg.sample_rate,
            mic_fmt.sample_rate,
            ref_fmt.sample_rate,
            cfg.sample_rate
        );
    }

    sink.start(AudioFormat {
        sample_rate: cfg.sample_rate,
        channels: 1,
    })?;

    let mut chain_cfg = cfg.chain.clone();
    apply_reference_channels_to_chain(&mut chain_cfg, cfg.reference_channels);
    let reference_channels = cfg.reference_channels.channel_count();
    let mut chain = chain_from_nodes(&chain_cfg, cfg.sample_rate, reference_channels)?;
    let chain_names = chain.names();
    let total_latency_ms = chain.total_latency_ms();

    let timeout = Duration::from_millis(100);
    let mut total_frames: u64 = 0;

    loop {
        let m = mic.read(timeout)?;
        let r = reference.read(timeout)?;
        let (mp, rp) = match (m, r) {
            (Some(a), Some(b)) => (a, b),
            _ => break,
        };
        let frames = mp.frames.min(rp.frames);
        if frames == 0 {
            break;
        }
        let near = downmix_to_mono(&mp.data, mp.format.channels, frames);
        let far =
            remap_reference_channels(&rp.data, ref_fmt.channels, frames, cfg.reference_channels);
        let mut out = vec![0f32; frames as usize];
        chain.process(&near, &far, &mut out, frames);
        apply_output_level(&mut out, cfg.output_level);
        sink.write(&out, frames)?;
        total_frames += frames as u64;
    }

    sink.stop();
    mic.stop();
    reference.stop();

    Ok(RunReport {
        frames: total_frames,
        seconds: total_frames as f64 / cfg.sample_rate as f64,
        chain: chain_names,
        total_latency_ms,
        node_stats: chain.stats(),
    })
}

pub fn apply_reference_channels_to_chain(nodes: &mut [NodeConfig], mode: ReferenceChannels) {
    for node in nodes.iter_mut().filter(|node| {
        node.kind == "aec3" || node.kind == "sonora_aec3" // legacy alias, remove after 2 releases
    }) {
        node.params.insert(
            "reference_channels".to_string(),
            toml::Value::String(mode.as_str().to_string()),
        );
    }
}

pub fn output_level_gain_db(level: u32) -> Option<f32> {
    let gain = output_level_gain(level);
    (gain > 0.0).then(|| 20.0 * gain.log10())
}

pub fn output_level_gain(level: u32) -> f32 {
    let level = level.min(MAX_OUTPUT_LEVEL);
    if level == 0 {
        return 0.0;
    }
    (level as f32 / UNITY_OUTPUT_LEVEL as f32).powf(OUTPUT_LEVEL_CURVE_EXPONENT)
}

pub fn apply_output_level(samples: &mut [f32], level: u32) {
    let gain = output_level_gain(level);
    for sample in samples {
        *sample = soft_limit_output_sample(*sample * gain);
    }
}

pub fn soft_limit_output_sample(sample: f32) -> f32 {
    if !sample.is_finite() {
        return 0.0;
    }
    let abs = sample.abs();
    if abs <= OUTPUT_SOFT_LIMIT_THRESHOLD {
        return sample;
    }

    let headroom = 1.0 - OUTPUT_SOFT_LIMIT_THRESHOLD;
    let excess = abs - OUTPUT_SOFT_LIMIT_THRESHOLD;
    let limited = OUTPUT_SOFT_LIMIT_THRESHOLD + headroom * (1.0 - (-excess / headroom).exp());
    sample.signum() * limited.min(1.0)
}

fn downmix_to_mono(data: &[f32], channels: u16, frames: u32) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    let frames = frames as usize;
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let start = f * ch;
        if start + ch > data.len() {
            break;
        }
        let s: f32 = data[start..start + ch].iter().copied().sum::<f32>() / ch as f32;
        out.push(s);
    }
    out
}

fn remap_reference_channels(
    data: &[f32],
    channels: u16,
    frames: u32,
    mode: ReferenceChannels,
) -> Vec<f32> {
    match mode {
        ReferenceChannels::Mono => downmix_to_mono(data, channels, frames),
        ReferenceChannels::Stereo => preserve_first_two_channels(data, channels, frames),
    }
}

fn preserve_first_two_channels(data: &[f32], channels: u16, frames: u32) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    let frames = frames as usize;
    let mut out = Vec::with_capacity(frames * 2);
    for f in 0..frames {
        let start = f * ch;
        if start >= data.len() {
            break;
        }
        let left = data[start];
        let right = if ch > 1 {
            data.get(start + 1).copied().unwrap_or(left)
        } else {
            left
        };
        out.push(left);
        out.push(right);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, epsilon: f32) {
        assert!(
            (a - b).abs() <= epsilon,
            "expected {a} to be within {epsilon} of {b}"
        );
    }

    #[test]
    fn output_level_curve_has_stable_anchors() {
        assert_eq!(output_level_gain_db(0), None);
        approx_eq(output_level_gain(25), 1.0 / 3.0, 0.001);
        approx_eq(
            output_level_gain_db(UNITY_OUTPUT_LEVEL).unwrap(),
            0.0,
            0.001,
        );
        approx_eq(
            output_level_gain(MAX_OUTPUT_LEVEL),
            OUTPUT_LEVEL_MAX_GAIN,
            0.001,
        );
        approx_eq(
            output_level_gain_db(MAX_OUTPUT_LEVEL).unwrap(),
            OUTPUT_LEVEL_MAX_BOOST_DB,
            0.001,
        );
        approx_eq(
            output_level_gain(MAX_OUTPUT_LEVEL + 1),
            OUTPUT_LEVEL_MAX_GAIN,
            0.001,
        );
        approx_eq(output_level_gain(u32::MAX), OUTPUT_LEVEL_MAX_GAIN, 0.001);
        approx_eq(
            output_level_gain_db(u32::MAX).unwrap(),
            OUTPUT_LEVEL_MAX_BOOST_DB,
            0.001,
        );
    }

    #[test]
    fn frame_size_handles_zero_and_extreme_values_without_overflow() {
        let normal = PipelineConfig {
            sample_rate: 48_000,
            frame_ms: 10,
            ..PipelineConfig::default()
        };
        assert_eq!(normal.frame_size(), 480);

        let zero = PipelineConfig {
            sample_rate: 0,
            frame_ms: 0,
            ..PipelineConfig::default()
        };
        assert_eq!(zero.frame_size(), 1);

        let huge = PipelineConfig {
            sample_rate: u32::MAX,
            frame_ms: u32::MAX,
            ..PipelineConfig::default()
        };
        assert_eq!(huge.frame_size(), u32::MAX);
    }

    #[test]
    fn output_level_applies_mute_unity_boost_and_soft_limit() {
        let mut muted = [0.25, -0.25];
        apply_output_level(&mut muted, 0);
        assert_eq!(muted, [0.0, 0.0]);

        let mut unity = [0.25, -0.5];
        apply_output_level(&mut unity, UNITY_OUTPUT_LEVEL);
        approx_eq(unity[0], 0.25, 0.001);
        approx_eq(unity[1], -0.5, 0.001);

        let mut boosted = [0.2, -0.2];
        apply_output_level(&mut boosted, MAX_OUTPUT_LEVEL);
        approx_eq(boosted[0], 0.6, 0.001);
        approx_eq(boosted[1], -0.6, 0.001);

        let mut protected = [0.6, f32::INFINITY, f32::NAN];
        apply_output_level(&mut protected, MAX_OUTPUT_LEVEL);
        assert!(protected[0] <= 1.0);
        assert!(protected[0] > OUTPUT_SOFT_LIMIT_THRESHOLD);
        assert_eq!(protected[1], 0.0);
        assert_eq!(protected[2], 0.0);

        let mut over_range = [0.2];
        apply_output_level(&mut over_range, u32::MAX);
        approx_eq(over_range[0], 0.6, 0.001);
    }
}
