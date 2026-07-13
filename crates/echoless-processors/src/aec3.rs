//! 经典 AEC3 节点(包 vendored aec3 — 纯 Rust WebRTC AudioProcessing 移植)。
//!
//! io_spec:48k,near mono,far mono/stereo 可配置。处理域固定 10ms = 480 样本/声道。
//! 调用顺序铁律:每块先 process_render(far),再 process_capture(near)。
//!
//! 调参(经 vendored fork 开放的 `aec3_config()` 注入 EchoCanceller3Config):
//!   delay_hold / tail_ms / delay_num_filters / linear_stable_echo_path。
//! AGC remains part of this AEC3 APM pipeline. Noise suppression is a separate
//! processor node so every echo canceller can share the same post-filter.
//! 详见 research/aec3_internal_map.md §2/§9/§11。
//!
//! feature `aec3-engine`(默认开)= 真实 AEC3;关掉 = 直通。

use crate::{EchoProcessor, IoSpec, ProcessorStats};

const SR: u32 = 48_000;
const FRAME: usize = 480; // 10ms @ 48k
const AEC3_BLOCK_MS: u32 = 4;
pub const MIN_TAIL_MS: u32 = 14;

/// 我们的高层调参。tail/num_filters/linear 映射到 EchoCanceller3Config;agc 走 aec3 APM。
#[derive(Clone)]
#[cfg_attr(not(feature = "aec3-engine"), allow(dead_code))]
struct Aec3Tuning {
    /// echo tail 长度(ms);底层 4ms/block,默认 52ms。外放+房间混响常需更长。
    tail_ms: Option<u32>,
    /// 延迟搜索的并行匹配滤波器数(默认 5 ≈ 608ms 搜索窗)。
    delay_num_filters: Option<usize>,
    /// 标记 echo path 线性且稳定(纯 loopback 参考时可酌情开)。
    linear_stable_echo_path: bool,
    /// ref 断续/静音时保持 AEC3 延迟估计;产品默认显式开启,配置层可置 false。
    /// None 仅表示不覆盖 vendored upstream 字段默认值。
    delay_hold: Option<bool>,
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
            delay_hold: Some(true),
            agc: false,
            far_channels: 1,
        }
    }
}

pub struct Aec3Engine {
    tuning: Aec3Tuning,
    initial_delay_ms: i32,
    last: ProcessorStats,
    #[cfg(feature = "aec3-engine")]
    inner: Inner,
    #[cfg(feature = "aec3-engine")]
    stream_delay_pending: bool,
}

impl Aec3Engine {
    pub fn new() -> Self {
        let tuning = Aec3Tuning::default();
        Self {
            #[cfg(feature = "aec3-engine")]
            inner: Inner::new(&tuning),
            #[cfg(feature = "aec3-engine")]
            stream_delay_pending: false,
            tuning,
            initial_delay_ms: 0,
            last: ProcessorStats::empty("aec3"),
        }
    }
}
impl Default for Aec3Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl EchoProcessor for Aec3Engine {
    fn name(&self) -> &'static str {
        "aec3"
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
        // 注:外部延迟在当前 realtime 路径下基本无效(只在 reset 时用一次,见 §4.3);保留备用。
        if let Some(v) = params.get("initial_delay_ms").and_then(|v| v.as_integer()) {
            self.initial_delay_ms = v as i32;
        }
        if let Some(value) = params.get("tail_ms") {
            let v = value
                .as_integer()
                .ok_or_else(|| anyhow::anyhow!("tail_ms must be an integer"))?;
            if v < i64::from(MIN_TAIL_MS) {
                anyhow::bail!("tail_ms must be >= {MIN_TAIL_MS}");
            }
            self.tuning.tail_ms =
                Some(u32::try_from(v).map_err(|_| anyhow::anyhow!("tail_ms is too large"))?);
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
        if let Some(v) = params.get("delay_hold").and_then(|v| v.as_bool()) {
            self.tuning.delay_hold = Some(v);
        }
        if let Some(v) = params.get("agc").and_then(|v| v.as_bool()) {
            self.tuning.agc = v;
        }
        if let Some(v) = params.get("reference_channels") {
            self.tuning.far_channels = parse_reference_channels(v)?;
        }
        // config 在引擎构造时注入,故参数变化需重建引擎。
        #[cfg(feature = "aec3-engine")]
        {
            self.inner = Inner::new(&self.tuning);
            self.stream_delay_pending = self.initial_delay_ms > 0;
        }
        Ok(())
    }
    fn set_stream_delay_ms(&mut self, ms: i32) {
        self.initial_delay_ms = ms;
        #[cfg(feature = "aec3-engine")]
        {
            self.stream_delay_pending = true;
        }
    }
    fn set_runtime_param(&mut self, key: &str, value: &toml::Value) -> anyhow::Result<bool> {
        match key {
            "agc" => {
                self.tuning.agc = value
                    .as_bool()
                    .ok_or_else(|| anyhow::anyhow!("agc must be a boolean"))?;
                self.apply_runtime_apm_config();
                Ok(true)
            }
            _ => Ok(false),
        }
    }
    fn process(&mut self, near: &[f32], far: &[f32], out: &mut [f32], frames: u32) {
        #[cfg(feature = "aec3-engine")]
        {
            self.process_aec3(near, far, out, frames as usize);
        }
        #[cfg(not(feature = "aec3-engine"))]
        {
            let _ = (far, frames);
            crate::dsp::copy_or_zero(near, out);
        }
    }
    fn stats(&self) -> ProcessorStats {
        self.last.clone()
    }
    fn reset(&mut self) {
        #[cfg(feature = "aec3-engine")]
        {
            self.inner = Inner::new(&self.tuning);
            self.stream_delay_pending = self.initial_delay_ms > 0;
        }
    }
}

