use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::dsp::{rms_dbfs_from_sum_squares as rms_dbfs, sum_squares};
use echoless_core::output_level_gain_db;
use echoless_processors::ProcessorStats;

use super::diagnostics::DiagnosticsStatusHandle;

pub(super) struct StatsSample<'a> {
    pub(super) algorithmic_latency_ms: f32,
    pub(super) near_delay_ms: u32,
    pub(super) near_delay_buffered_samples: usize,
    pub(super) frame_size: usize,
    pub(super) near: &'a [f32],
    pub(super) far: &'a [f32],
    pub(super) out: &'a [f32],
    pub(super) mic_q: usize,
    pub(super) ref_q: usize,
    pub(super) out_q: usize,
    pub(super) mic_input_drops: u64,
    pub(super) ref_input_drops: u64,
    pub(super) stale_drops: u64,
    pub(super) ref_underruns: u64,
    pub(super) output_overruns: u64,
    pub(super) output_underruns: u64,
    pub(super) node_stats: &'a [ProcessorStats],
}

pub(super) fn aggregate_process_time_ms(stats: &[ProcessorStats]) -> f32 {
    stats
        .iter()
        .map(|stat| stat.process_time_ms)
        .fold(0.0, f32::max)
}

pub(super) fn aggregate_runtime_errors(stats: &[ProcessorStats]) -> u64 {
    stats.iter().map(|stat| stat.runtime_error_count).sum()
}

pub(super) fn aggregate_estimated_delay_ms(stats: &[ProcessorStats]) -> i32 {
    stats
        .iter()
        .map(|stat| stat.estimated_delay_ms)
        .max()
        .unwrap_or(0)
}

pub(super) fn aggregate_aec3_delay_blocks(stats: &[ProcessorStats]) -> Option<u32> {
    stats.iter().find_map(|stat| stat.aec3_delay_blocks)
}

pub(super) fn aggregate_diverged(stats: &[ProcessorStats]) -> bool {
    stats.iter().any(|stat| stat.diverged)
}

pub(super) fn aggregate_last_error(stats: &[ProcessorStats]) -> Option<String> {
    stats
        .iter()
        .find_map(|stat| stat.last_backend_error.as_ref())
        .cloned()
}

pub(super) fn output_queue_latency_ms(out_q_samples: usize, sample_rate: u32) -> f64 {
    queue_latency_ms(out_q_samples, sample_rate)
}

pub(super) fn input_queue_latency_ms(mic_q_samples: usize, sample_rate: u32) -> f64 {
    queue_latency_ms(mic_q_samples, sample_rate)
}

fn queue_latency_ms(samples: usize, sample_rate: u32) -> f64 {
    if sample_rate == 0 {
        return 0.0;
    }
    samples as f64 / sample_rate as f64 * 1000.0
}

pub(super) fn estimate_user_latency_ms(
    frame_ms: u32,
    near_delay_ms: u32,
    algorithmic_latency_ms: f32,
    mic_q_samples: usize,
    out_q_samples: usize,
    sample_rate: u32,
) -> f64 {
    frame_ms as f64 / 2.0
        + near_delay_ms as f64
        + algorithmic_latency_ms as f64
        + input_queue_latency_ms(mic_q_samples, sample_rate)
        + output_queue_latency_ms(out_q_samples, sample_rate)
}

const STATUS_WAVE_BUCKETS: usize = 64;

struct WaveBuckets {
    peaks: Vec<f32>,
    bucket_index: usize,
    frames_in_bucket: usize,
    frames_per_bucket: usize,
}

impl WaveBuckets {
    fn new(buckets: usize, sample_rate: u32, interval: Duration) -> Self {
        let expected_frames = (sample_rate as f64 * interval.as_secs_f64()).ceil() as usize;
        let frames_per_bucket = (expected_frames
            .max(1)
            .saturating_add(buckets.saturating_sub(1))
            / buckets.max(1))
        .max(1);
        Self {
            peaks: vec![0.0; buckets],
            bucket_index: 0,
            frames_in_bucket: 0,
            frames_per_bucket,
        }
    }

