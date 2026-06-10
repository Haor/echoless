//! 经典 AEC3 节点(包 vendored sonora — 纯 Rust WebRTC AudioProcessing 移植)。
//!
//! io_spec:48k,near mono,far mono/stereo 可配置。处理域固定 10ms = 480 样本/声道。
//! 调用顺序铁律:每块先 process_render(far),再 process_capture(near)。
//!
//! 调参(经 vendored fork 开放的 `aec3_config()` 注入 EchoCanceller3Config):
//!   tail_ms / delay_num_filters / linear_stable_echo_path。
//! 顺带可选 NS(降噪)/ AGC(自动增益)—— 与 AEC3 同属一个 sonora APM pipeline。
//! 详见 research/sonora_aec3_internal_map.md §2/§9/§11。
//!
//! feature `sonora-engine`(默认开)= 真实 AEC3;关掉 = 直通。

use crate::{EchoProcessor, IoSpec, ProcessorStats};

const SR: u32 = 48_000;
const FRAME: usize = 480; // 10ms @ 48k

/// 我们的高层调参。tail/num_filters/linear 映射到 EchoCanceller3Config;ns/agc 走 sonora APM。
#[derive(Clone)]
#[cfg_attr(not(feature = "sonora-engine"), allow(dead_code))]
struct Aec3Tuning {
    /// echo tail 长度(ms);底层 4ms/block,默认 52ms。外放+房间混响常需更长。
    tail_ms: Option<u32>,
    /// 延迟搜索的并行匹配滤波器数(默认 5 ≈ 608ms 搜索窗)。
    delay_num_filters: Option<usize>,
    /// 标记 echo path 线性且稳定(纯 loopback 参考时可酌情开)。
    linear_stable_echo_path: bool,
    /// 开启降噪(NoiseSuppression)。
    ns: bool,
    /// 降噪强度:low / moderate / high / veryhigh。
    ns_level: String,
    /// 开启 AGC2 自适应增益。
    agc: bool,
    /// far-end reference 声道数:1=mono downmix,2=stereo L/R。
    far_channels: u16,
}

impl Default for Aec3Tuning {
    fn default() -> Self {
        Self {
            tail_ms: None,
            delay_num_filters: None,
            linear_stable_echo_path: false,
            ns: false,
            ns_level: "low".into(),
            agc: false,
            far_channels: 1,
        }
    }
}

impl Aec3Tuning {
    /// 是否对 EchoCanceller3Config 无任何调参(决定是否走上游原始默认路径,见 §11.4)。
    /// 注:ns/agc 属高层 APM 配置,不影响 aec3_config 注入,故不计入。
    fn aec3_is_default(&self) -> bool {
        self.tail_ms.is_none() && self.delay_num_filters.is_none() && !self.linear_stable_echo_path
    }
}

pub struct SonoraAec3 {
    tuning: Aec3Tuning,
    initial_delay_ms: i32,
    last: ProcessorStats,
    #[cfg(feature = "sonora-engine")]
    inner: Inner,
    #[cfg(feature = "sonora-engine")]
    delay_applied: bool,
}

impl SonoraAec3 {
    pub fn new() -> Self {
        let tuning = Aec3Tuning::default();
        Self {
            #[cfg(feature = "sonora-engine")]
            inner: Inner::new(&tuning),
            #[cfg(feature = "sonora-engine")]
            delay_applied: false,
            tuning,
            initial_delay_ms: 0,
            last: ProcessorStats::empty("sonora_aec3"),
        }
    }
}
impl Default for SonoraAec3 {
    fn default() -> Self {
        Self::new()
    }
}

