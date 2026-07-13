//! Standalone WebRTC noise suppression processor.
//!
//! The processor keeps the complete AudioProcessing module boundary so the
//! upstream filter-bank, upper-band handling, limiter, and frame state remain
//! identical to the product's former integrated AEC3 noise-suppression path.

use std::time::Instant;

use crate::{EchoProcessor, IoSpec, ProcessorStats};

const SAMPLE_RATE: u32 = 48_000;
const FRAME_SAMPLES: usize = 480;
pub const ALGORITHMIC_LATENCY_MS: f32 = 6.5;
pub const DEFAULT_LEVEL: &str = "low";

pub struct WebRtcNs {
    level: String,
    last: ProcessorStats,
    #[cfg(feature = "aec3-engine")]
    inner: Inner,
}

impl WebRtcNs {
    pub fn new() -> Self {
        let level = DEFAULT_LEVEL.to_owned();
        Self {
            #[cfg(feature = "aec3-engine")]
            inner: Inner::new(&level),
            level,
            last: ProcessorStats::empty("webrtc_ns"),
        }
    }

    fn rebuild(&mut self) {
        #[cfg(feature = "aec3-engine")]
        {
            self.inner = Inner::new(&self.level);
        }
    }

    #[cfg(feature = "aec3-engine")]
    fn process_ns(&mut self, near: &[f32], out: &mut [f32], frames: usize) {
        let mut offset = 0;
        while offset < frames {
            let block_len = (frames - offset).min(FRAME_SAMPLES);
            for index in 0..FRAME_SAMPLES {
                self.inner.input[index] = if index < block_len {
                    finite_or_zero(near.get(offset + index).copied().unwrap_or(0.0))
                } else {
                    0.0
                };
            }

            match self
                .inner
                .apm
                .process_capture_f32(&[&self.inner.input], &mut [&mut self.inner.output])
            {
                Ok(()) => {
                    self.last.last_backend_error = None;
                }
                Err(err) => {
                    self.last.runtime_error_count = self.last.runtime_error_count.saturating_add(1);
                    self.last.last_backend_error = Some(format!("process_capture_f32: {err}"));
                    self.inner.output[..block_len].copy_from_slice(&self.inner.input[..block_len]);
                }
            }

            let copy_len = block_len.min(out.len().saturating_sub(offset));
            for index in 0..copy_len {
                out[offset + index] = finite_or_zero(self.inner.output[index]);
            }
            offset += block_len;
        }
    }
}

impl Default for WebRtcNs {
    fn default() -> Self {
        Self::new()
    }
}

impl EchoProcessor for WebRtcNs {
    fn name(&self) -> &'static str {
        "webrtc_ns"
    }

    fn io_spec(&self) -> IoSpec {
        IoSpec {
            sample_rate: SAMPLE_RATE,
            near_channels: 1,
            far_channels: 1,
            algorithmic_latency_ms: ALGORITHMIC_LATENCY_MS,
        }
    }

    fn configure(&mut self, params: &toml::Table) -> anyhow::Result<()> {
        if let Some(value) = params.get("level") {
            let level = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("level must be a string"))?;
            validate_level(level)?;
            self.level = canonical_level(level).to_owned();
        }
        self.rebuild();
        Ok(())
    }

    fn set_runtime_param(&mut self, key: &str, value: &toml::Value) -> anyhow::Result<bool> {
        if key != "level" {
            return Ok(false);
        }
        let level = value
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("level must be a string"))?;
        validate_level(level)?;
        self.level = canonical_level(level).to_owned();
        self.rebuild();
        Ok(true)
    }

    fn process(&mut self, near: &[f32], _far: &[f32], out: &mut [f32], frames: u32) {
        let started = Instant::now();
        out.fill(0.0);

        #[cfg(feature = "aec3-engine")]
        self.process_ns(near, out, frames as usize);

        #[cfg(not(feature = "aec3-engine"))]
        {
            let requested = (frames as usize).min(out.len());
            for index in 0..requested {
                out[index] = finite_or_zero(near.get(index).copied().unwrap_or(0.0));
            }
        }

        self.last.process_time_ms = started.elapsed().as_secs_f32() * 1_000.0;
    }

    fn stats(&self) -> ProcessorStats {
        self.last.clone()
    }

    fn reset(&mut self) {
        self.rebuild();
    }
}

