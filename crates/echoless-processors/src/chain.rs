//! ProcessorChain:把若干 `EchoProcessor` 串成链。
//!
//! 负责:① 相邻节点间的采样率/声道适配(边界 SRC + downmix);② 把真实 far ref 分发到各节点域;
//! ③ 延迟累计。单开 = 长度 1 的链;串联/组合 = 有序节点列表;空链 = 直通。
//!
//! 节点边界 SRC 使用 rubato 同步 FFT resampler,并在 chain 生命周期内复用 scratch buffer。

use crate::{dsp::copy_or_zero, registry, EchoProcessor, NodeConfig, ProcessorStats};
use rubato::{FftFixedIn, Resampler};

pub struct ProcessorChain {
    base_rate: u32,
    base_far_channels: u16,
    nodes: Vec<Box<dyn EchoProcessor>>,
    adapters: Vec<NodeAdapters>,
    cur_near_base: Vec<f32>,
}

impl ProcessorChain {
    pub fn new(base_rate: u32, base_far_channels: u16) -> Self {
        Self {
            base_rate,
            base_far_channels: base_far_channels.max(1),
            nodes: Vec::new(),
            adapters: Vec::new(),
            cur_near_base: Vec::new(),
        }
    }

    pub fn push(&mut self, p: Box<dyn EchoProcessor>) {
        let spec = p.io_spec();
        self.adapters.push(NodeAdapters::new(
            self.base_rate,
            self.base_far_channels,
            spec,
        ));
        self.nodes.push(p);
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.nodes.iter().map(|n| n.name()).collect()
    }

    /// 算法延迟累计:节点自报 io_spec 延迟 + 节点边界 SRC(rubato)在主信号路径
    /// (near_in 进节点、near_out 回 base)上引入的重采样延迟。far_in 是并行参考
    /// 路径,其延迟影响 AEC 对齐而非 mouth-to-ear 输出延迟,故不计入此处。
    ///
    /// 注意:边界 SRC 的 resampler 在首帧 `process`/`warm_up` 后才建立,在此之前调用
    /// 本方法只会得到节点自报延迟(SRC 项为 0)。实时路径在启动前调用 `warm_up` 预热,
    /// 以保证此值已含 SRC 延迟。
    pub fn total_latency_ms(&self) -> f32 {
        self.nodes
            .iter()
            .zip(self.adapters.iter())
            .map(|(node, adapter)| {
                node.io_spec().algorithmic_latency_ms
                    + adapter.near_in.latency_ms()
                    + adapter.near_out.latency_ms()
            })
            .sum()
    }

    pub fn stats(&self) -> Vec<ProcessorStats> {
        self.nodes.iter().map(|n| n.stats()).collect()
    }

    pub fn reset(&mut self) {
        for n in self.nodes.iter_mut() {
            n.reset();
        }
        for adapter in self.adapters.iter_mut() {
            adapter.reset();
        }
    }

    /// 用一帧静音预跑整链,促使各节点边界的 rubato resampler 按 `frames` 尺寸建立,
    /// 之后 `total_latency_ms()` 才能反映边界 SRC 延迟。预热后 `reset()` 清除预热引入
    /// 的节点/缓冲状态,但保留已建立的 resampler 实例(及其固定延迟)。
    pub fn warm_up(&mut self, frames: usize) {
        if self.nodes.is_empty() || frames == 0 {
            return;
        }
        let near = vec![0.0f32; frames];
        let far = vec![0.0f32; frames * self.base_far_channels as usize];
        let mut out = vec![0.0f32; frames];
        self.process(&near, &far, &mut out, frames as u32);
        self.reset();
    }

    pub fn set_stream_delay_ms(&mut self, ms: i32) {
        for node in self.nodes.iter_mut() {
            node.set_stream_delay_ms(ms);
        }
    }

    pub fn set_runtime_param(
        &mut self,
        node_name: &str,
        key: &str,
        value: &toml::Value,
    ) -> anyhow::Result<usize> {
        let mut applied = 0;
        for node in self.nodes.iter_mut() {
            if node.name() == node_name && node.set_runtime_param(key, value)? {
                applied += 1;
            }
        }
        Ok(applied)
    }