// ── 把高层 tuning 映射成底层 EchoCanceller3Config ──────────────────────────────
#[cfg(feature = "aec3-engine")]
fn build_aec3_config(t: &Aec3Tuning) -> aec3_apm::EchoCanceller3Config {
    let mut c = aec3_apm::EchoCanceller3Config::default();

    if let Some(tail_ms) = t.tail_ms {
        let blocks = tail_ms_to_filter_blocks(tail_ms);
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
    if let Some(delay_hold) = t.delay_hold {
        c.delay.delay_hold = delay_hold;
        c.delay.render_gate_power_threshold = c.render_levels.active_render_limit;
        c.delay.render_gate_hold_blocks = 3;
    }

    c.validate(); // clamp 所有字段到合法范围
    c
}

fn tail_ms_to_filter_blocks(tail_ms: u32) -> usize {
    ((u64::from(tail_ms) + u64::from(AEC3_BLOCK_MS / 2)) / u64::from(AEC3_BLOCK_MS)) as usize
}

fn parse_reference_channels(v: &toml::Value) -> anyhow::Result<u16> {
    if let Some(n) = v.as_integer() {
        return match n {
            1 => Ok(1),
            2 => Ok(2),
            _ => anyhow::bail!("reference_channels must be 1/2 or mono/stereo"),
        };
    }
    let Some(s) = v.as_str() else {
        anyhow::bail!("reference_channels must be 1/2 or mono/stereo");
    };
    match s.to_ascii_lowercase().as_str() {
        "mono" | "1" | "1ch" => Ok(1),
        "stereo" | "2" | "2ch" => Ok(2),
        _ => anyhow::bail!("reference_channels must be 1/2 or mono/stereo"),
    }
}

// ── 真实 AEC3 实现 ────────────────────────────────────────────────────────────
#[cfg(feature = "aec3-engine")]
struct Inner {
    apm: aec3_apm::AudioProcessing,
    far_channels: u16,
    near_buf: Vec<f32>,
    far_l: Vec<f32>,
    far_r: Vec<f32>,
    far_out_l: Vec<f32>,
    far_out_r: Vec<f32>,
    out_buf: Vec<f32>,
}

#[cfg(feature = "aec3-engine")]
impl Inner {
    fn new(tuning: &Aec3Tuning) -> Self {
        use aec3_apm::{AudioProcessing, StreamConfig};

        let builder = AudioProcessing::builder()
            .config(build_apm_config(tuning))
            // 产品默认 delay_hold=Some(true),所以这里始终显式注入 AEC3 config。
            // vendored aec3-apm 会从这份 base config 派生 stereo/multichannel 变体。
            .aec3_config(build_aec3_config(tuning));
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

#[cfg(feature = "aec3-engine")]
fn build_apm_config(tuning: &Aec3Tuning) -> aec3_apm::Config {
    use aec3_apm::config::{AdaptiveDigital, EchoCanceller, GainController2, Pipeline};

    aec3_apm::Config {
        echo_canceller: Some(EchoCanceller::default()),
        noise_suppression: None,
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
    }
}

impl Aec3Engine {
    fn apply_runtime_apm_config(&mut self) {
        #[cfg(feature = "aec3-engine")]
        {
            self.inner.apm.apply_config(build_apm_config(&self.tuning));
        }
    }
}

#[cfg(feature = "aec3-engine")]
impl Aec3Engine {
    fn process_aec3(&mut self, near: &[f32], far: &[f32], out: &mut [f32], frames: usize) {
        let mut runtime_error_count = self.last.runtime_error_count;
        let mut last_backend_error = self.last.last_backend_error.clone();
        if self.stream_delay_pending {
            if let Err(err) = self.inner.apm.set_stream_delay_ms(self.initial_delay_ms) {
                record_backend_error(
                    &mut runtime_error_count,
                    &mut last_backend_error,
                    "set_stream_delay_ms",
                    err,
                );
            }
            self.stream_delay_pending = false;
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
                self.inner.out_buf[..blk].copy_from_slice(&self.inner.near_buf[..blk]);
            } else if let Err(err) = self
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
        let aec3_delay_blocks = s
            .delay_ms
            .and_then(|delay_ms| (delay_ms >= 0).then_some(delay_ms as u32 / 4));
        self.last = ProcessorStats {
            name: "aec3",
            erle_db: s.echo_return_loss_enhancement.unwrap_or(0.0) as f32,
            residual_echo_likelihood: s.residual_echo_likelihood.unwrap_or(0.0) as f32,
            estimated_delay_ms: s.delay_ms.unwrap_or(0),
            aec3_delay_blocks,
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

#[cfg(feature = "aec3-engine")]
fn record_backend_error(
    runtime_error_count: &mut u64,
    last_backend_error: &mut Option<String>,
    stage: &str,
    err: aec3_apm::Error,
) {
    *runtime_error_count = runtime_error_count.saturating_add(1);
    *last_backend_error = Some(format!("{stage}: {err}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aec3_runtime_params_only_update_owned_apm_tuning() {
        let mut processor = Aec3Engine::new();

        assert!(processor
            .set_runtime_param("agc", &toml::Value::Boolean(true))
            .unwrap());
        assert!(!processor
            .set_runtime_param("ns", &toml::Value::Boolean(true))
            .unwrap());
        assert!(!processor
            .set_runtime_param("tail_ms", &toml::Value::Integer(120))
            .unwrap());

        assert!(processor.tuning.agc);

        #[cfg(feature = "aec3-engine")]
        assert!(build_apm_config(&processor.tuning)
            .noise_suppression
            .is_none());
    }

    #[test]
    fn aec3_tail_boundary_rejects_three_blocks_and_builds_four() {
        let mut invalid = Aec3Engine::new();
        let mut params = toml::Table::new();
        params.insert(
            "tail_ms".into(),
            toml::Value::Integer(i64::from(MIN_TAIL_MS - 1)),
        );
        assert!(invalid.configure(&params).is_err());

        let mut valid = Aec3Engine::new();
        params.insert(
            "tail_ms".into(),
            toml::Value::Integer(i64::from(MIN_TAIL_MS)),
        );
        valid.configure(&params).unwrap();
        assert_eq!(valid.tuning.tail_ms, Some(MIN_TAIL_MS));
        assert_eq!(tail_ms_to_filter_blocks(MIN_TAIL_MS), 4);

        #[cfg(feature = "aec3-engine")]
        {
            let config = build_aec3_config(&valid.tuning);
            assert_eq!(config.filter.refined.length_blocks, 4);
            assert_eq!(config.filter.coarse.length_blocks, 4);
        }
    }

    #[test]
    fn aec3_config_injects_delay_hold_by_default() {
        let tuning = Aec3Tuning::default();
        assert_eq!(tuning.delay_hold, Some(true));

        #[cfg(feature = "aec3-engine")]
        {
            let config = build_aec3_config(&tuning);
            assert!(config.delay.delay_hold);
            assert_eq!(
                config.delay.render_gate_power_threshold,
                config.render_levels.active_render_limit
            );
            assert_eq!(config.delay.render_gate_hold_blocks, 3);
        }
    }

    #[test]
    fn aec3_delay_hold_can_be_disabled_from_config() {
        let mut processor = Aec3Engine::new();
        let mut params = toml::Table::new();
        params.insert("delay_hold".into(), toml::Value::Boolean(false));

        processor.configure(&params).unwrap();

        assert_eq!(processor.tuning.delay_hold, Some(false));
        #[cfg(feature = "aec3-engine")]
        {
            let config = build_aec3_config(&processor.tuning);
            assert!(!config.delay.delay_hold);
        }
    }

    #[test]
    fn aec3_runtime_params_validate_types_and_values() {
        let mut processor = Aec3Engine::new();

        assert!(processor
            .set_runtime_param("agc", &toml::Value::Integer(1))
            .unwrap_err()
            .to_string()
            .contains("boolean"));
    }

    #[cfg(feature = "aec3-engine")]
    #[test]
    fn aec3_backend_errors_are_reported_in_stats() {
        let mut processor = Aec3Engine::new();
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
