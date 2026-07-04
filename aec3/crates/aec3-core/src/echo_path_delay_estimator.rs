//! Echo path delay estimator.
//!
//! Estimates the delay of the echo path using matched filtering and lag
//! aggregation.
//!
//! Ported from `modules/audio_processing/aec3/echo_path_delay_estimator.h/cc`.

use crate::alignment_mixer::AlignmentMixer;
use crate::block::Block;
use crate::clockdrift_detector::{ClockdriftDetector, ClockdriftLevel};
use crate::common::{
    BLOCK_SIZE, MATCHED_FILTER_ALIGNMENT_SHIFT_SIZE_SUB_BLOCKS,
    MATCHED_FILTER_WINDOW_SIZE_SUB_BLOCKS, NUM_BLOCKS_PER_SECOND,
};
use crate::config::EchoCanceller3Config;
use crate::decimator::Decimator;
use crate::delay_estimate::{DelayEstimate, DelayEstimateQuality};
use crate::downsampled_render_buffer::DownsampledRenderBuffer;
use crate::matched_filter::MatchedFilter;
use crate::matched_filter_lag_aggregator::MatchedFilterLagAggregator;
use aec3_simd::SimdBackend;

/// Estimates the delay of the echo path.
#[derive(Debug)]
pub(crate) struct EchoPathDelayEstimator {
    down_sampling_factor: usize,
    sub_block_size: usize,
    capture_mixer: AlignmentMixer,
    capture_decimator: Decimator,
    matched_filter: MatchedFilter,
    matched_filter_lag_aggregator: MatchedFilterLagAggregator,
    old_aggregated_lag: Option<DelayEstimate>,
    consistent_estimate_counter: usize,
    delay_hold: bool,
    render_gate_power_threshold: f32,
    render_gate_hold_blocks: usize,
    low_render_blocks: usize,
    clockdrift_detector: ClockdriftDetector,
}

impl EchoPathDelayEstimator {
    pub(crate) fn new(
        backend: SimdBackend,
        config: &EchoCanceller3Config,
        num_capture_channels: usize,
    ) -> Self {
        let down_sampling_factor = config.delay.down_sampling_factor;
        let sub_block_size = BLOCK_SIZE
            .checked_div(down_sampling_factor)
            .unwrap_or(BLOCK_SIZE);

        let excitation_limit = if config.delay.down_sampling_factor == 8 {
            config.render_levels.poor_excitation_render_limit_ds8
        } else {
            config.render_levels.poor_excitation_render_limit
        };

        let matched_filter = MatchedFilter::new(
            backend,
            sub_block_size,
            MATCHED_FILTER_WINDOW_SIZE_SUB_BLOCKS,
            config.delay.num_filters,
            MATCHED_FILTER_ALIGNMENT_SHIFT_SIZE_SUB_BLOCKS,
            excitation_limit,
            config.delay.delay_estimate_smoothing,
            config.delay.delay_estimate_smoothing_delay_found,
            config.delay.delay_candidate_detection_threshold,
            config.delay.detect_pre_echo,
        );

        let matched_filter_lag_aggregator =
            MatchedFilterLagAggregator::new(matched_filter.get_max_filter_lag(), &config.delay);

        Self {
            down_sampling_factor,
            sub_block_size,
            capture_mixer: AlignmentMixer::new(
                num_capture_channels,
                &config.delay.capture_alignment_mixing,
            ),
            capture_decimator: Decimator::new(down_sampling_factor),
            matched_filter,
            matched_filter_lag_aggregator,
            old_aggregated_lag: None,
            consistent_estimate_counter: 0,
            delay_hold: config.delay.delay_hold,
            render_gate_power_threshold: config.delay.render_gate_power_threshold,
            render_gate_hold_blocks: config.delay.render_gate_hold_blocks,
            low_render_blocks: 0,
            clockdrift_detector: ClockdriftDetector::new(),
        }
    }

    /// Resets the estimation. If `reset_delay_confidence` is true, the reset
    /// behavior is as if the call is restarted.
    pub(crate) fn reset(&mut self, reset_delay_confidence: bool) {
        self.reset_internal(true, reset_delay_confidence);
    }

