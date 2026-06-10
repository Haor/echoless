use std::fs::{create_dir, create_dir_all, rename, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::dsp::{rms_dbfs_from_sum_squares as rms_dbfs, sum_squares};
use anyhow::{bail, Context, Result};
use echoless_core::{output_level_gain_db, DiagnosticsConfig};
use echoless_processors::ProcessorStats;
use hound::{WavSpec, WavWriter};
use serde_json::json;

use super::print_human;
use super::stats::{
    aggregate_diverged, aggregate_estimated_delay_ms, aggregate_last_error,
    aggregate_process_time_ms, aggregate_runtime_errors, estimate_user_latency_ms,
    input_queue_latency_ms, output_queue_latency_ms, StatsSample,
};

const DIAGNOSTIC_QUEUE_FRAMES: usize = 128;

#[derive(Clone)]
pub(super) struct DiagnosticsStatusHandle {
    inner: Arc<DiagnosticsSharedState>,
    sample_rate: u32,
}

impl DiagnosticsStatusHandle {
    fn new(sample_rate: u32) -> Self {
        Self {
            inner: Arc::new(DiagnosticsSharedState {
                recording: AtomicBool::new(true),
                frames: AtomicU64::new(0),
                drops: AtomicU64::new(0),
            }),
            sample_rate,
        }
    }

    pub(super) fn is_recording(&self) -> bool {
        self.inner.recording.load(Ordering::Relaxed)
    }

    fn set_recording(&self, recording: bool) {
        self.inner.recording.store(recording, Ordering::Relaxed);
    }

    pub(super) fn frames(&self) -> u64 {
        self.inner.frames.load(Ordering::Relaxed)
    }

    fn set_frames(&self, frames: u64) {
        self.inner.frames.store(frames, Ordering::Relaxed);
    }

    pub(super) fn drops(&self) -> u64 {
        self.inner.drops.load(Ordering::Relaxed)
    }

    fn increment_drops(&self) {
        self.inner.drops.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn elapsed_s(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames() as f64 / self.sample_rate as f64
        }
    }
}

struct DiagnosticsSharedState {
    recording: AtomicBool,
    frames: AtomicU64,
    drops: AtomicU64,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum DiagnosticDoneReason {
    MaxSeconds,
    Stopped,
    RunExit,
    Error,
}

impl DiagnosticDoneReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::MaxSeconds => "max_seconds",
            Self::Stopped => "stopped",
            Self::RunExit => "run_exit",
            Self::Error => "error",
        }
    }
}

enum DiagnosticCommand {
    Frame(Box<DiagnosticFrame>),
    Finish(DiagnosticDoneReason),
}

struct DiagnosticFrame {
    frame_size: usize,
    algorithmic_latency_ms: f32,
    near_delay_ms: u32,
    near_delay_buffered_samples: usize,
    near: Vec<f32>,
    far: Vec<f32>,
    out: Vec<f32>,
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
    node_diverged: bool,
    node_last_error: Option<String>,
}

impl DiagnosticFrame {
    fn from_sample(sample: &StatsSample<'_>) -> Self {
        Self {
            frame_size: sample.frame_size,
            algorithmic_latency_ms: sample.algorithmic_latency_ms,
            near_delay_ms: sample.near_delay_ms,
            near_delay_buffered_samples: sample.near_delay_buffered_samples,
            near: sample.near.to_vec(),
            far: sample.far.to_vec(),
            out: sample.out.to_vec(),
            mic_q: sample.mic_q,
            ref_q: sample.ref_q,
            out_q: sample.out_q,
            mic_input_drops: sample.mic_input_drops,
            ref_input_drops: sample.ref_input_drops,
            stale_drops: sample.stale_drops,
            ref_underruns: sample.ref_underruns,
            output_overruns: sample.output_overruns,
            output_underruns: sample.output_underruns,
            node_process_time_ms: aggregate_process_time_ms(sample.node_stats),
            node_runtime_errors: aggregate_runtime_errors(sample.node_stats),
            aec_estimated_delay_ms: aggregate_estimated_delay_ms(sample.node_stats),
            node_diverged: aggregate_diverged(sample.node_stats),
            node_last_error: aggregate_last_error(sample.node_stats),
        }
    }
}