    fn observe_samples(&mut self, samples: &[f32], frame_size: usize) {
        if self.peaks.is_empty() || samples.is_empty() {
            return;
        }
        let channels = samples.len().checked_div(frame_size).unwrap_or(1).max(1);
        for frame in samples.chunks(channels) {
            let peak = frame
                .iter()
                .map(|sample| sample.abs().min(1.0))
                .fold(0.0, f32::max);
            self.peaks[self.bucket_index] = self.peaks[self.bucket_index].max(peak);
            self.frames_in_bucket += 1;
            if self.frames_in_bucket >= self.frames_per_bucket {
                self.frames_in_bucket = 0;
                self.bucket_index = (self.bucket_index + 1).min(self.peaks.len() - 1);
            }
        }
    }

    fn values(&self) -> Vec<f32> {
        self.peaks.clone()
    }

    fn reset(&mut self) {
        self.peaks.fill(0.0);
        self.bucket_index = 0;
        self.frames_in_bucket = 0;
    }
}

pub(super) struct RealtimeStats {
    interval: Duration,
    started: Instant,
    last_print: Instant,
    sample_rate: u32,
    frame_ms: u32,
    backend: String,
    near_delay_ms: u32,
    output_level: u32,
    output_gain_db: Option<f32>,
    near_delay_buffered_samples: usize,
    algorithmic_latency_ms: f32,
    status_json: bool,
    diagnostics_session_dir: Option<String>,
    diagnostics_status: Option<DiagnosticsStatusHandle>,
    total_frames: u64,
    near_samples: u64,
    far_samples: u64,
    out_samples: u64,
    near_sq: f64,
    far_sq: f64,
    out_sq: f64,
    near_wave: WaveBuckets,
    far_wave: WaveBuckets,
    out_wave: WaveBuckets,
    mic_q: usize,
    ref_q: usize,
    out_q: usize,
    mic_input_drops: u64,
    ref_input_drops: u64,
    stale_drops: u64,
    ref_underruns: u64,
    output_overruns: u64,
    output_underruns: u64,
    node_process_time_ms: f32,
    node_runtime_errors: u64,
    aec_estimated_delay_ms: i32,
    aec3_delay_blocks: Option<u32>,
    node_diverged: bool,
    node_last_error: Option<String>,
}

pub(super) struct RealtimeStatsConfig {
    pub(super) interval: Duration,
    pub(super) sample_rate: u32,
    pub(super) frame_ms: u32,
    pub(super) near_delay_ms: u32,
    pub(super) output_level: u32,
    pub(super) backend: String,
    pub(super) algorithmic_latency_ms: f32,
    pub(super) status_json: bool,
    pub(super) diagnostics_session_dir: Option<String>,
    pub(super) diagnostics_status: Option<DiagnosticsStatusHandle>,
}

impl RealtimeStats {
    pub(super) fn new(config: RealtimeStatsConfig) -> Self {
        let now = Instant::now();
        Self {
            interval: config.interval,
            started: now,
            last_print: now,
            sample_rate: config.sample_rate,
            frame_ms: config.frame_ms,
            backend: config.backend,
            near_delay_ms: config.near_delay_ms,
            output_level: config.output_level,
            output_gain_db: output_level_gain_db(config.output_level),
            near_delay_buffered_samples: 0,
            algorithmic_latency_ms: config.algorithmic_latency_ms,
            status_json: config.status_json,
            diagnostics_session_dir: config.diagnostics_session_dir,
            diagnostics_status: config.diagnostics_status,
            total_frames: 0,
            near_samples: 0,
            far_samples: 0,
            out_samples: 0,
            near_sq: 0.0,
            far_sq: 0.0,
            out_sq: 0.0,
            near_wave: WaveBuckets::new(STATUS_WAVE_BUCKETS, config.sample_rate, config.interval),
            far_wave: WaveBuckets::new(STATUS_WAVE_BUCKETS, config.sample_rate, config.interval),
            out_wave: WaveBuckets::new(STATUS_WAVE_BUCKETS, config.sample_rate, config.interval),
            mic_q: 0,
            ref_q: 0,
            out_q: 0,
            mic_input_drops: 0,
            ref_input_drops: 0,
            stale_drops: 0,
            ref_underruns: 0,
            output_overruns: 0,
            output_underruns: 0,
            node_process_time_ms: 0.0,
            node_runtime_errors: 0,
            aec_estimated_delay_ms: 0,
            aec3_delay_blocks: None,
            node_diverged: false,
            node_last_error: None,
        }
    }

