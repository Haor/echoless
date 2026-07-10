use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::dsp::{rms_dbfs_from_sum_squares as rms_dbfs, sum_squares};
use echoless_core::output_level_gain_db;
use echoless_processors::ProcessorStats;

use super::diagnostics::DiagnosticsStatusHandle;
use super::emit::emit_stdout_line;

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
    pub(super) mic_stale_drops: u64,
    pub(super) ref_stale_drops: u64,
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
const CLOCK_SKEW_WINDOW_SECS: u64 = 5;
const CLOCK_SKEW_ENTER_RATIO: f64 = 0.02;
const CLOCK_SKEW_EXIT_RATIO: f64 = 0.01;
const CLOCK_SKEW_REF_TOLERANCE_RATIO: f64 = 0.5;
// 窗口级 EMA:新满窗占 50%。配合 ENTER_WINDOWS 滤掉单窗瞬时尖峰
// (切窗/调度抖动把一次 underrun 突发挤进一个窗口,raw 比率可冲到 8%+)。
const CLOCK_SKEW_EMA_ALPHA: f64 = 0.5;
// 连续满窗超阈值(且参考侧佐证)才进入告警:单窗尖峰不告警。
const CLOCK_SKEW_ENTER_WINDOWS: u32 = 2;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ClockSkewDirection {
    OutputFaster,
    CaptureFaster,
}