pub(super) struct DiagnosticRecorder {
    dir: PathBuf,
    sender: Option<SyncSender<DiagnosticCommand>>,
    writer: Option<JoinHandle<()>>,
    status: DiagnosticsStatusHandle,
}

pub(super) struct DiagnosticRecorderConfig<'a> {
    pub(super) cfg: &'a DiagnosticsConfig,
    pub(super) sample_rate: u32,
    pub(super) reference_channels: u16,
    pub(super) frame_ms: u32,
    pub(super) near_delay_ms: u32,
    pub(super) output_level: u32,
    pub(super) node_stats: &'a [ProcessorStats],
    pub(super) status_json: bool,
}

impl DiagnosticRecorder {
    pub(super) fn new(config: DiagnosticRecorderConfig<'_>) -> Result<Option<Self>> {
        let Some(record_dir) = config
            .cfg
            .record_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Ok(None);
        };
        let base = Path::new(record_dir);
        create_dir_all(base)
            .with_context(|| format!("创建诊断录制目录失败: {}", base.display()))?;
        let dir = make_session_dir(base)?;
        let spec = WavSpec {
            channels: 1,
            sample_rate: config.sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let ref_spec = WavSpec {
            channels: config.reference_channels.max(1),
            ..spec
        };
        let max_frames = config
            .cfg
            .max_seconds
            .map(|seconds| u64::from(seconds) * u64::from(config.sample_rate));

        write_diagnostic_metadata(DiagnosticMetadata {
            dir: &dir,
            sample_rate: config.sample_rate,
            frame_ms: config.frame_ms,
            near_delay_ms: config.near_delay_ms,
            output_level: config.output_level,
            reference_channels: config.reference_channels,
            max_frames,
            node_stats: config.node_stats,
        })?;
        let stats_part_path = dir.join("stats.csv.part");
        let mut stats = BufWriter::new(
            File::create(&stats_part_path)
                .with_context(|| format!("创建诊断 stats.csv 失败: {}", dir.display()))?,
        );
        writeln!(
            stats,
            "frame_index,frames,near_delay_ms,near_delay_buffered_samples,mic_dbfs,ref_dbfs,out_dbfs,mic_q,ref_q,out_q,input_queue_latency_ms,output_queue_latency_ms,estimated_user_latency_ms,aec_estimated_delay_ms,mic_input_drops,ref_input_drops,input_drops,stale_drops,ref_underruns,output_overruns,output_underruns,node_process_time_ms,node_runtime_errors,node_diverged,node_last_error"
        )?;

        let mic_part_path = dir.join("mic.wav.part");
        let ref_part_path = dir.join("ref.wav.part");
        let out_part_path = dir.join("out.wav.part");
        let status = DiagnosticsStatusHandle::new(config.sample_rate);
        let writer = DiagnosticWriter {
            dir: dir.clone(),
            mic: Some(WavWriter::create(&mic_part_path, spec)?),
            reference: Some(WavWriter::create(&ref_part_path, ref_spec)?),
            out: Some(WavWriter::create(&out_part_path, spec)?),
            stats: Some(stats),
            mic_part_path,
            ref_part_path,
            out_part_path,
            stats_part_path,
            mic_path: dir.join("mic.wav"),
            ref_path: dir.join("ref.wav"),
            out_path: dir.join("out.wav"),
            stats_path: dir.join("stats.csv"),
            sample_rate: config.sample_rate,
            frame_ms: config.frame_ms,
            max_frames,
            written_frames: 0,
            frame_index: 0,
            human_to_stderr: config.status_json,
            status_json: config.status_json,
            status: status.clone(),
        };
        let (sender, receiver) = sync_channel(DIAGNOSTIC_QUEUE_FRAMES);
        let writer = thread::spawn(move || writer.run(receiver));

        print_human(
            config.status_json,
            format!("诊断录制目录: {}", dir.display()),
        );
        Ok(Some(Self {
            dir,
            sender: Some(sender),
            writer: Some(writer),
            status,
        }))
    }

