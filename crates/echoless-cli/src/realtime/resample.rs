use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

use ringbuf::traits::Consumer;
use rubato::{FftFixedIn, FftFixedOut, Resampler};

pub(super) struct InterleavedInputResampler {
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    configured_input_frames: usize,
    planes: Vec<Vec<f32>>,
    resampled_planes: Vec<Vec<f32>>,
    output: Vec<f32>,
    resampler: Option<FftFixedIn<f32>>,
    linear_fallback: InterleavedLinearFallback,
}

impl InterleavedInputResampler {
    pub(super) fn new(in_rate: u32, out_rate: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        Self {
            in_rate,
            out_rate,
            channels,
            configured_input_frames: 0,
            planes: Vec::new(),
            resampled_planes: Vec::new(),
            output: Vec::new(),
            resampler: None,
            linear_fallback: InterleavedLinearFallback::new(in_rate, out_rate, channels),
        }
    }

    pub(super) fn process(&mut self, input: &[f32]) -> &[f32] {
        self.output.clear();
        if input.is_empty() {
            return &self.output;
        }
        if self.in_rate == self.out_rate {
            self.output.extend_from_slice(input);
            return &self.output;
        }

        let frames = input.len() / self.channels;
        if frames == 0 {
            return &self.output;
        }

        self.ensure_layout(frames);
        self.fill_planes(input, frames);

        if let Some((_, out_frames)) = self.resampler.as_mut().and_then(|resampler| {
            resampler
                .process_into_buffer(&self.planes, &mut self.resampled_planes, None)
                .ok()
        }) {
            self.interleave_from_resampled(out_frames);
        } else {
            self.linear_fallback.process_into(input, &mut self.output);
        }

        &self.output
    }

    fn ensure_layout(&mut self, input_frames: usize) {
        if self.configured_input_frames == input_frames {
            return;
        }
        self.configured_input_frames = input_frames;
        resize_planes(&mut self.planes, self.channels, input_frames);

        self.resampler = FftFixedIn::<f32>::new(
            self.in_rate as usize,
            self.out_rate as usize,
            input_frames,
            1,
            self.channels,
        )
        .ok();
        let output_frames_max = self
            .resampler
            .as_ref()
            .map(Resampler::output_frames_max)
            .unwrap_or_else(|| scaled_frames(input_frames, self.in_rate, self.out_rate));
        resize_planes(&mut self.resampled_planes, self.channels, output_frames_max);
        self.output.resize(output_frames_max * self.channels, 0.0);
        self.output.clear();
    }

    fn fill_planes(&mut self, input: &[f32], frames: usize) {
        for frame in 0..frames {
            let frame_start = frame * self.channels;
            for channel in 0..self.channels {
                self.planes[channel][frame] = input[frame_start + channel];
            }
        }
    }

    fn interleave_from_resampled(&mut self, frames: usize) {
        self.output.resize(frames * self.channels, 0.0);
        for frame in 0..frames {
            for channel in 0..self.channels {
                self.output[frame * self.channels + channel] =
                    self.resampled_planes[channel][frame];
            }
        }
    }
}

struct InterleavedLinearFallback {
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    input_frames_seen: u64,
    next_output_source_pos: f64,
    prev_frame: Option<Vec<f32>>,
}

impl InterleavedLinearFallback {
    fn new(in_rate: u32, out_rate: u32, channels: usize) -> Self {
        Self {
            in_rate,
            out_rate,
            channels: channels.max(1),
            input_frames_seen: 0,
            next_output_source_pos: 0.0,
            prev_frame: None,
        }
    }

    fn process_into(&mut self, input: &[f32], out: &mut Vec<f32>) {
        out.clear();
        if self.in_rate == self.out_rate || input.is_empty() {
            out.extend_from_slice(input);
            return;
        }
        let frames = input.len() / self.channels;
        if frames == 0 {
            return;
        }

        let start_abs = self.input_frames_seen;
        let end_abs = start_abs + frames as u64;
        let step = self.in_rate as f64 / self.out_rate as f64;
        out.reserve(((frames as f64) / step).ceil() as usize * self.channels);

        while self.next_output_source_pos.floor() as u64 + 1 < end_abs {
            let pos = self.next_output_source_pos;
            let i0 = pos.floor() as u64;
            let i1 = i0 + 1;
            let frac = (pos - i0 as f64) as f32;
            for ch in 0..self.channels {
                let a = self.sample_at(input, start_abs, i0, ch).unwrap_or(0.0);
                let b = self.sample_at(input, start_abs, i1, ch).unwrap_or(a);
                out.push(a + (b - a) * frac);
            }
            self.next_output_source_pos += step;
        }

        self.input_frames_seen = end_abs;
        let last_start = (frames - 1) * self.channels;
        self.prev_frame = Some(input[last_start..last_start + self.channels].to_vec());
    }

