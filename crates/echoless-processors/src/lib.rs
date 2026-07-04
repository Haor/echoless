//! echoless-processors — 统一回声处理节点。
//!
//! 关键架构(蓝本 §7):aec3 经典 AEC3 与 LocalVQE 都是平级的 `EchoProcessor` 节点,
//! **没有「主引擎 + 残余」的固定主从**。怎么组合由配置决定:可单开、可串联、可自由组合、可扩展。
//! 加新方案 = 再写一个 `impl EchoProcessor` 并在 registry 注册,其余 crate 不动。

use serde::{Deserialize, Serialize};

pub mod aec3;
pub mod chain;
mod dsp;
pub mod localvqe;
pub mod nvafx;
pub mod passthrough;
pub mod registry;

pub use chain::{chain_from_nodes, ProcessorChain};

/// 处理器的「天然处理域」。`ProcessorChain` 在节点边界按它自动重采样 + 声道适配。
/// 例:Aec3Engine = {48000, near 1ch, far 2ch};LocalVqe = {16000, near 1ch, far 1ch}。
#[derive(Clone, Copy, Debug)]
pub struct IoSpec {
    pub sample_rate: u32,
    pub near_channels: u16,
    pub far_channels: u16,
    pub algorithmic_latency_ms: f32,
}

/// 单个节点的运行指标。
#[derive(Clone, Debug, Serialize)]
pub struct ProcessorStats {
    pub name: &'static str,
    pub erle_db: f32,
    pub residual_echo_likelihood: f32,
    pub estimated_delay_ms: i32,
    pub diverged: bool,
    pub mic_clipped: bool,
    pub process_time_ms: f32,
    pub runtime_error_count: u64,
    pub selected_model: Option<String>,
    pub selected_gpu_arch: Option<String>,
    pub last_backend_error: Option<String>,
}

impl ProcessorStats {
    pub fn empty(name: &'static str) -> Self {
        Self {
            name,
            erle_db: 0.0,
            residual_echo_likelihood: 0.0,
            estimated_delay_ms: 0,
            diverged: false,
            mic_clipped: false,
            process_time_ms: 0.0,
            runtime_error_count: 0,
            selected_model: None,
            selected_gpu_arch: None,
            last_backend_error: None,
        }
    }
}

/// 统一回声处理节点。约定:
///   - `near` = 上一级输出(链首为原始 mic);`far` = **始终为真实 far-end 参考**(非上一级产物)。
///   - 节点只在自己的 `io_spec()` 域里工作;跨域转换由 `ProcessorChain` 负责。
///   - 有状态节点(LocalVQE LSTM / AEC3 滤波器)即便被旁路也应持续喂帧(由 chain 保证)。
pub trait EchoProcessor: Send {
    fn name(&self) -> &'static str;
    fn io_spec(&self) -> IoSpec;
    fn configure(&mut self, params: &toml::Table) -> anyhow::Result<()>;
    fn set_stream_delay_ms(&mut self, _ms: i32) {}
    fn set_runtime_param(&mut self, _key: &str, _value: &toml::Value) -> anyhow::Result<bool> {
        Ok(false)
    }
    /// `near` / `far` 已由 chain 转到本节点 `io_spec` 域;写 `out`(同域,长度 = frames * near_channels)。
    fn process(&mut self, near: &[f32], far: &[f32], out: &mut [f32], frames: u32);
    fn stats(&self) -> ProcessorStats;
    fn reset(&mut self);
}

/// 链配置里的一个节点:`kind` + 该方案的自由参数(serde flatten 捕获额外键)。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NodeConfig {
    pub kind: String,
    #[serde(flatten, default)]
    pub params: toml::Table,
}
