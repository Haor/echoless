//! ProcessorChain:把若干 `EchoProcessor` 串成链。
//!
//! 负责:① 相邻节点间的采样率/声道适配(边界 SRC + downmix);② 把真实 far ref 分发到各节点域;
//! ③ 延迟累计。单开 = 长度 1 的链;串联/组合 = 有序节点列表;空链 = 直通。
//!
//! ⚠️ 骨架阶段边界 SRC 用占位**线性重采样**(每块独立,块边界有微伪影)+ downmix-then-spread。
//! 正式实现替换为 rubato 有状态 SRC + 立体声保留(蓝本 §10 / 主文档 §8.3)。

use crate::{registry, EchoProcessor, NodeConfig, ProcessorStats};

pub struct ProcessorChain {
    base_rate: u32,
    base_far_channels: u16,
    nodes: Vec<Box<dyn EchoProcessor>>,
}

impl ProcessorChain {
    pub fn new(base_rate: u32, base_far_channels: u16) -> Self {
        Self {
            base_rate,
            base_far_channels: base_far_channels.max(1),
            nodes: Vec::new(),
        }
    }

    pub fn push(&mut self, p: Box<dyn EchoProcessor>) {
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

    /// 算法延迟累计(节点 io_spec;边界 SRC 延迟待 rubato 实现后补)。
    pub fn total_latency_ms(&self) -> f32 {
        self.nodes
            .iter()
            .map(|n| n.io_spec().algorithmic_latency_ms)
            .sum()
    }

    pub fn stats(&self) -> Vec<ProcessorStats> {
        self.nodes.iter().map(|n| n.stats()).collect()
    }

    pub fn reset(&mut self) {
        for n in self.nodes.iter_mut() {
            n.reset();
        }
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
            copy_into(near_base_mono, out_base_mono);
            return;
        }
        let mut cur_near = near_base_mono.to_vec(); // base_rate, mono
        for node in self.nodes.iter_mut() {
            let spec = node.io_spec();
            let near_n = adapt(
                &cur_near,
                self.base_rate,
                1,
                spec.sample_rate,
                spec.near_channels,
            );
            let far_n = adapt(
                far_base,
                self.base_rate,
                self.base_far_channels,
                spec.sample_rate,
                spec.far_channels,
            );
            let nc = spec.near_channels.max(1) as usize;
            let node_frames = (near_n.len() / nc) as u32;
            let mut out_n = vec![0f32; near_n.len()];
            node.process(&near_n, &far_n, &mut out_n, node_frames);
            // 回到 base 域(mono),作为下一级 near
            cur_near = adapt(
                &out_n,
                spec.sample_rate,
                spec.near_channels,
                self.base_rate,
                1,
            );
        }
        copy_into(&cur_near, out_base_mono);
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

fn copy_into(src: &[f32], dst: &mut [f32]) {
    let n = dst.len().min(src.len());
    dst[..n].copy_from_slice(&src[..n]);
    for v in dst[n..].iter_mut() {
        *v = 0.0;
    }
}

/// 采样率 + 声道适配(占位实现)。
fn adapt(input: &[f32], in_rate: u32, in_ch: u16, out_rate: u32, out_ch: u16) -> Vec<f32> {
    let remapped = remap_channels(input, in_ch, out_ch);
    if in_rate == out_rate {
        return remapped;
    }
    let chs = out_ch.max(1) as usize;
    let frames = remapped.len() / chs;
    // 逐声道线性重采样(占位;TODO: rubato 有状态 SRC)
    let mut out = Vec::new();
    let mut planes: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); chs];
    for f in 0..frames {
        for c in 0..chs {
            planes[c].push(remapped[f * chs + c]);
        }
    }
    let resampled: Vec<Vec<f32>> = planes
        .iter()
        .map(|p| resample_linear(p, in_rate, out_rate))
        .collect();
    let out_frames = resampled.first().map(|p| p.len()).unwrap_or(0);
    out.reserve(out_frames * chs);
    for f in 0..out_frames {
        for plane in resampled.iter() {
            out.push(plane.get(f).copied().unwrap_or(0.0));
        }
    }
    out
}

/// 声道适配(占位):同数直通;降维取平均;升维复制平均值。
/// ⚠️ TODO: 立体声 far 应保留 L/R(蓝本 §7;主文档 §8.3),勿 downmix 后复制。
fn remap_channels(input: &[f32], in_ch: u16, out_ch: u16) -> Vec<f32> {
    if in_ch == out_ch || in_ch == 0 {
        return input.to_vec();
    }
    let inc = in_ch as usize;
    let outc = out_ch.max(1) as usize;
    let frames = input.len() / inc;
    let mut out = Vec::with_capacity(frames * outc);
    for f in 0..frames {
        let frame = &input[f * inc..(f + 1) * inc];
        let mono = frame.iter().copied().sum::<f32>() / inc as f32;
        for _ in 0..outc {
            out.push(mono);
        }
    }
    out
}

/// 线性插值重采样(单声道;占位实现)。
fn resample_linear(input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    if in_rate == out_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = out_rate as f64 / in_rate as f64;
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 / ratio;
        let i0 = src.floor() as usize;
        let frac = (src - i0 as f64) as f32;
        let a = input.get(i0).copied().unwrap_or(0.0);
        let b = input.get(i0 + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }
    out
}
