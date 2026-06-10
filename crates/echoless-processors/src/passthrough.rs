//! Passthrough:直通节点(near→out),用于测试链路与作链尾占位。

use crate::{EchoProcessor, IoSpec, ProcessorStats};

pub struct Passthrough;

impl Passthrough {
    pub fn new() -> Self {
        Passthrough
    }
}
impl Default for Passthrough {
    fn default() -> Self {
        Self::new()
    }
}

impl EchoProcessor for Passthrough {
    fn name(&self) -> &'static str {
        "passthrough"
    }
    fn io_spec(&self) -> IoSpec {
        IoSpec {
            sample_rate: 48000,
            near_channels: 1,
            far_channels: 1,
            algorithmic_latency_ms: 0.0,
        }
    }
    fn configure(&mut self, _params: &toml::Table) -> anyhow::Result<()> {
        Ok(())
    }
    fn process(&mut self, near: &[f32], _far: &[f32], out: &mut [f32], _frames: u32) {
        let n = out.len().min(near.len());
        out[..n].copy_from_slice(&near[..n]);
        for v in out[n..].iter_mut() {
            *v = 0.0;
        }
    }
    fn stats(&self) -> ProcessorStats {
        ProcessorStats::empty("passthrough")
    }
    fn reset(&mut self) {}
}
