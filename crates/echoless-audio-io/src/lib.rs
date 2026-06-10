//! echoless-audio-io — 平台无关音频 I/O 抽象。
//!
//! 核心思想(蓝本 §1):**麦克风与 far-end reference 都是一个 `AudioSource`**;
//! 核心永远不知道一帧 reference 来自 WASAPI loopback、macOS 路由还是虚拟声卡。
//! 当前实时路径直接走 cpal;本 crate 保留通用 trait、文件后端与 null 后端,用于离线评测和未来抽回 core 的实时编排。

use std::time::Duration;

pub mod file;
pub mod null;

/// 采样格式(交织 interleaved)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

bitflags::bitflags! {
    /// WASAPI/CoreAudio 一帧的状态标志(跨平台统一)。
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct PacketFlags: u32 {
        const SILENT = 0b001;
        const DISCONTINUITY = 0b010;
        const TS_ERROR = 0b100;
    }
}

/// 一帧带时间戳的音频(交织 f32)。时间戳统一为单调纳秒(平台各自换算)。
#[derive(Clone, Debug)]
pub struct OwnedPacket {
    pub data: Vec<f32>,
    pub format: AudioFormat,
    pub frames: u32,
    pub timestamp_ns: u64,
    pub device_pos: u64,
    pub flags: PacketFlags,
}

/// 音频输入源:麦克风 与 far-end reference 都实现它。
pub trait AudioSource: Send {
    fn start(&mut self) -> anyhow::Result<AudioFormat>;
    /// 取下一帧;EOF/无更多数据返回 `Ok(None)`。
    fn read(&mut self, timeout: Duration) -> anyhow::Result<Option<OwnedPacket>>;
    fn stop(&mut self);
}

/// 音频输出汇:写入用户选择的输出设备,通常是 VB-Cable/BlackHole 等外部虚拟设备。
pub trait AudioSink: Send {
    fn start(&mut self, format: AudioFormat) -> anyhow::Result<()>;
    fn write(&mut self, interleaved: &[f32], frames: u32) -> anyhow::Result<()>;
    fn stop(&mut self);
}

// 让 `Box<dyn AudioSource/Sink>` 也满足 trait,可直接喂泛型管线。
impl AudioSource for Box<dyn AudioSource> {
    fn start(&mut self) -> anyhow::Result<AudioFormat> {
        (**self).start()
    }
    fn read(&mut self, timeout: Duration) -> anyhow::Result<Option<OwnedPacket>> {
        (**self).read(timeout)
    }
    fn stop(&mut self) {
        (**self).stop()
    }
}

impl AudioSink for Box<dyn AudioSink> {
    fn start(&mut self, format: AudioFormat) -> anyhow::Result<()> {
        (**self).start(format)
    }
    fn write(&mut self, interleaved: &[f32], frames: u32) -> anyhow::Result<()> {
        (**self).write(interleaved, frames)
    }
    fn stop(&mut self) {
        (**self).stop()
    }
}

/// 单调时钟。当前默认实现用 std Instant;未来如需平台时基可在这里接入。
pub trait MonotonicClock: Send + Sync {
    fn now_ns(&self) -> u64;
}

/// std 实现的单调时钟(占位;实时路径后续换平台原生时基)。
pub struct StdClock {
    base: std::time::Instant,
}
impl StdClock {
    pub fn new() -> Self {
        Self {
            base: std::time::Instant::now(),
        }
    }
}
impl Default for StdClock {
    fn default() -> Self {
        Self::new()
    }
}
impl MonotonicClock for StdClock {
    fn now_ns(&self) -> u64 {
        self.base.elapsed().as_nanos() as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Microphone,
    SystemAudio,
    Output,
}

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
}