    /// near = 原始 mic(base_rate,mono);far = 真实 ref(base_rate,base_far_channels);
    /// out = 链尾(base_rate,mono),长度应 = frames。
    pub fn process(
        &mut self,
        near_base_mono: &[f32],
        far_base: &[f32],
        out_base_mono: &mut [f32],
        _frames: u32,
    ) {
        if self.nodes.is_empty() {
            copy_or_zero(near_base_mono, out_base_mono);
            return;
        }
        self.cur_near_base.clear();
        self.cur_near_base.extend_from_slice(near_base_mono);
        for (node, adapter) in self.nodes.iter_mut().zip(self.adapters.iter_mut()) {
            let spec = adapter.spec;
            let near_n = adapter.near_in.adapt(&self.cur_near_base);
            let far_n = adapter.far_in.adapt(far_base);
            let nc = spec.near_channels.max(1) as usize;
            let node_frames = (near_n.len() / nc) as u32;
            adapter.out_n.resize(near_n.len(), 0.0);
            node.process(near_n, far_n, &mut adapter.out_n, node_frames);
            // 回到 base 域(mono),作为下一级 near
            let out_base = adapter.near_out.adapt(&adapter.out_n);
            self.cur_near_base.clear();
            self.cur_near_base.extend_from_slice(out_base);
        }
        copy_or_zero(&self.cur_near_base, out_base_mono);
    }
}

/// 从配置构链。空 nodes = 直通链。
pub fn chain_from_nodes(
    nodes: &[NodeConfig],
    base_rate: u32,
    base_far_channels: u16,
) -> anyhow::Result<ProcessorChain> {
    let mut chain = ProcessorChain::new(base_rate, base_far_channels);
    for n in nodes {
        let mut p = registry::build(&n.kind)?;
        p.configure(&n.params)?;
        chain.push(p);
    }
    Ok(chain)
}

struct NodeAdapters {
    spec: crate::IoSpec,
    near_in: BoundaryAdapter,
    far_in: BoundaryAdapter,
    near_out: BoundaryAdapter,
    out_n: Vec<f32>,
}

impl NodeAdapters {
    fn new(base_rate: u32, base_far_channels: u16, spec: crate::IoSpec) -> Self {
        Self {
            spec,
            near_in: BoundaryAdapter::new(base_rate, 1, spec.sample_rate, spec.near_channels),
            far_in: BoundaryAdapter::new(
                base_rate,
                base_far_channels,
                spec.sample_rate,
                spec.far_channels,
            ),
            near_out: BoundaryAdapter::new(spec.sample_rate, spec.near_channels, base_rate, 1),
            out_n: Vec::new(),
        }
    }

    fn reset(&mut self) {
        self.near_in.reset();
        self.far_in.reset();
        self.near_out.reset();
        self.out_n.clear();
    }
}

struct BoundaryAdapter {
    in_rate: u32,
    in_channels: u16,
    out_rate: u32,
    out_channels: u16,
    configured_input_frames: usize,
    planes: Vec<Vec<f32>>,
    resampled_planes: Vec<Vec<f32>>,
    output: Vec<f32>,
    resampler: Option<FftFixedIn<f32>>,
}

impl BoundaryAdapter {
    fn new(in_rate: u32, in_channels: u16, out_rate: u32, out_channels: u16) -> Self {
        Self {
            in_rate,
            in_channels: in_channels.max(1),
            out_rate,
            out_channels: out_channels.max(1),
            configured_input_frames: 0,
            planes: Vec::new(),
            resampled_planes: Vec::new(),
            output: Vec::new(),
            resampler: None,
        }
    }