    pub(super) fn status_handle(&self) -> DiagnosticsStatusHandle {
        self.status.clone()
    }

    pub(super) fn is_recording(&self) -> bool {
        self.status.is_recording()
    }

    pub(super) fn session_dir_string(&self) -> String {
        self.dir.display().to_string()
    }

    pub(super) fn write_frame(&mut self, sample: &StatsSample<'_>) -> Result<bool> {
        if !self.status.is_recording() {
            return Ok(false);
        }
        let Some(sender) = self.sender.as_ref() else {
            return Ok(false);
        };
        match sender.try_send(DiagnosticCommand::Frame(Box::new(
            DiagnosticFrame::from_sample(sample),
        ))) {
            Ok(()) => Ok(true),
            Err(TrySendError::Full(_)) => {
                self.status.increment_drops();
                Ok(true)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.status.set_recording(false);
                bail!("诊断 writer 线程已退出")
            }
        }
    }

    pub(super) fn request_finish(&mut self, reason: DiagnosticDoneReason) {
        self.status.set_recording(false);
        if let Some(sender) = self.sender.take() {
            thread::spawn(move || {
                let _ = sender.send(DiagnosticCommand::Finish(reason));
            });
        }
    }

    fn finish(&mut self, reason: DiagnosticDoneReason) {
        if let Some(sender) = self.sender.take() {
            match sender.try_send(DiagnosticCommand::Finish(reason)) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => {}
                Err(TrySendError::Full(_)) => {}
            }
        }
        if let Some(writer) = self.writer.take() {
            if let Err(err) = writer.join() {
                eprintln!("诊断 writer 线程退出异常: {err:?}");
            }
        }
    }
}

impl Drop for DiagnosticRecorder {
    fn drop(&mut self) {
        self.finish(DiagnosticDoneReason::RunExit);
    }
}

struct DiagnosticWriter {
    dir: PathBuf,
    mic: Option<WavWriter<BufWriter<File>>>,
    reference: Option<WavWriter<BufWriter<File>>>,
    out: Option<WavWriter<BufWriter<File>>>,
    stats: Option<BufWriter<File>>,
    mic_part_path: PathBuf,
    ref_part_path: PathBuf,
    out_part_path: PathBuf,
    stats_part_path: PathBuf,
    mic_path: PathBuf,
    ref_path: PathBuf,
    out_path: PathBuf,
    stats_path: PathBuf,
    sample_rate: u32,
    frame_ms: u32,
    max_frames: Option<u64>,
    written_frames: u64,
    frame_index: u64,
    human_to_stderr: bool,
    status_json: bool,
    status: DiagnosticsStatusHandle,
}

impl DiagnosticWriter {
    fn run(mut self, receiver: Receiver<DiagnosticCommand>) {
        let mut reason = DiagnosticDoneReason::RunExit;
        let mut ok = true;
        while let Ok(command) = receiver.recv() {
            match command {
                DiagnosticCommand::Frame(frame) => match self.write_frame(&frame) {
                    Ok(true) => {}
                    Ok(false) => {
                        reason = DiagnosticDoneReason::MaxSeconds;
                        break;
                    }
                    Err(err) => {
                        eprintln!("诊断写入失败: {err:#}");
                        reason = DiagnosticDoneReason::Error;
                        ok = false;
                        break;
                    }
                },
                DiagnosticCommand::Finish(done_reason) => {
                    reason = done_reason;
                    break;
                }
            }
        }
        self.finish(reason, ok);
    }