    pub(super) fn set_diagnostics(
        &mut self,
        session_dir: Option<String>,
        status: Option<DiagnosticsStatusHandle>,
    ) {
        self.diagnostics_session_dir = session_dir;
        self.diagnostics_status = status;
    }

    pub(super) fn set_output_level(&mut self, output_level: u32) {
        self.output_level = output_level;
        self.output_gain_db = output_level_gain_db(output_level);
    }

    pub(super) fn set_near_delay_ms(&mut self, near_delay_ms: u32) {
        self.near_delay_ms = near_delay_ms;
    }

    pub(super) fn observe(&mut self, sample: &StatsSample<'_>) {
        self.total_frames += sample.frame_size as u64;
        self.near_samples += sample.near.len() as u64;
        self.far_samples += sample.far.len() as u64;
        self.out_samples += sample.out.len() as u64;
        self.near_sq += sum_squares(sample.near);
        self.far_sq += sum_squares(sample.far);
        self.out_sq += sum_squares(sample.out);
        self.near_wave
            .observe_samples(sample.near, sample.frame_size);
        self.far_wave.observe_samples(sample.far, sample.frame_size);
        self.out_wave.observe_samples(sample.out, sample.frame_size);
        self.mic_q = sample.mic_q;
        self.ref_q = sample.ref_q;
        self.out_q = sample.out_q;
        self.near_delay_buffered_samples = sample.near_delay_buffered_samples;
        self.mic_input_drops += sample.mic_input_drops;
        self.ref_input_drops += sample.ref_input_drops;
        self.stale_drops += sample.stale_drops;
        self.ref_underruns += sample.ref_underruns;
        self.output_overruns += sample.output_overruns;
        self.output_underruns += sample.output_underruns;
        self.node_process_time_ms = self
            .node_process_time_ms
            .max(aggregate_process_time_ms(sample.node_stats));
        self.node_runtime_errors = aggregate_runtime_errors(sample.node_stats);
        self.aec_estimated_delay_ms = aggregate_estimated_delay_ms(sample.node_stats);
        self.aec3_delay_blocks = aggregate_aec3_delay_blocks(sample.node_stats);
        self.node_diverged = aggregate_diverged(sample.node_stats);
        self.node_last_error = aggregate_last_error(sample.node_stats);
        self.maybe_print();
    }