fn canonical_level(level: &str) -> &'static str {
    match level.to_ascii_lowercase().as_str() {
        "low" => "low",
        "moderate" => "moderate",
        "high" => "high",
        "veryhigh" | "very_high" | "very-high" => "veryhigh",
        _ => "",
    }
}

fn validate_level(level: &str) -> anyhow::Result<()> {
    if canonical_level(level).is_empty() {
        anyhow::bail!("level must be one of: low, moderate, high, veryhigh");
    }
    Ok(())
}

fn finite_or_zero(sample: f32) -> f32 {
    if sample.is_finite() {
        sample
    } else {
        0.0
    }
}

#[cfg(feature = "aec3-engine")]
struct Inner {
    apm: aec3_apm::AudioProcessing,
    input: Vec<f32>,
    output: Vec<f32>,
}

#[cfg(feature = "aec3-engine")]
impl Inner {
    fn new(level: &str) -> Self {
        use aec3_apm::config::NoiseSuppression;
        use aec3_apm::{AudioProcessing, Config, StreamConfig};

        let config = Config {
            noise_suppression: Some(NoiseSuppression {
                level: apm_level(level),
                ..Default::default()
            }),
            ..Default::default()
        };
        let apm = AudioProcessing::builder()
            .config(config)
            .capture_config(StreamConfig::new(SAMPLE_RATE, 1))
            .render_config(StreamConfig::new(SAMPLE_RATE, 1))
            .build();

        Self {
            apm,
            input: vec![0.0; FRAME_SAMPLES],
            output: vec![0.0; FRAME_SAMPLES],
        }
    }
}

#[cfg(feature = "aec3-engine")]
fn apm_level(level: &str) -> aec3_apm::config::NoiseSuppressionLevel {
    use aec3_apm::config::NoiseSuppressionLevel as Level;
    match canonical_level(level) {
        "low" => Level::Low,
        "high" => Level::High,
        "veryhigh" => Level::VeryHigh,
        _ => Level::Moderate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_standalone_ns_contract() {
        let processor = WebRtcNs::new();
        let spec = processor.io_spec();

        assert_eq!(processor.name(), "webrtc_ns");
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.near_channels, 1);
        assert_eq!(spec.algorithmic_latency_ms, 6.5);
        assert_eq!(processor.stats().erle_db, 0.0);
    }

    #[test]
    fn rejects_unknown_levels() {
        let mut processor = WebRtcNs::new();
        let mut params = toml::Table::new();
        params.insert("level".into(), toml::Value::String("extreme".into()));

        assert!(processor.configure(&params).is_err());
    }

    #[test]
    fn keeps_output_finite_and_frame_aligned() {
        let mut processor = WebRtcNs::new();
        let mut near = vec![0.1; FRAME_SAMPLES * 2];
        near[7] = f32::NAN;
        near[501] = f32::INFINITY;
        let far = vec![0.0; near.len()];
        let mut out = vec![1.0; near.len()];

        processor.process(&near, &far, &mut out, near.len() as u32);

        assert_eq!(out.len(), near.len());
        assert!(out.iter().all(|sample| sample.is_finite()));
        assert!(processor.stats().process_time_ms >= 0.0);
        assert_eq!(processor.stats().runtime_error_count, 0);
    }

    #[test]
    fn reset_preserves_configuration_and_processing_contract() {
        let mut processor = WebRtcNs::new();
        let mut params = toml::Table::new();
        params.insert("level".into(), toml::Value::String("high".into()));
        processor.configure(&params).unwrap();
        processor.reset();

        assert_eq!(processor.level, "high");
        assert_eq!(processor.io_spec().algorithmic_latency_ms, 6.5);
    }
}
