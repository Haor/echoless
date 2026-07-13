//! Standalone processor backed by a pinned Xiph RNNoise runtime and model.

use std::ffi::{c_float, c_int, c_void};
use std::ptr::NonNull;
use std::time::Instant;

use anyhow::{ensure, Context};

use crate::{EchoProcessor, IoSpec, ProcessorStats};

const SAMPLE_RATE: u32 = 48_000;
const FRAME_SAMPLES: usize = 480;
const PCM_SCALE: f32 = i16::MAX as f32;
const MODEL_LEN: usize = include_bytes!("../vendor/rnnoise/model/weights_blob.bin").len();
pub const ALGORITHMIC_LATENCY_MS: f32 = 10.0;

#[repr(align(64))]
struct AlignedModel([u8; MODEL_LEN]);

static MODEL_BYTES: AlignedModel =
    AlignedModel(*include_bytes!("../vendor/rnnoise/model/weights_blob.bin"));

#[repr(C)]
struct DenoiseState {
    _private: [u8; 0],
}

#[repr(C)]
struct RnnModel {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn rnnoise_get_frame_size() -> c_int;
    fn rnnoise_init(state: *mut DenoiseState, model: *mut RnnModel) -> c_int;
    fn rnnoise_create(model: *mut RnnModel) -> *mut DenoiseState;
    fn rnnoise_destroy(state: *mut DenoiseState);
    fn rnnoise_process_frame(
        state: *mut DenoiseState,
        output: *mut c_float,
        input: *const c_float,
    ) -> c_float;
    fn rnnoise_model_from_buffer(data: *const c_void, len: c_int) -> *mut RnnModel;
    fn rnnoise_model_free(model: *mut RnnModel);
}

struct NativeRnNoise {
    state: NonNull<DenoiseState>,
    model: NonNull<RnnModel>,
}

impl NativeRnNoise {
    fn new() -> anyhow::Result<Self> {
        ensure!(
            unsafe { rnnoise_get_frame_size() } == FRAME_SAMPLES as c_int,
            "vendored RNNoise frame size does not match the 10 ms processor contract"
        );

        let model = NonNull::new(unsafe {
            rnnoise_model_from_buffer(MODEL_BYTES.0.as_ptr().cast(), MODEL_LEN as c_int)
        })
        .context("failed to load the embedded RNNoise model")?;

        let state = match NonNull::new(unsafe { rnnoise_create(model.as_ptr()) }) {
            Some(state) => state,
            None => {
                unsafe { rnnoise_model_free(model.as_ptr()) };
                anyhow::bail!("failed to initialize the embedded RNNoise model");
            }
        };

        Ok(Self { state, model })
    }

    fn process(&mut self, output: &mut [f32; FRAME_SAMPLES], input: &[f32; FRAME_SAMPLES]) {
        unsafe {
            rnnoise_process_frame(self.state.as_ptr(), output.as_mut_ptr(), input.as_ptr());
        }
    }

    fn reset(&mut self) -> bool {
        unsafe { rnnoise_init(self.state.as_ptr(), self.model.as_ptr()) == 0 }
    }
}

// The native state has exclusive ownership and is only accessed through `&mut self`.
unsafe impl Send for NativeRnNoise {}

impl Drop for NativeRnNoise {
    fn drop(&mut self) {
        unsafe {
            rnnoise_destroy(self.state.as_ptr());
            rnnoise_model_free(self.model.as_ptr());
        }
    }
}

pub struct RnNoise {
    native: NativeRnNoise,
    input: [f32; FRAME_SAMPLES],
    output: [f32; FRAME_SAMPLES],
    last: ProcessorStats,
}

impl RnNoise {
    pub fn try_new() -> anyhow::Result<Self> {
        let mut processor = Self {
            native: NativeRnNoise::new()?,
            input: [0.0; FRAME_SAMPLES],
            output: [0.0; FRAME_SAMPLES],
            last: ProcessorStats::empty("rnnoise"),
        };
        processor.prime_state();
        Ok(processor)
    }