impl ClockSkewDirection {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::OutputFaster => "output_faster_than_capture",
            Self::CaptureFaster => "capture_faster_than_output",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ClockSkewSnapshot {
    pub(super) output_skew_pct: f64,
    pub(super) ref_skew_pct: f64,
    pub(super) ref_correlated: bool,
    pub(super) direction: Option<ClockSkewDirection>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClockSkewEventKind {
    Warning,
    Resolved,
}

#[derive(Clone, Copy, Debug)]
struct ClockSkewEvent {
    kind: ClockSkewEventKind,
    snapshot: ClockSkewSnapshot,
}

#[derive(Clone, Debug)]
struct ClockSkewDetector {
    min_window_frames: u64,
    window_frames: u64,
    window_output_underruns: u64,
    window_output_overruns: u64,
    window_ref_overruns: u64,
    window_ref_underruns: u64,
    snapshot: ClockSkewSnapshot,
    warning: bool,
    // 平滑 + 消抖(2026-07-09,黑屏 RCA 遗留项):raw 单窗比率对调度抖动极敏感
    // (mac cmd+tab 切窗瞬间可从 0% 冲到 8%+ 再回落),导致 warning/resolved
    // 高频翻转刷事件、面板读数乱跳。两道闸:
    //   1. EMA 平滑比率(snapshot 对外报的也是平滑值,面板不再跳变);
    //   2. 连续 CLOCK_SKEW_ENTER_WINDOWS 个满窗超阈值才进入告警。
    // 解除侧保持单窗即出(EXIT 阈值本身已是滞回下沿,快恢复体验更好)。
    ema_output_ratio: Option<f64>,
    ema_ref_ratio: Option<f64>,
    enter_streak: u32,
}

impl ClockSkewDetector {
    fn new(sample_rate: u32) -> Self {
        Self {
            min_window_frames: u64::from(sample_rate).saturating_mul(CLOCK_SKEW_WINDOW_SECS),
            window_frames: 0,
            window_output_underruns: 0,
            window_output_overruns: 0,
            window_ref_overruns: 0,
            window_ref_underruns: 0,
            snapshot: ClockSkewSnapshot::default(),
            warning: false,
            ema_output_ratio: None,
            ema_ref_ratio: None,
            enter_streak: 0,
        }
    }

    fn observe(
        &mut self,
        frames: u64,
        output_underruns: u64,
        output_overruns: u64,
        ref_overruns: u64,
        ref_underruns: u64,
    ) -> Option<ClockSkewEvent> {
        self.window_frames = self.window_frames.saturating_add(frames);
        self.window_output_underruns = self
            .window_output_underruns
            .saturating_add(output_underruns);
        self.window_output_overruns = self.window_output_overruns.saturating_add(output_overruns);
        self.window_ref_overruns = self.window_ref_overruns.saturating_add(ref_overruns);
        self.window_ref_underruns = self.window_ref_underruns.saturating_add(ref_underruns);

        if self.min_window_frames == 0 || self.window_frames < self.min_window_frames {
            // 满窗前 snapshot 用 raw 累计值(EMA 尚无本窗贡献);对外读数在
            // 首个满窗后即切换为平滑值。
            self.snapshot = ClockSkewSnapshot::from_frame_counts(
                self.window_frames,
                self.window_output_underruns,
                self.window_output_overruns,
                self.window_ref_overruns,
                self.window_ref_underruns,
            );
            return None;
        }

        let raw = ClockSkewSnapshot::from_frame_counts(
            self.window_frames,
            self.window_output_underruns,
            self.window_output_overruns,
            self.window_ref_overruns,
            self.window_ref_underruns,
        );
        let raw_output = raw.output_skew_pct / 100.0;
        let raw_ref = raw.ref_skew_pct / 100.0;
        // 窗口级 EMA:首窗直取,此后按 α 混入 —— 对外读数(snapshot)与解除判定
        // 走平滑值,面板不跳、告警态不抖;进入判定走 raw(见下),否则尖峰的
        // EMA 余温会让 streak 在错配已消失的窗口里继续累积、造成误告警。
        let ema = |prev: Option<f64>, raw: f64| match prev {
            Some(p) => p + CLOCK_SKEW_EMA_ALPHA * (raw - p),
            None => raw,
        };
        let output_ratio = ema(self.ema_output_ratio, raw_output);
        let ref_ratio = ema(self.ema_ref_ratio, raw_ref);
        self.ema_output_ratio = Some(output_ratio);
        self.ema_ref_ratio = Some(ref_ratio);
        self.snapshot = ClockSkewSnapshot::from_ratios(output_ratio, ref_ratio);

        // 进入侧按 raw 单窗判定:本窗真实超阈值且参考侧佐证,streak 才累积;
        // 任一平静窗立即断裂 → 孤立尖峰(切窗/调度抖动)永远凑不齐连续窗。
        let raw_win = ClockSkewSnapshot::from_ratios(raw_output, raw_ref);
        let over = raw_output.abs() > CLOCK_SKEW_ENTER_RATIO && raw_win.ref_correlated;
        self.enter_streak = if over {
            self.enter_streak.saturating_add(1)
        } else {
            0
        };

        let event = if !self.warning && self.enter_streak >= CLOCK_SKEW_ENTER_WINDOWS {
            self.warning = true;
            Some(ClockSkewEvent {
                kind: ClockSkewEventKind::Warning,
                snapshot: self.snapshot,
            })
        } else if self.warning
            && (output_ratio.abs() < CLOCK_SKEW_EXIT_RATIO || !self.snapshot.ref_correlated)
        {
            self.warning = false;
            Some(ClockSkewEvent {
                kind: ClockSkewEventKind::Resolved,
                snapshot: self.snapshot,
            })
        } else {
            None
        };

        self.window_frames = 0;
        self.window_output_underruns = 0;
        self.window_output_overruns = 0;
        self.window_ref_overruns = 0;
        self.window_ref_underruns = 0;
        event
    }
}

impl ClockSkewSnapshot {
    pub(super) fn from_frame_counts(
        frames: u64,
        output_underruns: u64,
        output_overruns: u64,
        ref_overruns: u64,
        ref_underruns: u64,
    ) -> Self {
        Self::from_ratios(
            signed_ratio(output_underruns, output_overruns, frames),
            signed_ratio(ref_overruns, ref_underruns, frames),
        )
    }

    fn from_ratios(output_ratio: f64, ref_ratio: f64) -> Self {
        let tolerance = output_ratio.abs() * CLOCK_SKEW_REF_TOLERANCE_RATIO;
        Self {
            output_skew_pct: output_ratio * 100.0,
            ref_skew_pct: ref_ratio * 100.0,
            ref_correlated: output_ratio != 0.0
                && output_ratio.signum() == ref_ratio.signum()
                && (output_ratio - ref_ratio).abs() <= tolerance,
            direction: if output_ratio > 0.0 {
                Some(ClockSkewDirection::OutputFaster)
            } else if output_ratio < 0.0 {
                Some(ClockSkewDirection::CaptureFaster)
            } else {
                None
            },
        }
    }
}

fn signed_ratio(positive: u64, negative: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        (positive as f64 - negative as f64) / denominator as f64
    }
}

/// 参考侧「跟不上生产/消费节奏」的合并信号,供时钟错配检测用作与 output_underruns 同源的佐证。
///
/// - pre-T3(output_rate_match=off):ref 侧 `skip_stale` 硬丢帧 → 信号在 `ref_stale_drops`,
///   参考 ring 被 skip_stale 约束住基本不溢出,`ref_input_drops≈0`。
/// - T3(output_rate_match=on):ref 侧改连续重采样,`ref_stale_drops` 恒 0;大错配(超 ±3%
///   权限)时参考 ring 溢出 → 信号迁移到 `ref_input_drops`。
///
/// 两者相加使 [`ClockSkewDetector`] 在 T3 开关两种模式下都能拿到参考侧佐证(各模式恰有一路≈0),
/// 从而保住 T1 的告警能力(否则 T3 会让 `ref_stale_drops` 恒 0、`ref_correlated` 永假、告警失效)。
pub(super) fn ref_pace_loss(ref_stale_drops: u64, ref_input_drops: u64) -> u64 {
    ref_stale_drops.saturating_add(ref_input_drops)
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
    bypassed: bool,
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
    mic_stale_drops: u64,
    ref_stale_drops: u64,
    ref_underruns: u64,
    output_overruns: u64,
    output_underruns: u64,
    clock_frames: u64,
    clock_skew: ClockSkewDetector,
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
    pub(super) bypassed: bool,
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
            bypassed: config.bypassed,
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
            mic_stale_drops: 0,
            ref_stale_drops: 0,
            ref_underruns: 0,
            output_overruns: 0,
            output_underruns: 0,
            clock_frames: 0,
            clock_skew: ClockSkewDetector::new(config.sample_rate),
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

    pub(super) fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
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
        self.mic_stale_drops += sample.mic_stale_drops;
        self.ref_stale_drops += sample.ref_stale_drops;
        self.ref_underruns += sample.ref_underruns;
        self.output_overruns += sample.output_overruns;
        self.output_underruns += sample.output_underruns;
        self.clock_frames = self.clock_frames.saturating_add(sample.frame_size as u64);
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
        // 参考侧「跟不上」信号:pre-T3 = skip_stale 硬丢帧(ref_stale_drops);
        // T3 开启后 ref 侧改连续重采样,ref_stale_drops 恒 0,大错配时体现为参考 ring 溢出
        // (ref_input_drops)。两者相加 → 无论 T3 开关,都能提供与 output_underruns 同源的
        // 时钟错配佐证(pre-T3 时 ref_input_drops≈0,T3 时 ref_stale_drops≈0,各取其一)。
        let ref_pace_loss = ref_pace_loss(self.ref_stale_drops, self.ref_input_drops);
        let clock_skew_event = self.clock_skew.observe(
            self.clock_frames,
            self.output_underruns,
            self.output_overruns,
            ref_pace_loss,
            self.ref_underruns,
        );
        // 审计 B-02:本方法在音频处理线程上执行,写出必须走异步发射器,
        // 不许同步碰 stdout(管道满会阻塞处理循环 → 爆音)。
        if self.status_json {
            emit_stdout_line(self.status_json_line(now));
        } else {
            emit_stdout_line(self.text_line(now));
        }
        if let Some(event) = clock_skew_event {
            emit_stdout_line(self.clock_skew_event_line(event));
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
        self.mic_stale_drops = 0;
        self.ref_stale_drops = 0;
        self.ref_underruns = 0;
        self.output_overruns = 0;
        self.output_underruns = 0;
        self.clock_frames = 0;
        self.node_process_time_ms = 0.0;
        self.node_diverged = false;
        self.node_last_error = None;
    }

    fn text_line(&self, now: Instant) -> String {
        format!(
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
            self.mic_stale_drops + self.ref_stale_drops,
            self.node_process_time_ms,
            self.node_runtime_errors,
            self.node_diverged,
        )
    }

    fn clock_skew_event_line(&self, event: ClockSkewEvent) -> String {
        if self.status_json {
            let event_type = match event.kind {
                ClockSkewEventKind::Warning => "clock_skew_warning",
                ClockSkewEventKind::Resolved => "clock_skew_resolved",
            };
            serde_json::to_string(&json!({
                "type": event_type,
                "output_skew_pct": event.snapshot.output_skew_pct,
                "ref_skew_pct": event.snapshot.ref_skew_pct,
                "ref_correlated": event.snapshot.ref_correlated,
                "direction": event.snapshot.direction.map(ClockSkewDirection::as_str),
                "hint": clock_skew_hint(event.snapshot),
            }))
            .unwrap_or_else(|err| {
                json!({ "type": "error", "message": err.to_string() }).to_string()
            })
        } else {
            match event.kind {
                ClockSkewEventKind::Warning => {
                    format!(
                    "clock_skew_warning direction={} output_skew_pct={:.1} ref_skew_pct={:.1}: {}",
                    event.snapshot.direction.map(ClockSkewDirection::as_str).unwrap_or("unknown"),
                    event.snapshot.output_skew_pct,
                    event.snapshot.ref_skew_pct,
                    clock_skew_hint(event.snapshot)
                )
                }
                ClockSkewEventKind::Resolved => format!(
                    "clock_skew_resolved direction={} output_skew_pct={:.1} ref_skew_pct={:.1}",
                    event
                        .snapshot
                        .direction
                        .map(ClockSkewDirection::as_str)
                        .unwrap_or("unknown"),
                    event.snapshot.output_skew_pct,
                    event.snapshot.ref_skew_pct
                ),
            }
        }
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
            "bypassed": self.bypassed,
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
            "mic_stale_drops": self.mic_stale_drops,
            "ref_stale_drops": self.ref_stale_drops,
            "stale_drops": self.mic_stale_drops + self.ref_stale_drops,
            "ref_underruns": self.ref_underruns,
            "output_underruns": self.output_underruns,
            "output_overruns": self.output_overruns,
            "output_skew_pct": self.clock_skew.snapshot.output_skew_pct,
            "ref_skew_pct": self.clock_skew.snapshot.ref_skew_pct,
            "clock_skew_warning": self.clock_skew.warning,
            "clock_skew_ref_correlated": self.clock_skew.snapshot.ref_correlated,
            "clock_skew_direction": self.clock_skew.snapshot.direction.map(ClockSkewDirection::as_str),
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

fn clock_skew_hint(snapshot: ClockSkewSnapshot) -> String {
    match snapshot.direction {
        Some(ClockSkewDirection::OutputFaster) => format!(
            "the output device clock is {:.1}% faster than the microphone; a virtual audio device's sample rate is likely mismatched — set all virtual endpoints and hardware outputs to 48000 Hz",
            snapshot.output_skew_pct.abs()
        ),
        Some(ClockSkewDirection::CaptureFaster) => format!(
            "the microphone clock is {:.1}% faster than the output/reference path; a virtual audio device's sample rate is likely mismatched — set all virtual endpoints and hardware outputs to 48000 Hz",
            snapshot.output_skew_pct.abs()
        ),
        None => "audio clock skew is no longer measurable".to_string(),
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
            bypassed: true,
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
        stats.mic_stale_drops = 3;
        stats.ref_stale_drops = 5;
        stats.clock_skew.snapshot = ClockSkewSnapshot {
            output_skew_pct: 22.4,
            ref_skew_pct: 22.1,
            ref_correlated: true,
            direction: Some(ClockSkewDirection::OutputFaster),
        };
        stats.clock_skew.warning = true;
        stats.aec_estimated_delay_ms = 48;
        stats.aec3_delay_blocks = Some(12);

        let value = stats.status_value(stats.started + Duration::from_secs(1));

        assert_eq!(value["type"], "status");
        assert_eq!(value["backend"], "localvqe");
        assert_eq!(value["input_drops"], 3);
        assert_eq!(value["mic_stale_drops"], 3);
        assert_eq!(value["ref_stale_drops"], 5);
        assert_eq!(value["stale_drops"], 8);
        assert_eq!(value["near_delay_ms"], 25);
        assert_eq!(value["output_level"], 75);
        assert_eq!(value["output_gain_db"], output_level_gain_db(75).unwrap());
        assert_eq!(value["bypassed"], true);
        assert_eq!(value["input_queue_latency_ms"], 10.0);
        assert_eq!(value["output_queue_latency_ms"], 50.0);
        assert_eq!(value["estimated_user_latency_ms"], 106.0);
        assert_eq!(value["aec_estimated_delay_ms"], 48);
        assert_eq!(value["aec3_delay_blocks"], 12);
        assert_eq!(value["output_skew_pct"], 22.4);
        assert_eq!(value["ref_skew_pct"], 22.1);
        assert_eq!(value["clock_skew_warning"], true);
        assert_eq!(value["clock_skew_ref_correlated"], true);
        assert_eq!(value["clock_skew_direction"], "output_faster_than_capture");
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

        stats.set_bypassed(false);
        let value = stats.status_value(stats.started + Duration::from_secs(4));
        assert_eq!(value["bypassed"], false);
    }

    #[test]
    fn clock_skew_detector_warns_after_correlated_five_second_window() {
        let mut detector = ClockSkewDetector::new(48_000);

        // 第 1 个满窗(5 次 observe):超阈值但 streak=1 < ENTER_WINDOWS,不告警
        // (单窗尖峰豁免 —— 切窗/调度抖动的瞬时 underrun 突发不算时钟错配)。
        for _ in 0..5 {
            assert!(detector.observe(48_000, 10_752, 0, 10_752, 0).is_none());
        }
        // 第 2 个连续满窗:streak=2 → 告警。恒定错配下 EMA 收敛于 raw,读数不失真。
        for _ in 0..4 {
            assert!(detector.observe(48_000, 10_752, 0, 10_752, 0).is_none());
        }
        let event = detector.observe(48_000, 10_752, 0, 10_752, 0).unwrap();

        assert_eq!(event.kind, ClockSkewEventKind::Warning);
        assert!(detector.warning);
        assert!((event.snapshot.output_skew_pct - 22.4).abs() < 0.001);
        assert!((event.snapshot.ref_skew_pct - 22.4).abs() < 0.001);
        assert!(event.snapshot.ref_correlated);
        assert_eq!(
            event.snapshot.direction,
            Some(ClockSkewDirection::OutputFaster)
        );
    }

    #[test]
    fn clock_skew_detector_warns_when_capture_is_faster() {
        let mut detector = ClockSkewDetector::new(48_000);

        for _ in 0..9 {
            assert!(detector.observe(48_000, 0, 10_752, 0, 10_752).is_none());
        }
        let event = detector.observe(48_000, 0, 10_752, 0, 10_752).unwrap();

        assert_eq!(event.kind, ClockSkewEventKind::Warning);
        assert!((event.snapshot.output_skew_pct + 22.4).abs() < 0.001);
        assert!((event.snapshot.ref_skew_pct + 22.4).abs() < 0.001);
        assert!(event.snapshot.ref_correlated);
        assert_eq!(
            event.snapshot.direction,
            Some(ClockSkewDirection::CaptureFaster)
        );
    }

    #[test]
    fn clock_skew_detector_uses_hysteresis_for_resolution() {
        let mut detector = ClockSkewDetector::new(48_000);
        for _ in 0..10 {
            let _ = detector.observe(48_000, 10_752, 0, 10_752, 0);
        }
        assert!(detector.warning);

        // 错配消失(计数归零):EMA 每满窗衰减一半,22.4% → …… → 跌破 EXIT(1%)
        // 才解除。平滑让恢复比 raw 慢几个窗口 —— 换来的是告警态不抖。
        let mut resolved = None;
        for _ in 0..40 {
            if let Some(event) = detector.observe(48_000, 0, 0, 0, 0) {
                resolved = Some(event);
                break;
            }
        }
        let event = resolved.expect("Resolved should fire after EMA decay");
        assert_eq!(event.kind, ClockSkewEventKind::Resolved);
        assert!(!detector.warning);
        assert!(event.snapshot.output_skew_pct.abs() < 1.0);
    }

    #[test]
    fn clock_skew_detector_ignores_transient_single_window_spike() {
        // 回归(2026-07-09 黑屏 RCA 遗留项):单个满窗的瞬时尖峰(如 cmd+tab 切窗
        // 挤出的 underrun 突发)不得触发告警 —— 尖峰过后回零,streak 断裂。
        let mut detector = ClockSkewDetector::new(48_000);
        for _ in 0..5 {
            assert!(detector.observe(48_000, 10_752, 0, 10_752, 0).is_none());
        }
        // 下一个满窗回零:streak 归零,始终无告警。
        for _ in 0..5 {
            assert!(detector.observe(48_000, 0, 0, 0, 0).is_none());
        }
        assert!(!detector.warning);
    }

    #[test]
    fn clock_skew_detector_ignores_uncorrelated_ref_counts() {
        let mut detector = ClockSkewDetector::new(48_000);

        for _ in 0..4 {
            assert!(detector.observe(48_000, 10_752, 0, 0, 0).is_none());
        }

        assert!(detector.observe(48_000, 10_752, 0, 0, 0).is_none());
        assert!(!detector.warning);
    }

    #[test]
    fn ref_pace_loss_bridges_t3_signal_migration() {
        // pre-T3:信号在 stale_drops,input_drops≈0。
        assert_eq!(ref_pace_loss(10_752, 0), 10_752);
        // T3:ref 连续重采样,stale_drops=0,信号迁移到 input_drops(参考 ring 溢出)。
        assert_eq!(ref_pace_loss(0, 10_752), 10_752);
        // 两路都有时相加(过渡态),saturating 不溢出。
        assert_eq!(ref_pace_loss(u64::MAX, 1), u64::MAX);

        let pre_t3 =
            ClockSkewSnapshot::from_frame_counts(48_000, 10_752, 0, ref_pace_loss(10_752, 0), 0);
        let t3 =
            ClockSkewSnapshot::from_frame_counts(48_000, 10_752, 0, ref_pace_loss(0, 10_752), 0);
        assert_eq!(pre_t3.output_skew_pct, t3.output_skew_pct);
        assert_eq!(pre_t3.ref_skew_pct, t3.ref_skew_pct);
        assert_eq!(pre_t3.ref_correlated, t3.ref_correlated);
    }

    #[test]
    fn clock_skew_still_warns_when_ref_signal_is_input_drops() {
        // T3 场景:ref_stale_drops 恒 0,参考侧佐证经 ref_input_drops 传入。检测器仍应告警
        // (回归护栏:T3 不得让 T1 的时钟错配告警失效)。
        let mut detector = ClockSkewDetector::new(48_000);
        // T3: stale=0, input=10_752;两个连续满窗(enter_streak 消抖)后告警。
        let ref_signal = ref_pace_loss(0, 10_752);
        for _ in 0..9 {
            assert!(detector.observe(48_000, 10_752, 0, ref_signal, 0).is_none());
        }
        let event = detector.observe(48_000, 10_752, 0, ref_signal, 0).unwrap();
        assert_eq!(event.kind, ClockSkewEventKind::Warning);
        assert!(event.snapshot.ref_correlated);
    }
}