    fn write_frame(&mut self, frame: &DiagnosticFrame) -> Result<bool> {
        if self
            .max_frames
            .is_some_and(|max_frames| self.written_frames >= max_frames)
        {
            return Ok(false);
        }

        if let Some(writer) = self.mic.as_mut() {
            for v in &frame.near {
                writer.write_sample(*v)?;
            }
        }
        if let Some(writer) = self.reference.as_mut() {
            for v in &frame.far {
                writer.write_sample(*v)?;
            }
        }
        if let Some(writer) = self.out.as_mut() {
            for v in &frame.out {
                writer.write_sample(*v)?;
            }
        }

        let Some(stats) = self.stats.as_mut() else {
            bail!("诊断 stats writer 已关闭");
        };
        writeln!(
            stats,
            "{},{},{},{},{:.2},{:.2},{:.2},{},{},{},{:.2},{:.2},{:.2},{},{},{},{},{},{},{},{},{:.3},{},{},{}",
            self.frame_index,
            frame.frame_size,
            frame.near_delay_ms,
            frame.near_delay_buffered_samples,
            rms_dbfs(sum_squares(&frame.near), frame.near.len() as u64),
            rms_dbfs(sum_squares(&frame.far), frame.far.len() as u64),
            rms_dbfs(sum_squares(&frame.out), frame.out.len() as u64),
            frame.mic_q,
            frame.ref_q,
            frame.out_q,
            input_queue_latency_ms(frame.mic_q, self.sample_rate),
            output_queue_latency_ms(frame.out_q, self.sample_rate),
            estimate_user_latency_ms(
                self.frame_ms,
                frame.near_delay_ms,
                frame.algorithmic_latency_ms,
                frame.mic_q,
                frame.out_q,
                self.sample_rate
            ),
            frame.aec_estimated_delay_ms,
            frame.mic_input_drops,
            frame.ref_input_drops,
            frame.mic_input_drops + frame.ref_input_drops,
            frame.stale_drops,
            frame.ref_underruns,
            frame.output_overruns,
            frame.output_underruns,
            frame.node_process_time_ms,
            frame.node_runtime_errors,
            frame.node_diverged,
            csv_escape(&frame.node_last_error.clone().unwrap_or_default()),
        )?;
        self.frame_index += 1;
        self.written_frames += frame.frame_size as u64;
        self.status.set_frames(self.written_frames);

        Ok(self
            .max_frames
            .is_none_or(|max_frames| self.written_frames < max_frames))
    }

    fn finish(&mut self, reason: DiagnosticDoneReason, ok_so_far: bool) {
        let mut ok = ok_so_far;
        ok &= self.finalize_wav("mic.wav", DiagnosticWavKind::Mic);
        ok &= self.finalize_wav("ref.wav", DiagnosticWavKind::Ref);
        ok &= self.finalize_wav("out.wav", DiagnosticWavKind::Out);
        ok &= self.finalize_stats();

        self.status.set_frames(self.written_frames);
        self.status.set_recording(false);
        self.emit_done(reason, ok);
    }

    fn finalize_wav(&mut self, label: &str, kind: DiagnosticWavKind) -> bool {
        let (writer, part_path, final_path) = match kind {
            DiagnosticWavKind::Mic => (&mut self.mic, &self.mic_part_path, &self.mic_path),
            DiagnosticWavKind::Ref => (&mut self.reference, &self.ref_part_path, &self.ref_path),
            DiagnosticWavKind::Out => (&mut self.out, &self.out_part_path, &self.out_path),
        };
        let Some(writer) = writer.take() else {
            return true;
        };
        if let Err(err) = writer.finalize() {
            eprintln!("写入 {label} 尾部失败: {err}");
            return false;
        }
        if let Err(err) = rename(part_path, final_path) {
            eprintln!("提交 {label} 失败: {err}");
            return false;
        }
        true
    }