    fn prime_state(&mut self) {
        self.input.fill(0.0);
        self.output.fill(0.0);
        self.native.process(&mut self.output, &self.input);
    }
}

impl EchoProcessor for RnNoise {
    fn name(&self) -> &'static str {
        "rnnoise"
    }

    fn io_spec(&self) -> IoSpec {
        IoSpec {
            sample_rate: SAMPLE_RATE,
            near_channels: 1,
            far_channels: 1,
            algorithmic_latency_ms: ALGORITHMIC_LATENCY_MS,
        }
    }

    fn configure(&mut self, _params: &toml::Table) -> anyhow::Result<()> {
        self.reset();
        ensure!(
            self.last.runtime_error_count == 0,
            "failed to reset RNNoise state"
        );
        Ok(())
    }

    fn process(&mut self, near: &[f32], _far: &[f32], out: &mut [f32], frames: u32) {
        let started = Instant::now();
        out.fill(0.0);

        let mut offset = 0;
        let requested_frames = frames as usize;
        while offset < requested_frames {
            let block_len = (requested_frames - offset).min(FRAME_SAMPLES);
            for index in 0..FRAME_SAMPLES {
                let normalized = if index < block_len {
                    near.get(offset + index)
                        .copied()
                        .map(finite_or_zero)
                        .unwrap_or(0.0)
                        .clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                self.input[index] = normalized * PCM_SCALE;
            }

            self.native.process(&mut self.output, &self.input);

            let copy_len = block_len.min(out.len().saturating_sub(offset));
            for index in 0..copy_len {
                out[offset + index] =
                    finite_or_zero(self.output[index] / PCM_SCALE).clamp(-1.0, 1.0);
            }
            offset += block_len;
        }

        self.last.process_time_ms = started.elapsed().as_secs_f32() * 1_000.0;
    }

    fn stats(&self) -> ProcessorStats {
        self.last.clone()
    }

    fn reset(&mut self) {
        if self.native.reset() {
            self.last.runtime_error_count = 0;
            self.last.last_backend_error = None;
            self.prime_state();
        } else {
            self.last.runtime_error_count = self.last.runtime_error_count.saturating_add(1);
            self.last.last_backend_error = Some("failed to reset RNNoise state".to_string());
        }
    }
}

fn finite_or_zero(sample: f32) -> f32 {
    if sample.is_finite() {
        sample
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_rnnoise_contract_without_echo_metrics() {
        let processor = RnNoise::try_new().unwrap();
        let spec = processor.io_spec();

        assert_eq!(processor.name(), "rnnoise");
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.near_channels, 1);
        assert_eq!(spec.algorithmic_latency_ms, 10.0);
        assert_eq!(processor.stats().erle_db, 0.0);
    }

    #[test]
    fn converts_normalized_pcm_and_preserves_finite_frame_length() {
        let mut processor = RnNoise::try_new().unwrap();
        let mut near = (0..FRAME_SAMPLES * 2)
            .map(|index| ((index as f32) * 0.013).sin() * 0.25)
            .collect::<Vec<_>>();
        near[13] = f32::NAN;
        near[700] = f32::NEG_INFINITY;
        let mut out = vec![2.0; near.len()];

        processor.process(&near, &[], &mut out, near.len() as u32);

        assert_eq!(out.len(), near.len());
        assert!(out.iter().all(|sample| sample.is_finite()));
        assert!(out.iter().all(|sample| (-1.0..=1.0).contains(sample)));
        assert!(out.iter().any(|sample| sample.abs() > f32::EPSILON));
        assert_eq!(processor.stats().runtime_error_count, 0);
    }

    #[test]
    fn reset_keeps_processing_ready() {
        let mut processor = RnNoise::try_new().unwrap();
        processor.reset();
        let near = vec![0.05; FRAME_SAMPLES];
        let mut out = vec![0.0; FRAME_SAMPLES];

        processor.process(&near, &[], &mut out, FRAME_SAMPLES as u32);

        assert!(out.iter().all(|sample| sample.is_finite()));
        assert_eq!(processor.stats().runtime_error_count, 0);
    }
}