    fn sample_at(
        &self,
        input: &[f32],
        start_abs: u64,
        index_abs: u64,
        channel: usize,
    ) -> Option<f32> {
        if index_abs + 1 == start_abs {
            return self
                .prev_frame
                .as_ref()
                .and_then(|frame| frame.get(channel).copied());
        }
        if index_abs < start_abs {
            return None;
        }
        let local = (index_abs - start_abs) as usize;
        input.get(local * self.channels + channel).copied()
    }
}

pub(super) struct OutputDeviceResampler {
    in_rate: u32,
    out_rate: u32,
    configured_output_frames: usize,
    input_planes: Vec<Vec<f32>>,
    output_planes: Vec<Vec<f32>>,
    output: Vec<f32>,
    resampler: Option<FftFixedOut<f32>>,
    linear_fallback: OutputLinearFallback,
}

impl OutputDeviceResampler {
    pub(super) fn new(in_rate: u32, out_rate: u32) -> Self {
        Self {
            in_rate,
            out_rate,
            configured_output_frames: 0,
            input_planes: Vec::new(),
            output_planes: Vec::new(),
            output: Vec::new(),
            resampler: None,
            linear_fallback: OutputLinearFallback::new(in_rate, out_rate),
        }
    }

    pub(super) fn next_chunk<C>(
        &mut self,
        output_frames: usize,
        consumer: &mut C,
        underruns: &AtomicU64,
    ) -> &[f32]
    where
        C: Consumer<Item = f32>,
    {
        self.output.clear();
        if output_frames == 0 {
            return &self.output;
        }
        if self.in_rate == self.out_rate {
            self.output.resize(output_frames, 0.0);
            fill_from_consumer(&mut self.output, consumer, underruns);
            return &self.output;
        }

        self.ensure_layout(output_frames);
        if let Some(resampler) = self.resampler.as_mut() {
            let input_frames = resampler.input_frames_next();
            resize_planes(&mut self.input_planes, 1, input_frames);
            fill_from_consumer(&mut self.input_planes[0], consumer, underruns);
            if let Ok((_, out_frames)) =
                resampler.process_into_buffer(&self.input_planes, &mut self.output_planes, None)
            {
                self.output
                    .extend_from_slice(&self.output_planes[0][..out_frames.min(output_frames)]);
                self.output.resize(output_frames, 0.0);
                return &self.output;
            }
            self.output.resize(output_frames, 0.0);
            return &self.output;
        }

        self.linear_fallback
            .fill_chunk(output_frames, consumer, underruns, &mut self.output);
        &self.output
    }

    fn ensure_layout(&mut self, output_frames: usize) {
        if self.configured_output_frames == output_frames {
            return;
        }
        self.configured_output_frames = output_frames;
        self.resampler = FftFixedOut::<f32>::new(
            self.in_rate as usize,
            self.out_rate as usize,
            output_frames,
            1,
            1,
        )
        .ok();
        let input_frames = self
            .resampler
            .as_ref()
            .map(Resampler::input_frames_next)
            .unwrap_or_else(|| scaled_frames(output_frames, self.out_rate, self.in_rate));
        let output_frames_max = self
            .resampler
            .as_ref()
            .map(Resampler::output_frames_max)
            .unwrap_or(output_frames);
        resize_planes(&mut self.input_planes, 1, input_frames);
        resize_planes(&mut self.output_planes, 1, output_frames_max);
        self.output.resize(output_frames, 0.0);
        self.output.clear();
    }
}

struct OutputLinearFallback {
    step: f64,
    pos: f64,
    buffer: VecDeque<f32>,
}

impl OutputLinearFallback {
    fn new(in_rate: u32, out_rate: u32) -> Self {
        let step = if out_rate == 0 {
            1.0
        } else {
            in_rate as f64 / out_rate as f64
        };
        Self {
            step,
            pos: 0.0,
            buffer: VecDeque::new(),
        }
    }

    fn fill_chunk<C>(
        &mut self,
        output_frames: usize,
        consumer: &mut C,
        underruns: &AtomicU64,
        output: &mut Vec<f32>,
    ) where
        C: Consumer<Item = f32>,
    {
        output.resize(output_frames, 0.0);
        for sample in output.iter_mut() {
            *sample = self.next_sample(consumer, underruns);
        }
    }