impl EchoProcessor for SonoraAec3 {
    fn name(&self) -> &'static str {
        "sonora_aec3"
    }
    fn io_spec(&self) -> IoSpec {
        IoSpec {
            sample_rate: SR,
            near_channels: 1,
            far_channels: self.tuning.far_channels,
            algorithmic_latency_ms: 0.0,
        }
    }
    fn configure(&mut self, params: &toml::Table) -> anyhow::Result<()> {
        // 注:外部延迟在默认路径下基本无效(只在 reset 时用一次,见 §4.3);保留备用。
        if let Some(v) = params.get("initial_delay_ms").and_then(|v| v.as_integer()) {
            self.initial_delay_ms = v as i32;
        }
        if let Some(v) = params.get("tail_ms").and_then(|v| v.as_integer()) {
            self.tuning.tail_ms = Some(v.max(4) as u32);
        }
        if let Some(v) = params.get("delay_num_filters").and_then(|v| v.as_integer()) {
            self.tuning.delay_num_filters = Some(v.max(1) as usize);
        }
        if let Some(v) = params
            .get("linear_stable_echo_path")
            .and_then(|v| v.as_bool())
        {
            self.tuning.linear_stable_echo_path = v;
        }
        if let Some(v) = params.get("ns").and_then(|v| v.as_bool()) {
            self.tuning.ns = v;
        }
        if let Some(v) = params.get("ns_level").and_then(|v| v.as_str()) {
            self.tuning.ns_level = v.to_string();
        }
        if let Some(v) = params.get("agc").and_then(|v| v.as_bool()) {
            self.tuning.agc = v;
        }
        if let Some(v) = params.get("reference_channels") {
            self.tuning.far_channels = parse_reference_channels(v)?;
        }
        // config 在引擎构造时注入,故参数变化需重建引擎。
        #[cfg(feature = "sonora-engine")]
        {
            self.inner = Inner::new(&self.tuning);
            self.delay_applied = false;
        }
        Ok(())
    }
    fn set_stream_delay_ms(&mut self, ms: i32) {
        self.initial_delay_ms = ms;
        #[cfg(feature = "sonora-engine")]
        {
            self.delay_applied = false;
        }
    }
    fn process(&mut self, near: &[f32], far: &[f32], out: &mut [f32], frames: u32) {
        #[cfg(feature = "sonora-engine")]
        {
            self.process_sonora(near, far, out, frames as usize);
        }
        #[cfg(not(feature = "sonora-engine"))]
        {
            let _ = (far, frames);
            crate::dsp::copy_or_zero(near, out);
        }
    }
    fn stats(&self) -> ProcessorStats {
        self.last.clone()
    }
    fn reset(&mut self) {
        #[cfg(feature = "sonora-engine")]
        {
            self.inner = Inner::new(&self.tuning);
            self.delay_applied = false;
        }
    }
}

// ── 把高层 tuning 映射成底层 EchoCanceller3Config ──────────────────────────────
#[cfg(feature = "sonora-engine")]
fn build_aec3_config(t: &Aec3Tuning) -> sonora::EchoCanceller3Config {
    const BLOCK_MS: u32 = 4; // 底层 4ms/block(common.rs)
    let mut c = sonora::EchoCanceller3Config::default();

    if let Some(tail_ms) = t.tail_ms {
        let blocks = ((tail_ms + BLOCK_MS / 2) / BLOCK_MS).max(1) as usize; // 四舍五入到 block
        c.filter.refined.length_blocks = blocks;
        c.filter.coarse.length_blocks = blocks;
        // 注:不动 refined_initial / coarse_initial。上游初始滤波器故意短(快速粗收敛,
        // 再切到长滤波器精修);拉长 initial 会破坏两阶段收敛、显著劣化效果(§11.3)。
    }
    if let Some(nf) = t.delay_num_filters {
        c.delay.num_filters = nf;
    }
    if t.linear_stable_echo_path {
        c.echo_removal_control.linear_and_stable_echo_path = true;
    }

    c.validate(); // clamp 所有字段到合法范围
    c
}

#[cfg(feature = "sonora-engine")]
fn ns_level(s: &str) -> sonora::config::NoiseSuppressionLevel {
    use sonora::config::NoiseSuppressionLevel as L;
    match s.to_ascii_lowercase().as_str() {
        "low" => L::Low,
        "high" => L::High,
        "veryhigh" | "very_high" | "very-high" => L::VeryHigh,
        _ => L::Moderate,
    }
}

