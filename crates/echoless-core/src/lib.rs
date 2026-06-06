//! echoless-core — 管线编排 + 配置 + 控制面。**不依赖任何平台 crate**(蓝本 §1)。
//!
//! 前端(CLI 现在、Electron 后期)只透过 `ControlApi` 访问;配置类型 serde 可序列化,
//! CLI 用 TOML、Electron 用 JSON,映射到同一套(蓝本 §14)。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use echoless_hal::{AudioFormat, AudioSink, AudioSource, DeviceInfo};
use echoless_processors::{chain_from_nodes, NodeConfig, ProcessorStats};

pub use echoless_processors::{EchoProcessor, NodeConfig as ChainNode, ProcessorChain, ProcessorStats as NodeStats};

fn default_sample_rate() -> u32 {
    48000
}
fn default_frame_ms() -> u32 {
    10
}

/// 整条管线配置(设备选择 + 处理链)。TOML/JSON 都映射到它。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PipelineConfig {
    /// 麦克风设备(near):"default" 或设备名/ID。
    pub mic: String,
    /// far-end 参考源:Win="system"(loopback),mac="system"(Process Tap),或设备名。
    pub reference: String,
    /// 虚拟麦输出:Win=VB-Cable 名,mac=BlackHole 名。
    pub output: String,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_frame_ms")]
    pub frame_ms: u32,
    /// 处理链:有序节点;空 = 直通。可单开/串联/组合。
    #[serde(default)]
    pub chain: Vec<NodeConfig>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            mic: "default".into(),
            reference: "system".into(),
            output: "default".into(),
            sample_rate: default_sample_rate(),
            frame_ms: default_frame_ms(),
            chain: Vec::new(),
        }
    }
}

impl PipelineConfig {
    pub fn frame_size(&self) -> u32 {
        (self.sample_rate * self.frame_ms / 1000).max(1)
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

/// 控制面:CLI 现在直接内嵌调用;Electron 后期经 echoless-daemon 映射成 JSON-RPC(蓝本 §14)。
pub trait ControlApi: Send + Sync {
    fn list_devices(&self) -> Vec<DeviceInfo>;
    fn start(&self, cfg: &PipelineConfig) -> anyhow::Result<()>;
    fn stop(&self);
    fn set_chain(&self, nodes: &[NodeConfig]) -> anyhow::Result<()>;
    fn subscribe_stats(&self) -> crossbeam_channel::Receiver<Vec<ProcessorStats>>;
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
             HAL 边界重采样属 TODO,请先按 {} 录制。",
            cfg.sample_rate,
            mic_fmt.sample_rate,
            ref_fmt.sample_rate,
            cfg.sample_rate
        );
    }

    sink.start(AudioFormat { sample_rate: cfg.sample_rate, channels: 1 })?;

    let mut chain = chain_from_nodes(&cfg.chain, cfg.sample_rate, ref_fmt.channels)?;
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
        let far_len = (frames as usize) * ref_fmt.channels.max(1) as usize;
        let far = &rp.data[..far_len.min(rp.data.len())];
        let mut out = vec![0f32; frames as usize];
        chain.process(&near, far, &mut out, frames);
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

/// 实时管线(基于泛型 AudioSource/Sink 的版本)。
///
/// 注:MVP 的实时管线已落在 `echoless-cli` 的 cpal 实现(`realtime.rs`)——cpal 的回调
/// 是 push 模型且 Stream !Send,直接套 pull 式 AudioSource 代价大,故 I/O 与处理分离、
/// 处理仍走同一个 `ProcessorChain`。此泛型版保留供未来把实时编排抽回 core(经 daemon
/// 复用)时使用;当前用 cpal 路径,见 cli。
pub fn run_realtime<M, R, S>(_cfg: &PipelineConfig, mut mic: M, _reference: R, _sink: S) -> anyhow::Result<()>
where
    M: AudioSource,
    R: AudioSource,
    S: AudioSink,
{
    let _ = mic.start()?;
    anyhow::bail!("请用 echoless-cli 的 cpal 实时管线(`echoless run`);core 泛型版待重构")
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