    fn maybe_print(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_print) < self.interval {
            return;
        }
        if self.status_json {
            println!("{}", self.status_json_line(now));
        } else {
            self.print_text(now);
        }
        self.last_print = now;
        self.near_samples = 0;
        self.far_samples = 0;
        self.out_samples = 0;
        self.near_sq = 0.0;
        self.far_sq = 0.0;
        self.out_sq = 0.0;
        self.near_wave.reset();
        self.far_wave.reset();
        self.out_wave.reset();
        self.mic_input_drops = 0;
        self.ref_input_drops = 0;
        self.stale_drops = 0;
        self.ref_underruns = 0;
        self.output_overruns = 0;
        self.output_underruns = 0;
        self.node_process_time_ms = 0.0;
        self.node_diverged = false;
        self.node_last_error = None;
    }

    fn print_text(&self, now: Instant) {
        println!(
            "t={:.1}s frames={} mic={:.1}dB ref={:.1}dB out={:.1}dB mic_q={} ref_q={} out_q={} near_delay_ms={} in_q_ms={:.1} out_q_ms={:.1} est_user_ms={:.1} aec_delay_ms={} ref_underrun={} out_underrun={} out_overrun={} input_drop={} stale_drop={} node_ms={:.2} runtime_errors={} diverged={}",
            now.duration_since(self.started).as_secs_f64(),
            self.total_frames,
            rms_dbfs(self.near_sq, self.near_samples),
            rms_dbfs(self.far_sq, self.far_samples),
            rms_dbfs(self.out_sq, self.out_samples),
            self.mic_q,
            self.ref_q,
            self.out_q,
            self.near_delay_ms,
            input_queue_latency_ms(self.mic_q, self.sample_rate),
            output_queue_latency_ms(self.out_q, self.sample_rate),
            estimate_user_latency_ms(
                self.frame_ms,
                self.near_delay_ms,
                self.algorithmic_latency_ms,
                self.mic_q,
                self.out_q,
                self.sample_rate
            ),
            self.aec_estimated_delay_ms,
            self.ref_underruns,
            self.output_underruns,
            self.output_overruns,
            self.mic_input_drops + self.ref_input_drops,
            self.stale_drops,
            self.node_process_time_ms,
            self.node_runtime_errors,
            self.node_diverged,
        );
    }

    fn status_json_line(&self, now: Instant) -> String {
        serde_json::to_string(&self.status_value(now)).unwrap_or_else(|err| {
            json!({ "type": "error", "message": err.to_string() }).to_string()
        })
    }

    fn status_value(&self, now: Instant) -> Value {
        let output_queue_latency_ms = output_queue_latency_ms(self.out_q, self.sample_rate);
        let input_queue_latency_ms = input_queue_latency_ms(self.mic_q, self.sample_rate);
        let estimated_user_latency_ms = estimate_user_latency_ms(
            self.frame_ms,
            self.near_delay_ms,
            self.algorithmic_latency_ms,
            self.mic_q,
            self.out_q,
            self.sample_rate,
        );
        let (recording, diagnostics_frames, diagnostics_elapsed_s, diagnostics_drops) = self
            .diagnostics_status
            .as_ref()
            .map(|status| {
                (
                    status.is_recording(),
                    status.frames(),
                    status.elapsed_s(),
                    status.drops(),
                )
            })
            .unwrap_or((false, 0, 0.0, 0));
        json!({
            "type": "status",
            "elapsed_s": now.duration_since(self.started).as_secs_f64(),
            "frames": self.total_frames,
            "sample_rate": self.sample_rate,
            "frame_ms": self.frame_ms,
            "backend": self.backend.as_str(),
            "near_delay_ms": self.near_delay_ms,
            "near_delay_buffered_samples": self.near_delay_buffered_samples,
            "output_level": self.output_level,
            "output_gain_db": self.output_gain_db,
            "mic_dbfs": rms_dbfs(self.near_sq, self.near_samples),
            "ref_dbfs": rms_dbfs(self.far_sq, self.far_samples),
            "out_dbfs": rms_dbfs(self.out_sq, self.out_samples),
            "mic_wave": self.near_wave.values(),
            "ref_wave": self.far_wave.values(),
            "out_wave": self.out_wave.values(),
            "mic_q_samples": self.mic_q,
            "ref_q_samples": self.ref_q,
            "out_q_samples": self.out_q,
            "input_queue_latency_ms": input_queue_latency_ms,
            "output_queue_latency_ms": output_queue_latency_ms,
            "algorithmic_latency_ms": self.algorithmic_latency_ms,
            "estimated_user_latency_ms": estimated_user_latency_ms,
            "aec_estimated_delay_ms": self.aec_estimated_delay_ms,
            "aec3_delay_blocks": self.aec3_delay_blocks,
            "mic_input_drops": self.mic_input_drops,
            "ref_input_drops": self.ref_input_drops,
            "input_drops": self.mic_input_drops + self.ref_input_drops,
            "stale_drops": self.stale_drops,
            "ref_underruns": self.ref_underruns,
            "output_underruns": self.output_underruns,
            "output_overruns": self.output_overruns,
            "node_process_time_ms": self.node_process_time_ms,
            "runtime_errors": self.node_runtime_errors,
            "diverged": self.node_diverged,
            "last_backend_error": self.node_last_error.as_deref(),
            "diagnostics_session_dir": self.diagnostics_session_dir.as_deref(),
            "recording": recording,
            "diagnostics_frames": diagnostics_frames,
            "diagnostics_elapsed_s": diagnostics_elapsed_s,
            "diagnostics_drops": diagnostics_drops,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_dbfs_reports_silence_and_full_scale() {
        assert_eq!(rms_dbfs(0.0, 480), -120.0);
        assert_eq!(rms_dbfs(480.0, 480), 0.0);
    }

    #[test]
    fn user_latency_estimate_includes_half_frame_near_delay_algorithm_and_queues() {
        let latency = estimate_user_latency_ms(10, 25, 16.0, 480, 2400, 48_000);

        assert_eq!(latency, 106.0);
    }

    #[test]
    fn wave_buckets_accumulate_peaks_without_sample_cache() {
        let mut buckets = WaveBuckets::new(2, 4, Duration::from_secs(1));

        buckets.observe_samples(&[0.0, -0.5, 0.25, 1.5], 4);

        assert_eq!(buckets.values(), vec![0.5, 1.0]);
        buckets.reset();
        assert_eq!(buckets.values(), vec![0.0, 0.0]);
    }

    #[test]
    fn runtime_status_json_exposes_frontend_latency_fields() {
        let mut stats = RealtimeStats::new(RealtimeStatsConfig {
            interval: Duration::from_millis(1000),
            sample_rate: 48_000,
            frame_ms: 10,
            near_delay_ms: 25,
            output_level: 75,
            backend: "localvqe".into(),
            algorithmic_latency_ms: 16.0,
            status_json: true,
            diagnostics_session_dir: Some("diagnostics/session-1".into()),
            diagnostics_status: None,
        });
        stats.total_frames = 480;
        stats.near_samples = 480;
        stats.far_samples = 480;
        stats.out_samples = 480;
        stats.near_sq = 120.0;
        stats.far_sq = 30.0;
        stats.out_sq = 100.0;
        stats.mic_q = 480;
        stats.out_q = 2400;
        stats.mic_input_drops = 1;
        stats.ref_input_drops = 2;
        stats.aec_estimated_delay_ms = 48;
        stats.aec3_delay_blocks = Some(12);

        let value = stats.status_value(stats.started + Duration::from_secs(1));

        assert_eq!(value["type"], "status");
        assert_eq!(value["backend"], "localvqe");
        assert_eq!(value["input_drops"], 3);
        assert_eq!(value["near_delay_ms"], 25);
        assert_eq!(value["output_level"], 75);
        assert_eq!(value["output_gain_db"], output_level_gain_db(75).unwrap());
        assert_eq!(value["input_queue_latency_ms"], 10.0);
        assert_eq!(value["output_queue_latency_ms"], 50.0);
        assert_eq!(value["estimated_user_latency_ms"], 106.0);
        assert_eq!(value["aec_estimated_delay_ms"], 48);
        assert_eq!(value["aec3_delay_blocks"], 12);
        assert_eq!(value["diagnostics_session_dir"], "diagnostics/session-1");
        assert_eq!(value["recording"], false);
        assert_eq!(value["diagnostics_frames"], 0);
        assert_eq!(value["diagnostics_drops"], 0);
        assert_eq!(
            value["mic_wave"].as_array().unwrap().len(),
            STATUS_WAVE_BUCKETS
        );
        assert_eq!(
            value["ref_wave"].as_array().unwrap().len(),
            STATUS_WAVE_BUCKETS
        );
        assert_eq!(
            value["out_wave"].as_array().unwrap().len(),
            STATUS_WAVE_BUCKETS
        );

        stats.set_output_level(0);
        let value = stats.status_value(stats.started + Duration::from_secs(2));
        assert_eq!(value["output_level"], 0);
        assert!(value["output_gain_db"].is_null());

        stats.set_near_delay_ms(40);
        let value = stats.status_value(stats.started + Duration::from_secs(3));
        assert_eq!(value["near_delay_ms"], 40);
    }
}