    /// Produces a delay estimate if one is available.
    pub(crate) fn estimate_delay(
        &mut self,
        render_buffer: &DownsampledRenderBuffer,
        capture: &Block,
    ) -> Option<DelayEstimate> {
        let mut downmixed_capture = [0.0f32; BLOCK_SIZE];
        self.capture_mixer
            .produce_output(capture, &mut downmixed_capture);

        let mut downsampled_capture_data = [0.0f32; BLOCK_SIZE];
        let downsampled_capture = &mut downsampled_capture_data[..self.sub_block_size];
        self.capture_decimator
            .decimate(&downmixed_capture, downsampled_capture);

        if self.delay_hold && self.render_gate_holding(render_buffer) {
            return self.old_aggregated_lag;
        }

        self.matched_filter.update(
            render_buffer,
            downsampled_capture,
            self.matched_filter_lag_aggregator.reliable_delay_found(),
        );

        let mut aggregated_matched_filter_lag = self
            .matched_filter_lag_aggregator
            .aggregate(self.matched_filter.get_best_lag_estimate());

        // Run clockdrift detection.
        if let Some(lag) = &aggregated_matched_filter_lag
            && lag.quality == DelayEstimateQuality::Refined
        {
            self.clockdrift_detector.update(
                self.matched_filter_lag_aggregator
                    .get_delay_at_highest_peak(),
            );
        }

        // Return the detected delay in samples as the aggregated matched filter
        // lag compensated by the down sampling factor.
        if let Some(lag) = &mut aggregated_matched_filter_lag {
            lag.delay *= self.down_sampling_factor;
        }

        if let (Some(old), Some(new)) = (&self.old_aggregated_lag, &aggregated_matched_filter_lag) {
            if old.delay == new.delay {
                self.consistent_estimate_counter += 1;
            } else {
                self.consistent_estimate_counter = 0;
            }
        } else {
            self.consistent_estimate_counter = 0;
        }
        self.old_aggregated_lag = aggregated_matched_filter_lag;

        const NUM_BLOCKS_PER_SECOND_BY_2: usize = NUM_BLOCKS_PER_SECOND / 2;
        if self.consistent_estimate_counter > NUM_BLOCKS_PER_SECOND_BY_2 {
            self.reset_internal(false, false);
        }

        aggregated_matched_filter_lag
    }

    /// Returns the level of detected clock drift.
    pub(crate) fn clockdrift(&self) -> ClockdriftLevel {
        self.clockdrift_detector.clockdrift_level()
    }

    fn reset_internal(&mut self, reset_lag_aggregator: bool, reset_delay_confidence: bool) {
        if reset_lag_aggregator {
            self.matched_filter_lag_aggregator
                .reset(reset_delay_confidence);
        }
        self.matched_filter.reset(reset_lag_aggregator);
        self.old_aggregated_lag = None;
        self.consistent_estimate_counter = 0;
        self.low_render_blocks = 0;
    }

    fn render_gate_holding(&mut self, render_buffer: &DownsampledRenderBuffer) -> bool {
        let render_energy = self.current_render_energy(render_buffer);
        let energy_threshold = self.render_gate_power_threshold
            * self.render_gate_power_threshold
            * self.sub_block_size as f32;

        if render_energy > energy_threshold {
            self.low_render_blocks = 0;
            return false;
        }

        self.low_render_blocks = self.low_render_blocks.saturating_add(1);
        self.low_render_blocks >= self.render_gate_hold_blocks
    }

    fn current_render_energy(&self, render_buffer: &DownsampledRenderBuffer) -> f32 {
        (0..self.sub_block_size)
            .map(|offset| {
                let index = render_buffer.offset_index(render_buffer.read, offset as i32);
                let sample = render_buffer.buffer[index];
                sample * sample
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn estimator_with_hold(hold_blocks: usize) -> EchoPathDelayEstimator {
        let mut config = EchoCanceller3Config::default();
        config.delay.delay_hold = true;
        config.delay.render_gate_power_threshold = 100.0;
        config.delay.render_gate_hold_blocks = hold_blocks;
        EchoPathDelayEstimator::new(aec3_simd::SimdBackend::Scalar, &config, 1)
    }

    fn render_buffer_with_level(sub_block_size: usize, level: f32) -> DownsampledRenderBuffer {
        let mut render_buffer = DownsampledRenderBuffer::new(sub_block_size * 2);
        for sample in &mut render_buffer.buffer[..sub_block_size] {
            *sample = level;
        }
        render_buffer
    }

    #[test]
    fn render_gate_holds_after_configured_low_render_blocks() {
        let mut estimator = estimator_with_hold(2);
        let low_render = render_buffer_with_level(estimator.sub_block_size, 0.0);
        let high_render = render_buffer_with_level(estimator.sub_block_size, 1000.0);

        assert!(!estimator.render_gate_holding(&low_render));
        assert!(estimator.render_gate_holding(&low_render));
        assert!(!estimator.render_gate_holding(&high_render));
        assert!(!estimator.render_gate_holding(&low_render));
    }

    #[test]
    fn render_gate_freezes_lag_without_consistency_counter_growth() {
        let mut estimator = estimator_with_hold(1);
        let frozen_lag = DelayEstimate::new(DelayEstimateQuality::Refined, 64);
        estimator.old_aggregated_lag = Some(frozen_lag);
        estimator.consistent_estimate_counter = 7;

        let low_render = render_buffer_with_level(estimator.sub_block_size, 0.0);
        let capture = Block::new(1, 1);

        assert_eq!(
            estimator
                .estimate_delay(&low_render, &capture)
                .map(|d| d.delay),
            Some(frozen_lag.delay)
        );
        assert_eq!(estimator.consistent_estimate_counter, 7);
        assert_eq!(estimator.old_aggregated_lag.map(|d| d.delay), Some(64));
    }
}