    fn reset(&mut self) {
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.reset();
        }
        for plane in self.planes.iter_mut() {
            plane.fill(0.0);
        }
        for plane in self.resampled_planes.iter_mut() {
            plane.fill(0.0);
        }
        self.output.clear();
    }

    /// 本边界 SRC 引入的延迟(ms)。rubato `FftFixedIn` 的 `output_delay()` 以输出侧
    /// 帧数计;同采样率(无 resampler)或 resampler 尚未建立时为 0。
    fn latency_ms(&self) -> f32 {
        match self.resampler.as_ref() {
            Some(resampler) if self.out_rate > 0 => {
                resampler.output_delay() as f32 / self.out_rate as f32 * 1000.0
            }
            _ => 0.0,
        }
    }

    fn adapt(&mut self, input: &[f32]) -> &[f32] {
        let input_frames = input.len() / self.in_channels as usize;
        self.ensure_layout(input_frames);
        self.fill_planes(input, input_frames);

        let output_frames = if self.in_rate == self.out_rate {
            self.interleave_from_planes(input_frames);
            input_frames
        } else if let Some(resampler) = self.resampler.as_mut() {
            match resampler.process_into_buffer(&self.planes, &mut self.resampled_planes, None) {
                Ok((_in_frames, out_frames)) => {
                    self.interleave_from_resampled(out_frames);
                    out_frames
                }
                Err(_err) => {
                    self.resample_linear_fallback(input_frames);
                    self.output.len() / self.out_channels as usize
                }
            }
        } else {
            self.resample_linear_fallback(input_frames);
            self.output.len() / self.out_channels as usize
        };
        let output_len = output_frames * self.out_channels as usize;
        &self.output[..output_len.min(self.output.len())]
    }

    fn ensure_layout(&mut self, input_frames: usize) {
        if self.configured_input_frames == input_frames {
            return;
        }
        self.configured_input_frames = input_frames;
        let out_channels = self.out_channels as usize;
        resize_planes(&mut self.planes, out_channels, input_frames);
        if self.in_rate != self.out_rate && input_frames > 0 {
            self.resampler = FftFixedIn::<f32>::new(
                self.in_rate as usize,
                self.out_rate as usize,
                input_frames,
                1,
                out_channels,
            )
            .ok();
            let output_frames_max = self
                .resampler
                .as_ref()
                .map(Resampler::output_frames_max)
                .unwrap_or_else(|| scaled_frames(input_frames, self.in_rate, self.out_rate));
            resize_planes(&mut self.resampled_planes, out_channels, output_frames_max);
            self.output.resize(output_frames_max * out_channels, 0.0);
        } else {
            self.resampler = None;
            self.resampled_planes.clear();
            self.output.resize(input_frames * out_channels, 0.0);
        }
    }

    fn fill_planes(&mut self, input: &[f32], frames: usize) {
        let in_channels = self.in_channels as usize;
        let out_channels = self.out_channels as usize;
        for plane in self.planes.iter_mut() {
            plane[..frames].fill(0.0);
        }
        for frame in 0..frames {
            let frame_start = frame * in_channels;
            let frame_samples = &input[frame_start..frame_start + in_channels];
            if out_channels == in_channels {
                for (channel, sample) in frame_samples.iter().enumerate().take(out_channels) {
                    self.planes[channel][frame] = *sample;
                }
            } else if out_channels == 1 {
                self.planes[0][frame] =
                    frame_samples.iter().copied().sum::<f32>() / in_channels as f32;
            } else if in_channels == 1 {
                let sample = frame_samples[0];
                for channel in 0..out_channels {
                    self.planes[channel][frame] = sample;
                }
            } else {
                for channel in 0..out_channels {
                    self.planes[channel][frame] = frame_samples[channel.min(in_channels - 1)];
                }
            }
        }
    }

    fn interleave_from_planes(&mut self, frames: usize) {
        let channels = self.out_channels as usize;
        self.output.resize(frames * channels, 0.0);
        for frame in 0..frames {
            for channel in 0..channels {
                self.output[frame * channels + channel] = self.planes[channel][frame];
            }
        }
    }

    fn interleave_from_resampled(&mut self, frames: usize) {
        let channels = self.out_channels as usize;
        self.output.resize(frames * channels, 0.0);
        for frame in 0..frames {
            for channel in 0..channels {
                self.output[frame * channels + channel] = self.resampled_planes[channel][frame];
            }
        }
    }

    fn resample_linear_fallback(&mut self, frames: usize) {
        let channels = self.out_channels as usize;
        let output_frames = scaled_frames(frames, self.in_rate, self.out_rate);
        self.output.resize(output_frames * channels, 0.0);
        for channel in 0..channels {
            for frame in 0..output_frames {
                let src = frame as f64 * self.in_rate as f64 / self.out_rate as f64;
                let i0 = src.floor() as usize;
                let frac = (src - i0 as f64) as f32;
                let a = self.planes[channel].get(i0).copied().unwrap_or(0.0);
                let b = self.planes[channel].get(i0 + 1).copied().unwrap_or(a);
                self.output[frame * channels + channel] = a + (b - a) * frac;
            }
        }
    }
}