    fn finalize_stats(&mut self) -> bool {
        let Some(mut stats) = self.stats.take() else {
            return true;
        };
        let mut ok = true;
        if let Err(err) = stats.flush() {
            eprintln!("刷新诊断 stats.csv 失败: {err}");
            ok = false;
        }
        drop(stats);
        if let Err(err) = rename(&self.stats_part_path, &self.stats_path) {
            eprintln!("提交诊断 stats.csv 失败: {err}");
            ok = false;
        }
        ok
    }

    fn emit_done(&self, reason: DiagnosticDoneReason, ok: bool) {
        if self.status_json {
            let event = json!({
                "type": "diagnostics_done",
                "session_dir": self.dir.display().to_string(),
                "frames": self.written_frames,
                "seconds": self.status.elapsed_s(),
                "reason": reason.as_str(),
                "drops": self.status.drops(),
                "ok": ok,
            });
            println!("{event}");
        } else {
            print_human(
                self.human_to_stderr,
                format!(
                    "诊断录制完成(reason={}, ok={}, drops={}): {}",
                    reason.as_str(),
                    ok,
                    self.status.drops(),
                    self.dir.display()
                ),
            );
        }
    }
}

enum DiagnosticWavKind {
    Mic,
    Ref,
    Out,
}

fn make_session_dir(base: &Path) -> Result<PathBuf> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("系统时间早于 UNIX_EPOCH")?
        .as_secs();
    for attempt in 0..1000 {
        let name = if attempt == 0 {
            format!("session-{now}")
        } else {
            format!("session-{now}-{attempt}")
        };
        let dir = base.join(name);
        match create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("创建诊断 session 目录失败: {}", dir.display()));
            }
        }
    }
    bail!("创建诊断 session 目录失败: {} 下重名过多", base.display())
}

struct DiagnosticMetadata<'a> {
    dir: &'a Path,
    sample_rate: u32,
    frame_ms: u32,
    near_delay_ms: u32,
    output_level: u32,
    reference_channels: u16,
    max_frames: Option<u64>,
    node_stats: &'a [ProcessorStats],
}

fn write_diagnostic_metadata(metadata: DiagnosticMetadata<'_>) -> Result<()> {
    let mut file = BufWriter::new(
        File::create(metadata.dir.join("metadata.txt"))
            .with_context(|| format!("创建诊断 metadata.txt 失败: {}", metadata.dir.display()))?,
    );
    writeln!(file, "version={}", env!("CARGO_PKG_VERSION"))?;
    writeln!(file, "sample_rate={}", metadata.sample_rate)?;
    writeln!(file, "frame_ms={}", metadata.frame_ms)?;
    writeln!(file, "near_delay_ms={}", metadata.near_delay_ms)?;
    writeln!(file, "output_level={}", metadata.output_level)?;
    match output_level_gain_db(metadata.output_level) {
        Some(gain_db) => writeln!(file, "output_gain_db={gain_db:.3}")?,
        None => writeln!(file, "output_gain_db=mute")?,
    }
    writeln!(file, "reference_channels={}", metadata.reference_channels)?;
    if let Some(max_frames) = metadata.max_frames {
        writeln!(file, "max_frames={max_frames}")?;
    } else {
        writeln!(file, "max_frames=unbounded")?;
    }
    for (index, node) in metadata.node_stats.iter().enumerate() {
        writeln!(file, "node.{index}.name={}", node.name)?;
        if let Some(arch) = &node.selected_gpu_arch {
            writeln!(file, "node.{index}.selected_gpu_arch={arch}")?;
        }
        if let Some(model) = &node.selected_model {
            writeln!(file, "node.{index}.selected_model={model}")?;
        }
        if let Some(err) = &node.last_backend_error {
            writeln!(file, "node.{index}.last_backend_error={err}")?;
        }
    }
    file.flush()?;
    Ok(())
}

fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use echoless_processors::ProcessorStats;

    fn temp_diagnostic_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn diagnostic_recorder_writes_audio_and_stats() -> Result<()> {
        let base = temp_diagnostic_dir("echoless-diagnostic-test");
        let cfg = DiagnosticsConfig {
            record_dir: Some(base.to_string_lossy().to_string()),
            max_seconds: Some(1),
        };
        let node_stats = [ProcessorStats::empty("test")];
        let mut recorder = DiagnosticRecorder::new(DiagnosticRecorderConfig {
            cfg: &cfg,
            sample_rate: 48_000,
            reference_channels: 2,
            frame_ms: 10,
            near_delay_ms: 25,
            output_level: 75,
            node_stats: &node_stats,
            status_json: false,
        })?
        .unwrap();
        let dir = recorder.dir.clone();
        let near = [0.25f32, -0.25];
        let far = [0.1f32, -0.1, 0.2, -0.2];
        let out = [0.0f32, 0.1];
        let sample = StatsSample {
            algorithmic_latency_ms: 16.0,
            near_delay_ms: 25,
            near_delay_buffered_samples: 1200,
            frame_size: near.len(),
            near: &near,
            far: &far,
            out: &out,
            mic_q: 480,
            ref_q: 0,
            out_q: 0,
            mic_input_drops: 0,
            ref_input_drops: 0,
            stale_drops: 0,
            ref_underruns: 0,
            output_overruns: 0,
            output_underruns: 0,
            node_stats: &node_stats,
        };

        assert!(recorder.write_frame(&sample)?);
        drop(recorder);

        let mic_reader = hound::WavReader::open(dir.join("mic.wav"))?;
        assert_eq!(mic_reader.spec().channels, 1);
        assert_eq!(mic_reader.spec().sample_rate, 48_000);
        let ref_reader = hound::WavReader::open(dir.join("ref.wav"))?;
        assert_eq!(ref_reader.spec().channels, 2);
        let stats = std::fs::read_to_string(dir.join("stats.csv"))?;
        assert_eq!(stats.lines().count(), 2);
        assert!(stats.contains("node_process_time_ms"));
        assert!(stats.contains("input_queue_latency_ms"));
        assert!(stats.contains("estimated_user_latency_ms"));
        assert!(stats.contains("near_delay_ms"));
        assert!(stats.contains(",10.00,0.00,56.00,"));
        assert!(dir.join("out.wav").exists());
        let metadata = std::fs::read_to_string(dir.join("metadata.txt"))?;
        assert!(metadata.contains("near_delay_ms=25"));
        assert!(metadata.contains("output_level=75"));

        let _ = std::fs::remove_dir_all(base);
        Ok(())
    }

    #[test]
    fn diagnostic_recorder_auto_finishes_at_max_seconds() -> Result<()> {
        let base = temp_diagnostic_dir("echoless-diagnostic-max-test");
        let cfg = DiagnosticsConfig {
            record_dir: Some(base.to_string_lossy().to_string()),
            max_seconds: Some(1),
        };
        let node_stats = [ProcessorStats::empty("test")];
        let mut recorder = DiagnosticRecorder::new(DiagnosticRecorderConfig {
            cfg: &cfg,
            sample_rate: 2,
            reference_channels: 1,
            frame_ms: 1000,
            near_delay_ms: 0,
            output_level: 50,
            node_stats: &node_stats,
            status_json: false,
        })?
        .unwrap();
        let dir = recorder.dir.clone();
        let status = recorder.status_handle();
        let near = [0.25f32, -0.25];
        let far = [0.1f32, -0.1];
        let out = [0.0f32, 0.1];
        let sample = StatsSample {
            algorithmic_latency_ms: 16.0,
            near_delay_ms: 0,
            near_delay_buffered_samples: 0,
            frame_size: near.len(),
            near: &near,
            far: &far,
            out: &out,
            mic_q: 0,
            ref_q: 0,
            out_q: 0,
            mic_input_drops: 0,
            ref_input_drops: 0,
            stale_drops: 0,
            ref_underruns: 0,
            output_overruns: 0,
            output_underruns: 0,
            node_stats: &node_stats,
        };

        assert!(recorder.write_frame(&sample)?);
        for _ in 0..50 {
            if !status.is_recording() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(!status.is_recording());
        assert_eq!(status.frames(), 2);
        assert_eq!(status.elapsed_s(), 1.0);
        assert!(dir.join("mic.wav").exists());
        assert!(dir.join("ref.wav").exists());
        assert!(dir.join("out.wav").exists());
        assert!(dir.join("stats.csv").exists());
        assert!(!dir.join("mic.wav.part").exists());
        assert!(!dir.join("stats.csv.part").exists());

        let _ = std::fs::remove_dir_all(base);
        Ok(())
    }

    #[test]
    fn diagnostic_recorder_async_stop_finalizes_files() -> Result<()> {
        let base = temp_diagnostic_dir("echoless-diagnostic-stop-test");
        let cfg = DiagnosticsConfig {
            record_dir: Some(base.to_string_lossy().to_string()),
            max_seconds: None,
        };
        let node_stats = [ProcessorStats::empty("test")];
        let mut recorder = DiagnosticRecorder::new(DiagnosticRecorderConfig {
            cfg: &cfg,
            sample_rate: 48_000,
            reference_channels: 1,
            frame_ms: 10,
            near_delay_ms: 0,
            output_level: 50,
            node_stats: &node_stats,
            status_json: false,
        })?
        .unwrap();
        let dir = recorder.dir.clone();
        let status = recorder.status_handle();
        let near = [0.25f32, -0.25];
        let far = [0.1f32, -0.1];
        let out = [0.0f32, 0.1];
        let sample = StatsSample {
            algorithmic_latency_ms: 16.0,
            near_delay_ms: 0,
            near_delay_buffered_samples: 0,
            frame_size: near.len(),
            near: &near,
            far: &far,
            out: &out,
            mic_q: 0,
            ref_q: 0,
            out_q: 0,
            mic_input_drops: 0,
            ref_input_drops: 0,
            stale_drops: 0,
            ref_underruns: 0,
            output_overruns: 0,
            output_underruns: 0,
            node_stats: &node_stats,
        };

        assert!(recorder.write_frame(&sample)?);
        recorder.request_finish(DiagnosticDoneReason::Stopped);
        assert!(!status.is_recording());
        for _ in 0..100 {
            if dir.join("stats.csv").exists() && dir.join("mic.wav").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(dir.join("mic.wav").exists());
        assert!(dir.join("ref.wav").exists());
        assert!(dir.join("out.wav").exists());
        assert!(dir.join("stats.csv").exists());

        let _ = std::fs::remove_dir_all(base);
        Ok(())
    }

    #[test]
    fn diagnostic_recorder_stop_keeps_writer_joinable() -> Result<()> {
        let base = temp_diagnostic_dir("echoless-diagnostic-join-test");
        let cfg = DiagnosticsConfig {
            record_dir: Some(base.to_string_lossy().to_string()),
            max_seconds: None,
        };
        let node_stats = [ProcessorStats::empty("test")];
        let mut recorder = DiagnosticRecorder::new(DiagnosticRecorderConfig {
            cfg: &cfg,
            sample_rate: 48_000,
            reference_channels: 1,
            frame_ms: 10,
            near_delay_ms: 0,
            output_level: 50,
            node_stats: &node_stats,
            status_json: false,
        })?
        .unwrap();

        recorder.request_finish(DiagnosticDoneReason::Stopped);

        let dir = recorder.dir.clone();
        assert!(recorder.writer.is_some());

        drop(recorder);
        assert!(dir.join("mic.wav").exists());
        assert!(dir.join("stats.csv").exists());
        assert!(!dir.join("mic.wav.part").exists());
        assert!(!dir.join("stats.csv.part").exists());

        let _ = std::fs::remove_dir_all(base);
        Ok(())
    }
}