fn parse_reference_channels(v: &toml::Value) -> anyhow::Result<u16> {
    if let Some(n) = v.as_integer() {
        return match n {
            1 => Ok(1),
            2 => Ok(2),
            _ => anyhow::bail!("reference_channels 只能是 1/2 或 mono/stereo"),
        };
    }
    let Some(s) = v.as_str() else {
        anyhow::bail!("reference_channels 只能是 1/2 或 mono/stereo");
    };
    match s.to_ascii_lowercase().as_str() {
        "mono" | "1" | "1ch" => Ok(1),
        "stereo" | "2" | "2ch" => Ok(2),
        _ => anyhow::bail!("reference_channels 只能是 1/2 或 mono/stereo"),
    }
}

// ── 真实 AEC3 实现 ────────────────────────────────────────────────────────────
#[cfg(feature = "sonora-engine")]
struct Inner {
    apm: sonora::AudioProcessing,
    far_channels: u16,
    near_buf: Vec<f32>,
    far_l: Vec<f32>,
    far_r: Vec<f32>,
    far_out_l: Vec<f32>,
    far_out_r: Vec<f32>,
    out_buf: Vec<f32>,
}

#[cfg(feature = "sonora-engine")]
impl Inner {
    fn new(tuning: &Aec3Tuning) -> Self {
        use sonora::config::{
            AdaptiveDigital, EchoCanceller, GainController2, NoiseSuppression, Pipeline,
        };
        use sonora::{AudioProcessing, Config, StreamConfig};

        let config = Config {
            echo_canceller: Some(EchoCanceller::default()),
            noise_suppression: tuning.ns.then(|| NoiseSuppression {
                level: ns_level(&tuning.ns_level),
                ..Default::default()
            }),
            gain_controller2: tuning.agc.then(|| GainController2 {
                adaptive_digital: Some(AdaptiveDigital::default()),
                ..Default::default()
            }),
            pipeline: Pipeline {
                multi_channel_render: tuning.far_channels > 1,
                multi_channel_capture: false, // near = mono
                ..Default::default()
            },
            ..Default::default()
        };

        let mut builder = AudioProcessing::builder().config(config);
        // 仅在真有 AEC3 调参时注入(经 vendored fork 开放的入口);否则保持上游默认路径(§11.4)。
        if !tuning.aec3_is_default() {
            builder = builder.aec3_config(build_aec3_config(tuning));
        }
        let apm = builder
            .capture_config(StreamConfig::new(SR, 1))
            .render_config(StreamConfig::new(SR, tuning.far_channels))
            .echo_detector(true) // 提供 residual_echo_likelihood(独立 EchoDetector,§7)
            .build();

        Self {
            apm,
            far_channels: tuning.far_channels,
            near_buf: vec![0.0; FRAME],
            far_l: vec![0.0; FRAME],
            far_r: vec![0.0; FRAME],
            far_out_l: vec![0.0; FRAME],
            far_out_r: vec![0.0; FRAME],
            out_buf: vec![0.0; FRAME],
        }
    }
}