fn resize_planes(planes: &mut Vec<Vec<f32>>, channels: usize, frames: usize) {
    if planes.len() != channels {
        planes.resize_with(channels, Vec::new);
    }
    for plane in planes.iter_mut() {
        plane.resize(frames, 0.0);
    }
}

fn scaled_frames(input_frames: usize, in_rate: u32, out_rate: u32) -> usize {
    if in_rate == 0 {
        return input_frames;
    }
    ((input_frames as f64) * out_rate as f64 / in_rate as f64).round() as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IoSpec;
    use std::sync::{Arc, Mutex};

    struct IdentityNode {
        spec: IoSpec,
    }

    impl EchoProcessor for IdentityNode {
        fn name(&self) -> &'static str {
            "identity"
        }

        fn io_spec(&self) -> IoSpec {
            self.spec
        }

        fn configure(&mut self, _params: &toml::Table) -> anyhow::Result<()> {
            Ok(())
        }

        fn process(&mut self, near: &[f32], _far: &[f32], out: &mut [f32], _frames: u32) {
            copy_or_zero(near, out);
        }

        fn stats(&self) -> ProcessorStats {
            ProcessorStats::empty("identity")
        }

        fn reset(&mut self) {}
    }

    struct CaptureFarNode {
        spec: IoSpec,
        far_seen: Arc<Mutex<Vec<f32>>>,
    }

    impl EchoProcessor for CaptureFarNode {
        fn name(&self) -> &'static str {
            "capture_far"
        }

        fn io_spec(&self) -> IoSpec {
            self.spec
        }

        fn configure(&mut self, _params: &toml::Table) -> anyhow::Result<()> {
            Ok(())
        }

        fn process(&mut self, near: &[f32], far: &[f32], out: &mut [f32], _frames: u32) {
            *self.far_seen.lock().unwrap() = far.to_vec();
            copy_or_zero(near, out);
        }

        fn stats(&self) -> ProcessorStats {
            ProcessorStats::empty("capture_far")
        }

        fn reset(&mut self) {}
    }

    struct CaptureDelayNode {
        delays: Arc<Mutex<Vec<i32>>>,
    }

    impl EchoProcessor for CaptureDelayNode {
        fn name(&self) -> &'static str {
            "capture_delay"
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

        fn set_stream_delay_ms(&mut self, ms: i32) {
            self.delays.lock().unwrap().push(ms);
        }

        fn process(&mut self, near: &[f32], _far: &[f32], out: &mut [f32], _frames: u32) {
            copy_or_zero(near, out);
        }

        fn stats(&self) -> ProcessorStats {
            ProcessorStats::empty("capture_delay")
        }

        fn reset(&mut self) {}
    }

    struct CaptureParamNode {
        seen: Arc<Mutex<Vec<(String, toml::Value)>>>,
    }

    impl EchoProcessor for CaptureParamNode {
        fn name(&self) -> &'static str {
            "capture_param"
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

        fn set_runtime_param(&mut self, key: &str, value: &toml::Value) -> anyhow::Result<bool> {
            self.seen
                .lock()
                .unwrap()
                .push((key.to_string(), value.clone()));
            Ok(true)
        }

        fn process(&mut self, near: &[f32], _far: &[f32], out: &mut [f32], _frames: u32) {
            copy_or_zero(near, out);
        }

        fn stats(&self) -> ProcessorStats {
            ProcessorStats::empty("capture_param")
        }

        fn reset(&mut self) {}
    }

    #[test]
    fn chain_resamples_through_16k_node_and_preserves_output_length() {
        let mut chain = ProcessorChain::new(48_000, 1);
        chain.push(Box::new(IdentityNode {
            spec: IoSpec {
                sample_rate: 16_000,
                near_channels: 1,
                far_channels: 1,
                algorithmic_latency_ms: 0.0,
            },
        }));

        let near = sine_block(480, 440.0, 48_000);
        let far = vec![0.0; 480];
        let mut out = vec![0.0; 480];

        chain.process(&near, &far, &mut out, 480);

        assert_eq!(out.len(), 480);
        assert!(out.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn total_latency_includes_boundary_src_delay_after_warmup() {
        let mut chain = ProcessorChain::new(48_000, 1);
        chain.push(Box::new(IdentityNode {
            spec: IoSpec {
                sample_rate: 16_000,
                near_channels: 1,
                far_channels: 1,
                algorithmic_latency_ms: 0.0,
            },
        }));

        // 预热前:resampler 未建,只有节点自报延迟(此处为 0)。
        assert_eq!(chain.total_latency_ms(), 0.0);

        chain.warm_up(480);

        // 预热后:near_in(48k→16k)+ near_out(16k→48k)的 rubato 延迟应被计入。
        let latency = chain.total_latency_ms();
        assert!(
            latency > 0.0,
            "boundary SRC delay was not accounted for: {latency}"
        );
    }

    #[test]
    fn chain_resampler_preserves_block_boundary_continuity() {
        use std::f32::consts::TAU;

        let mut chain = ProcessorChain::new(48_000, 1);
        chain.push(Box::new(IdentityNode {
            spec: IoSpec {
                sample_rate: 16_000,
                near_channels: 1,
                far_channels: 1,
                algorithmic_latency_ms: 0.0,
            },
        }));

        // 把一段连续 440Hz 正弦切成多块逐块送入;有状态 SRC 应让块边界保持相位连续。
        let frames = 480usize;
        let blocks = 12usize;
        let mut full_out = Vec::with_capacity(frames * blocks);
        let far = vec![0.0; frames];
        for block in 0..blocks {
            let near: Vec<f32> = (0..frames)
                .map(|i| {
                    let n = (block * frames + i) as f32;
                    0.2 * (n * 440.0 * TAU / 48_000.0).sin()
                })
                .collect();
            let mut out = vec![0.0; frames];
            chain.process(&near, &far, &mut out, frames as u32);
            full_out.extend_from_slice(&out);
        }

        // 跳过前 4 块暖机,稳态段相邻样本差应无块边界台阶。
        // 440Hz@48k 正弦单样本最大斜率 ≈ 0.2*2π*440/48000 ≈ 0.0115;边界台阶会远超此值。
        let steady = &full_out[4 * frames..];
        let max_step = steady
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_step < 0.05,
            "block boundary discontinuity detected: max_step={max_step}"
        );
        assert!(steady.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn chain_reuses_adapter_capacity_after_warmup() {
        let mut chain = ProcessorChain::new(48_000, 1);
        chain.push(Box::new(IdentityNode {
            spec: IoSpec {
                sample_rate: 16_000,
                near_channels: 1,
                far_channels: 1,
                algorithmic_latency_ms: 0.0,
            },
        }));

        let near = sine_block(480, 440.0, 48_000);
        let far = vec![0.0; 480];
        let mut out = vec![0.0; 480];
        chain.process(&near, &far, &mut out, 480);
        let warm = capacity_signature(&chain);

        chain.process(&near, &far, &mut out, 480);

        assert_eq!(capacity_signature(&chain), warm);
    }

    #[test]
    fn stereo_far_resampling_preserves_channel_difference() {
        let far_seen = Arc::new(Mutex::new(Vec::new()));
        let mut chain = ProcessorChain::new(48_000, 2);
        chain.push(Box::new(CaptureFarNode {
            spec: IoSpec {
                sample_rate: 16_000,
                near_channels: 1,
                far_channels: 2,
                algorithmic_latency_ms: 0.0,
            },
            far_seen: far_seen.clone(),
        }));

        let near = vec![0.0; 480];
        let mut far = vec![0.0; 480 * 2];
        for frame in 0..480 {
            far[frame * 2] = 0.25;
            far[frame * 2 + 1] = -0.75;
        }
        let mut out = vec![0.0; 480];

        chain.process(&near, &far, &mut out, 480);

        let captured = far_seen.lock().unwrap().clone();
        assert_eq!(captured.len(), 160 * 2);
        let (mut left_sum, mut right_sum) = (0.0f32, 0.0f32);
        for frame in captured.chunks_exact(2).skip(8).take(144) {
            left_sum += frame[0];
            right_sum += frame[1];
        }
        let left_avg = left_sum / 144.0;
        let right_avg = right_sum / 144.0;
        assert!(left_avg > 0.1, "left channel collapsed: {left_avg}");
        assert!(right_avg < -0.3, "right channel collapsed: {right_avg}");
        assert!(
            (left_avg - right_avg).abs() > 0.4,
            "stereo channels were not preserved: L={left_avg}, R={right_avg}"
        );
    }

    #[test]
    fn boundary_adapter_downmixes_stereo_to_mono() {
        let mut adapter = BoundaryAdapter::new(48_000, 2, 48_000, 1);

        let output = adapter.adapt(&[1.0, 3.0, -1.0, 1.0]);

        assert_eq!(output, &[2.0, 0.0]);
    }

    #[test]
    fn boundary_adapter_spreads_mono_to_stereo() {
        let mut adapter = BoundaryAdapter::new(48_000, 1, 48_000, 2);

        let output = adapter.adapt(&[0.25, -0.5]);

        assert_eq!(output, &[0.25, 0.25, -0.5, -0.5]);
    }

    #[test]
    fn chain_forwards_stream_delay_to_nodes() {
        let delays = Arc::new(Mutex::new(Vec::new()));
        let mut chain = ProcessorChain::new(48_000, 1);
        chain.push(Box::new(CaptureDelayNode {
            delays: delays.clone(),
        }));

        chain.set_stream_delay_ms(25);
        chain.set_stream_delay_ms(0);

        assert_eq!(*delays.lock().unwrap(), vec![25, 0]);
    }

    #[test]
    fn chain_forwards_runtime_params_to_named_nodes() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut chain = ProcessorChain::new(48_000, 1);
        chain.push(Box::new(CaptureParamNode { seen: seen.clone() }));

        let value = toml::Value::Boolean(true);
        let applied = chain
            .set_runtime_param("capture_param", "ns", &value)
            .unwrap();
        let missed = chain.set_runtime_param("missing", "ns", &value).unwrap();

        assert_eq!(applied, 1);
        assert_eq!(missed, 0);
        assert_eq!(
            *seen.lock().unwrap(),
            vec![("ns".to_string(), toml::Value::Boolean(true))]
        );
    }

    fn capacity_signature(chain: &ProcessorChain) -> Vec<usize> {
        let mut caps = vec![chain.cur_near_base.capacity()];
        for adapter in &chain.adapters {
            caps.extend(boundary_capacity_signature(&adapter.near_in));
            caps.extend(boundary_capacity_signature(&adapter.far_in));
            caps.extend(boundary_capacity_signature(&adapter.near_out));
            caps.push(adapter.out_n.capacity());
        }
        caps
    }

    fn boundary_capacity_signature(adapter: &BoundaryAdapter) -> Vec<usize> {
        let mut caps = vec![adapter.output.capacity(), adapter.planes.capacity()];
        caps.extend(adapter.planes.iter().map(Vec::capacity));
        caps.push(adapter.resampled_planes.capacity());
        caps.extend(adapter.resampled_planes.iter().map(Vec::capacity));
        caps
    }

    fn sine_block(frames: usize, hz: f32, sample_rate: u32) -> Vec<f32> {
        (0..frames)
            .map(|frame| {
                let phase = frame as f32 * hz * std::f32::consts::TAU / sample_rate as f32;
                0.1 * phase.sin()
            })
            .collect()
    }
}