    fn next_sample<C>(&mut self, consumer: &mut C, underruns: &AtomicU64) -> f32
    where
        C: Consumer<Item = f32>,
    {
        let needed = (self.pos.floor() as usize).saturating_add(2);
        while self.buffer.len() < needed {
            match consumer.try_pop() {
                Some(sample) => self.buffer.push_back(sample.clamp(-1.0, 1.0)),
                None => {
                    underruns.fetch_add(1, Ordering::Relaxed);
                    return 0.0;
                }
            }
        }

        let i0 = self.pos.floor() as usize;
        let frac = (self.pos - i0 as f64) as f32;
        let a = self.buffer.get(i0).copied().unwrap_or(0.0);
        let b = self.buffer.get(i0 + 1).copied().unwrap_or(a);
        let sample = (a + (b - a) * frac).clamp(-1.0, 1.0);

        self.pos += self.step;
        let consumed = self.pos.floor() as usize;
        for _ in 0..consumed {
            let _ = self.buffer.pop_front();
        }
        self.pos -= consumed as f64;
        sample
    }
}

fn fill_from_consumer<C>(output: &mut [f32], consumer: &mut C, underruns: &AtomicU64)
where
    C: Consumer<Item = f32>,
{
    for sample in output.iter_mut() {
        *sample = match consumer.try_pop() {
            Some(v) => v.clamp(-1.0, 1.0),
            None => {
                underruns.fetch_add(1, Ordering::Relaxed);
                0.0
            }
        };
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
    use std::sync::atomic::AtomicU64;

    use ringbuf::traits::{Producer, Split};
    use ringbuf::HeapRb;

    use super::*;

    #[test]
    fn input_resampler_upsamples_and_reuses_buffers() {
        let mut resampler = InterleavedInputResampler::new(24_000, 48_000, 1);
        let input = sine_block(240, 440.0, 24_000);

        let first = resampler.process(&input).to_vec();
        let capacity = input_capacity_signature(&resampler);
        let second = resampler.process(&input).to_vec();

        assert!(first.len() >= 480);
        assert!(first.iter().all(|sample| sample.is_finite()));
        assert!(second.iter().all(|sample| sample.is_finite()));
        assert_eq!(input_capacity_signature(&resampler), capacity);
    }

    #[test]
    fn input_resampler_downsamples_stereo_without_collapsing_channels() {
        let mut resampler = InterleavedInputResampler::new(48_000, 24_000, 2);
        let mut input = vec![0.0; 480 * 2];
        for frame in 0..480 {
            input[frame * 2] = 0.5;
            input[frame * 2 + 1] = -0.5;
        }

        let output = resampler.process(&input);

        assert!(output.len() >= 240 * 2);
        let mut left_sum = 0.0f32;
        let mut right_sum = 0.0f32;
        let mut frames = 0usize;
        for frame in output.chunks_exact(2).skip(16).take(200) {
            left_sum += frame[0];
            right_sum += frame[1];
            frames += 1;
        }
        let left_avg = left_sum / frames as f32;
        let right_avg = right_sum / frames as f32;
        assert!(left_avg > 0.2, "left channel collapsed: {left_avg}");
        assert!(right_avg < -0.2, "right channel collapsed: {right_avg}");
    }

    #[test]
    fn output_resampler_returns_requested_device_chunk() {
        let drops = AtomicU64::new(0);
        let (mut prod, mut cons) = HeapRb::<f32>::new(4096).split();
        let input = sine_block(960, 440.0, 48_000);
        assert_eq!(prod.push_slice(&input), input.len());
        let mut resampler = OutputDeviceResampler::new(48_000, 24_000);

        let output = resampler.next_chunk(240, &mut cons, &drops);

        assert_eq!(output.len(), 240);
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert_eq!(drops.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn output_resampler_passthrough_pulls_pipeline_samples() {
        let drops = AtomicU64::new(0);
        let (mut prod, mut cons) = HeapRb::<f32>::new(8).split();
        assert_eq!(prod.push_slice(&[0.0, 0.25, 0.5, 0.75]), 4);
        let mut resampler = OutputDeviceResampler::new(48_000, 48_000);

        let output = resampler.next_chunk(4, &mut cons, &drops);

        assert_eq!(output, &[0.0, 0.25, 0.5, 0.75]);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
    }

    fn sine_block(frames: usize, hz: f32, rate: u32) -> Vec<f32> {
        use std::f32::consts::TAU;

        (0..frames)
            .map(|i| 0.2 * (i as f32 * hz * TAU / rate as f32).sin())
            .collect()
    }

    fn input_capacity_signature(resampler: &InterleavedInputResampler) -> Vec<usize> {
        let mut caps = vec![resampler.output.capacity(), resampler.planes.capacity()];
        caps.extend(resampler.planes.iter().map(Vec::capacity));
        caps.push(resampler.resampled_planes.capacity());
        caps.extend(resampler.resampled_planes.iter().map(Vec::capacity));
        caps
    }
}
