//! null 后端:不支持的平台/未实现路径的 fallback,start 时报错。

use std::time::Duration;

use crate::{AudioFormat, AudioSink, AudioSource, OwnedPacket};

pub struct NullSource {
    label: String,
}
impl NullSource {
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into() }
    }
}
impl AudioSource for NullSource {
    fn start(&mut self) -> anyhow::Result<AudioFormat> {
        anyhow::bail!("NullSource ({}):该平台/路径无实现", self.label)
    }
    fn read(&mut self, _timeout: Duration) -> anyhow::Result<Option<OwnedPacket>> {
        Ok(None)
    }
    fn stop(&mut self) {}
}

pub struct NullSink {
    label: String,
}
impl NullSink {
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into() }
    }
}
impl AudioSink for NullSink {
    fn start(&mut self, _format: AudioFormat) -> anyhow::Result<()> {
        anyhow::bail!("NullSink ({}):该平台/路径无实现", self.label)
    }
    fn write(&mut self, _interleaved: &[f32], _frames: u32) -> anyhow::Result<()> {
        Ok(())
    }
    fn stop(&mut self) {}
}