#[cfg(feature = "sonora-engine")]
impl SonoraAec3 {
    fn process_sonora(&mut self, near: &[f32], far: &[f32], out: &mut [f32], frames: usize) {
        let mut runtime_error_count = self.last.runtime_error_count;
        let mut last_backend_error = self.last.last_backend_error.clone();
        if !self.delay_applied && self.initial_delay_ms > 0 {
            if let Err(err) = self.inner.apm.set_stream_delay_ms(self.initial_delay_ms) {
                record_backend_error(
                    &mut runtime_error_count,
                    &mut last_backend_error,
                    "set_stream_delay_ms",
                    err,
                );
            }
            self.delay_applied = true;
        }

        let mut i = 0;
        while i < frames {
            let blk = (frames - i).min(FRAME);

            // near = mono;far = mono 或 interleaved stereo → pad 到 480
            let far_channels = self.inner.far_channels as usize;
            for j in 0..FRAME {
                if j < blk {
                    self.inner.near_buf[j] = near.get(i + j).copied().unwrap_or(0.0);
                    let far_index = (i + j) * far_channels;
                    let left = far.get(far_index).copied().unwrap_or(0.0);
                    self.inner.far_l[j] = left;
                    self.inner.far_r[j] = if far_channels > 1 {
                        far.get(far_index + 1).copied().unwrap_or(left)
                    } else {
                        left
                    };
                } else {
                    self.inner.near_buf[j] = 0.0;
                    self.inner.far_l[j] = 0.0;
                    self.inner.far_r[j] = 0.0;
                }
            }

            // 先 render(far),再 capture(near)
            let render_result = if self.inner.far_channels > 1 {
                self.inner.apm.process_render_f32(
                    &[&self.inner.far_l, &self.inner.far_r],
                    &mut [&mut self.inner.far_out_l, &mut self.inner.far_out_r],
                )
            } else {
                self.inner
                    .apm
                    .process_render_f32(&[&self.inner.far_l], &mut [&mut self.inner.far_out_l])
            };
            if let Err(err) = render_result {
                record_backend_error(
                    &mut runtime_error_count,
                    &mut last_backend_error,
                    "process_render_f32",
                    err,
                );
            }
            if let Err(err) = self
                .inner
                .apm
                .process_capture_f32(&[&self.inner.near_buf], &mut [&mut self.inner.out_buf])
            {
                record_backend_error(
                    &mut runtime_error_count,
                    &mut last_backend_error,
                    "process_capture_f32",
                    err,
                );
                self.inner.out_buf[..blk].copy_from_slice(&self.inner.near_buf[..blk]);
            }

            let n = blk.min(out.len().saturating_sub(i));
            out[i..i + n].copy_from_slice(&self.inner.out_buf[..n]);
            i += blk;
        }

        let s = self.inner.apm.statistics();
        self.last = ProcessorStats {
            name: "sonora_aec3",
            erle_db: s.echo_return_loss_enhancement.unwrap_or(0.0) as f32,
            residual_echo_likelihood: s.residual_echo_likelihood.unwrap_or(0.0) as f32,
            estimated_delay_ms: s.delay_ms.unwrap_or(0),
            // 替代判据:上游 divergent_filter_fraction 恒 None(§7),改用"回声似然极高"近似
            // (AEC 基本未起作用)。可靠 diverged 待 fork 暴露 all_filters_diverged。
            diverged: s
                .residual_echo_likelihood
                .map(|p| p > 0.95)
                .unwrap_or(false),
            mic_clipped: false,
            process_time_ms: 0.0,
            runtime_error_count,
            selected_model: None,
            selected_gpu_arch: None,
            last_backend_error,
        };
    }
}

#[cfg(feature = "sonora-engine")]
fn record_backend_error(
    runtime_error_count: &mut u64,
    last_backend_error: &mut Option<String>,
    stage: &str,
    err: sonora::Error,
) {
    *runtime_error_count = runtime_error_count.saturating_add(1);
    *last_backend_error = Some(format!("{stage}: {err}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aec3_default_ns_level_matches_frontend_contract() {
        let tuning = Aec3Tuning::default();
        assert_eq!(tuning.ns_level, "low");
    }

    #[cfg(feature = "sonora-engine")]
    #[test]
    fn sonora_backend_errors_are_reported_in_stats() {
        let mut processor = SonoraAec3::new();
        processor.set_stream_delay_ms(600);

        let near = vec![0.0; FRAME];
        let far = vec![0.0; FRAME];
        let mut out = vec![1.0; FRAME];
        processor.process(&near, &far, &mut out, FRAME as u32);

        let stats = processor.stats();
        assert_eq!(stats.runtime_error_count, 1);
        assert!(stats
            .last_backend_error
            .as_deref()
            .is_some_and(|err| err.contains("set_stream_delay_ms: stream parameter was clamped")));
    }
}
