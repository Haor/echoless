//! 文件后端:把 WAV 当作 `AudioSource` / `AudioSink`。
//! 用于 P1 离线评测(mic.wav + ref.wav → 处理链 → out.wav),跨平台纯 Rust。

use std::fs::File;
use std::io::BufWriter;
use std::time::Duration;

use anyhow::Context;
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};

use crate::{AudioFormat, AudioSink, AudioSource, OwnedPacket, PacketFlags};

/// 从 WAV 读取的 `AudioSource`。一次性载入内存,按固定帧数分块吐出,合成时间戳。
pub struct WavFileSource {
    format: AudioFormat,
    samples: Vec<f32>, // 全文件,交织
    pos_frames: usize,
    frames_per_read: u32,
}

impl WavFileSource {
    pub fn new(path: &str, frames_per_read: u32) -> anyhow::Result<Self> {
        let reader = WavReader::open(path).with_context(|| format!("打开 WAV 失败: {path}"))?;
        let spec = reader.spec();
        let samples: Vec<f32> = match spec.sample_format {
            SampleFormat::Float => reader
                .into_samples::<f32>()
                .map(|s| s.unwrap_or(0.0))
                .collect(),
            SampleFormat::Int => {
                let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .into_samples::<i32>()
                    .map(|s| s.unwrap_or(0) as f32 / max)
                    .collect()
            }
        };
        Ok(Self {
            format: AudioFormat {
                sample_rate: spec.sample_rate,
                channels: spec.channels,
            },
            samples,
            pos_frames: 0,
            frames_per_read: frames_per_read.max(1),
        })
    }
}

impl AudioSource for WavFileSource {
    fn start(&mut self) -> anyhow::Result<AudioFormat> {
        Ok(self.format)
    }

    fn read(&mut self, _timeout: Duration) -> anyhow::Result<Option<OwnedPacket>> {
        let ch = self.format.channels.max(1) as usize;
        let total_frames = self.samples.len() / ch;
        if self.pos_frames >= total_frames {
            return Ok(None);
        }
        let take = (self.frames_per_read as usize).min(total_frames - self.pos_frames);
        let start = self.pos_frames * ch;
        let end = start + take * ch;
        let data = self.samples[start..end].to_vec();
        let timestamp_ns =
            (self.pos_frames as u128 * 1_000_000_000u128 / self.format.sample_rate as u128) as u64;
        let device_pos = self.pos_frames as u64;
        self.pos_frames += take;
        Ok(Some(OwnedPacket {
            data,
            format: self.format,
            frames: take as u32,
            timestamp_ns,
            device_pos,
            flags: PacketFlags::empty(),
        }))
    }

    fn stop(&mut self) {}
}

/// 写出 WAV 的 `AudioSink`(float32)。
pub struct WavFileSink {
    path: String,
    writer: Option<WavWriter<BufWriter<File>>>,
}

impl WavFileSink {
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
            writer: None,
        }
    }
}

impl AudioSink for WavFileSink {
    fn start(&mut self, format: AudioFormat) -> anyhow::Result<()> {
        let spec = WavSpec {
            channels: format.channels,
            sample_rate: format.sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let w = WavWriter::create(&self.path, spec)
            .with_context(|| format!("创建 WAV 失败: {}", self.path))?;
        self.writer = Some(w);
        Ok(())
    }

    fn write(&mut self, interleaved: &[f32], frames: u32) -> anyhow::Result<()> {
        let w = self.writer.as_mut().context("WavFileSink 未 start")?;
        let _ = frames;
        for &s in interleaved {
            w.write_sample(s)?;
        }
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(w) = self.writer.take() {
            let _ = w.finalize();
        }
    }
}
