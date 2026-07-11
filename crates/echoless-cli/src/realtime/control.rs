use std::collections::VecDeque;
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::thread;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use echoless_core::{
    output_level_gain_db, MAX_INITIAL_DELAY_MS, MAX_NEAR_DELAY_MS, MAX_OUTPUT_LEVEL,
};

use super::diagnostics::{DiagnosticDoneReason, DiagnosticRecorder, DiagnosticRecorderConfig};
use super::emit::emit_stdout_line;
use super::stats::RealtimeStats;
use echoless_processors::ProcessorChain;

#[derive(Debug)]
pub(super) enum RuntimeControlCommand {
    StartDiagnostics { max_seconds: Option<u32> },
    StopDiagnostics,
    SetOutputLevel(u32),
    SetBypass(bool),
    SetNearDelayMs(u32),
    SetInitialDelayMs(i32),
    SetAec3Ns { enabled: bool, level: String },
    SetAec3Agc(bool),
    SetLocalvqeNoiseGate { enabled: bool, threshold_dbfs: f32 },
}

pub(super) const SUPPORTED_RUNTIME_CONTROLS: &[&str] = &[
    "start_diagnostics",
    "stop_diagnostics",
    "set_output_level",
    "set_bypass",
    "set_near_delay_ms",
    "set_initial_delay_ms",
    "set_aec3_ns",
    "set_aec3_agc",
    "set_localvqe_noise_gate",
];

#[derive(Debug)]
pub(super) enum RuntimeControlEvent {
    Command(RuntimeControlCommand),
    Error(String),
}

pub(super) fn spawn_control_reader() -> Receiver<RuntimeControlEvent> {
    let (sender, receiver) = channel();
    thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let event = match line {
                Ok(line) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match parse_runtime_control_command(trimmed) {
                        Ok(command) => RuntimeControlEvent::Command(command),
                        Err(err) => RuntimeControlEvent::Error(format!(
                            "invalid runtime control JSON: {err}; line={trimmed}"
                        )),
                    }
                }
                Err(err) => {
                    RuntimeControlEvent::Error(format!("runtime control stdin read failed: {err}"))
                }
            };
            if sender.send(event).is_err() {
                break;
            }
        }
    });
    receiver
}

fn parse_runtime_control_command(line: &str) -> Result<RuntimeControlCommand> {
    let value: Value = serde_json::from_str(line)?;
    let cmd = value
        .get("cmd")
        .and_then(Value::as_str)
        .context("missing string field `cmd`")?;
    match cmd {
        "start_diagnostics" => {
            let max_seconds =
                match value.get("max_seconds") {
                    None | Some(Value::Null) => None,
                    Some(v) => {
                        let seconds = v.as_u64().context(
                            "start_diagnostics `max_seconds` must be a positive integer",
                        )?;
                        Some(u32::try_from(seconds).context(
                            "start_diagnostics `max_seconds` is too large for this backend",
                        )?)
                    }
                };
            Ok(RuntimeControlCommand::StartDiagnostics { max_seconds })
        }
        "stop_diagnostics" => Ok(RuntimeControlCommand::StopDiagnostics),
        "set_output_level" => {
            let level = value
                .get("level")
                .and_then(Value::as_u64)
                .context("set_output_level requires integer field `level`")?;
            if level > u64::from(MAX_OUTPUT_LEVEL) {
                bail!("set_output_level `level` must be <= {MAX_OUTPUT_LEVEL}");
            }
            Ok(RuntimeControlCommand::SetOutputLevel(level as u32))
        }
        "set_bypass" => {
            let enabled = value
                .get("enabled")
                .and_then(Value::as_bool)
                .context("set_bypass requires boolean field `enabled`")?;
            Ok(RuntimeControlCommand::SetBypass(enabled))
        }
        "set_near_delay_ms" => {
            let delay_ms = value
                .get("near_delay_ms")
                .and_then(Value::as_u64)
                .context("set_near_delay_ms requires integer field `near_delay_ms`")?;
            if delay_ms > u64::from(MAX_NEAR_DELAY_MS) {
                bail!("set_near_delay_ms `near_delay_ms` must be <= {MAX_NEAR_DELAY_MS}");
            }
            Ok(RuntimeControlCommand::SetNearDelayMs(delay_ms as u32))
        }
        "set_initial_delay_ms" => {
            let delay_ms = value
                .get("initial_delay_ms")
                .and_then(Value::as_i64)
                .context("set_initial_delay_ms requires integer field `initial_delay_ms`")?;
            if delay_ms < 0 || delay_ms > i64::from(MAX_INITIAL_DELAY_MS) {
                bail!("set_initial_delay_ms `initial_delay_ms` must be between 0 and {MAX_INITIAL_DELAY_MS}");
            }
            Ok(RuntimeControlCommand::SetInitialDelayMs(delay_ms as i32))
        }
        "set_aec3_ns" => {
            let enabled = value
                .get("ns")
                .and_then(Value::as_bool)
                .context("set_aec3_ns requires boolean field `ns`")?;
            let level = value
                .get("ns_level")
                .and_then(Value::as_str)
                .context("set_aec3_ns requires string field `ns_level`")?;
            if !is_valid_ns_level(level) {
                bail!("set_aec3_ns `ns_level` must be one of: low, moderate, high, veryhigh");
            }
            Ok(RuntimeControlCommand::SetAec3Ns {
                enabled,
                level: level.to_string(),
            })
        }
        "set_aec3_agc" => {
            let enabled = value
                .get("agc")
                .and_then(Value::as_bool)
                .context("set_aec3_agc requires boolean field `agc`")?;
            Ok(RuntimeControlCommand::SetAec3Agc(enabled))
        }
        "set_localvqe_noise_gate" => {
            let enabled = value
                .get("noise_gate")
                .and_then(Value::as_bool)
                .context("set_localvqe_noise_gate requires boolean field `noise_gate`")?;
            let threshold_dbfs = value
                .get("noise_gate_threshold_dbfs")
                .and_then(Value::as_f64)
                .context(
                    "set_localvqe_noise_gate requires number field `noise_gate_threshold_dbfs`",
                )?;
            if !threshold_dbfs.is_finite() {
                bail!("set_localvqe_noise_gate `noise_gate_threshold_dbfs` must be finite");
            }
            Ok(RuntimeControlCommand::SetLocalvqeNoiseGate {
                enabled,
                threshold_dbfs: threshold_dbfs as f32,
            })
        }
        other => bail!("unknown runtime control command `{other}`"),
    }
}

pub(super) struct RuntimeControlContext<'a> {
    pub(super) diagnostic: &'a mut Option<DiagnosticRecorder>,
    pub(super) stats: Option<&'a mut RealtimeStats>,
    pub(super) chain: &'a mut ProcessorChain,
    pub(super) sample_rate: u32,
    pub(super) reference_channels: u16,
    pub(super) frame_ms: u32,
    pub(super) near_delay_ms: &'a mut u32,
    pub(super) near_delay_samples: &'a mut usize,
    pub(super) near_delay_buffer: &'a mut VecDeque<f32>,
    pub(super) output_level: &'a mut u32,
    pub(super) bypassed: &'a mut bool,
    pub(super) status_json: bool,
    pub(super) running: &'a AtomicBool,
}

pub(super) fn handle_runtime_controls(
    control: &mut Option<Receiver<RuntimeControlEvent>>,
    mut ctx: RuntimeControlContext<'_>,
) {
    let Some(receiver) = control.as_mut() else {
        return;
    };
    loop {
        match receiver.try_recv() {
            Ok(RuntimeControlEvent::Command(command)) => {
                handle_runtime_control_command(
                    command,
                    RuntimeControlCommandContext {
                        diagnostic: ctx.diagnostic,
                        stats: ctx.stats.as_deref_mut(),
                        chain: ctx.chain,
                        sample_rate: ctx.sample_rate,
                        reference_channels: ctx.reference_channels,
                        frame_ms: ctx.frame_ms,
                        near_delay_ms: ctx.near_delay_ms,
                        near_delay_samples: ctx.near_delay_samples,
                        near_delay_buffer: ctx.near_delay_buffer,
                        output_level: ctx.output_level,
                        bypassed: ctx.bypassed,
                        status_json: ctx.status_json,
                    },
                );
            }
            Ok(RuntimeControlEvent::Error(message)) => {
                emit_control_error(ctx.status_json, None, message);
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                // stdin 关闭 = 停机契约(审计 B-01):GUI 优雅停止先关 stdin;
                // GUI 崩溃/被杀时管道同样关闭。两种情况都收敛停机,
                // 避免孤儿 CLI 继续占用麦克风/虚拟麦/Process Tap helper。
                *control = None;
                ctx.running.store(false, Ordering::SeqCst);
                break;
            }
        }
    }
}

struct RuntimeControlCommandContext<'a> {
    diagnostic: &'a mut Option<DiagnosticRecorder>,
    stats: Option<&'a mut RealtimeStats>,
    chain: &'a mut ProcessorChain,
    sample_rate: u32,
    reference_channels: u16,
    frame_ms: u32,
    near_delay_ms: &'a mut u32,
    near_delay_samples: &'a mut usize,
    near_delay_buffer: &'a mut VecDeque<f32>,
    output_level: &'a mut u32,
    bypassed: &'a mut bool,
    status_json: bool,
}

fn handle_runtime_control_command(
    command: RuntimeControlCommand,
    ctx: RuntimeControlCommandContext<'_>,
) {
    match command {
        RuntimeControlCommand::StartDiagnostics { max_seconds } => {
            if matches!(max_seconds, Some(0)) {
                emit_control_error(
                    ctx.status_json,
                    Some("start_diagnostics"),
                    "max_seconds must be greater than 0",
                );
                return;
            }
            if ctx
                .diagnostic
                .as_ref()
                .is_some_and(DiagnosticRecorder::is_recording)
            {
                emit_control_error(
                    ctx.status_json,
                    Some("start_diagnostics"),
                    "diagnostics is already recording",
                );
                return;
            }

            let _previous = ctx.diagnostic.take();
            let node_stats = ctx.chain.stats();
            match DiagnosticRecorder::new(DiagnosticRecorderConfig {
                enabled: true,
                max_seconds,
                sample_rate: ctx.sample_rate,
                reference_channels: ctx.reference_channels,
                frame_ms: ctx.frame_ms,
                near_delay_ms: *ctx.near_delay_ms,
                output_level: *ctx.output_level,
                node_stats: &node_stats,
                status_json: ctx.status_json,
            }) {
                Ok(Some(recorder)) => {
                    let session_dir = recorder.session_dir_string();
                    let status = recorder.status_handle();
                    if let Some(stats) = ctx.stats {
                        stats.set_diagnostics(Some(session_dir.clone()), Some(status));
                    }
                    *ctx.diagnostic = Some(recorder);
                    emit_runtime_json(
                        ctx.status_json,
                        json!({
                            "type": "diagnostics_started",
                            "session_dir": session_dir,
                            "max_seconds": max_seconds,
                            "recording": true,
                        }),
                    );
                }
                Ok(None) => emit_control_error(
                    ctx.status_json,
                    Some("start_diagnostics"),
                    "diagnostics recorder was not created",
                ),
                Err(err) => emit_control_error(
                    ctx.status_json,
                    Some("start_diagnostics"),
                    format!("failed to start diagnostics: {err:#}"),
                ),
            }
        }
        RuntimeControlCommand::StopDiagnostics => {
            let Some(recorder) = ctx.diagnostic.as_mut() else {
                emit_control_error(
                    ctx.status_json,
                    Some("stop_diagnostics"),
                    "diagnostics is not active",
                );
                return;
            };
            if !recorder.is_recording() {
                emit_control_error(
                    ctx.status_json,
                    Some("stop_diagnostics"),
                    "diagnostics is already stopping or stopped",
                );
                return;
            }
            let session_dir = recorder.session_dir_string();
            recorder.request_finish(DiagnosticDoneReason::Stopped);
            emit_runtime_json(
                ctx.status_json,
                json!({
                    "type": "diagnostics_stopping",
                    "session_dir": session_dir,
                }),
            );
        }
        RuntimeControlCommand::SetOutputLevel(level) => {
            *ctx.output_level = level;
            if let Some(stats) = ctx.stats {
                stats.set_output_level(level);
            }
            emit_runtime_json(
                ctx.status_json,
                json!({
                    "type": "output_level_changed",
                    "output_level": level,
                    "output_gain_db": output_level_gain_db(level),
                }),
            );
        }
        RuntimeControlCommand::SetBypass(enabled) => {
            *ctx.bypassed = enabled;
            if let Some(stats) = ctx.stats {
                stats.set_bypassed(enabled);
            }
            emit_runtime_json(
                ctx.status_json,
                json!({
                    "type": "bypass_changed",
                    "bypassed": enabled,
                }),
            );
        }
        RuntimeControlCommand::SetNearDelayMs(delay_ms) => {
            let delay_samples = delay_ms_to_samples(delay_ms, ctx.sample_rate);
            retune_near_delay_buffer(ctx.near_delay_buffer, delay_samples);
            *ctx.near_delay_ms = delay_ms;
            *ctx.near_delay_samples = delay_samples;
            if let Some(stats) = ctx.stats {
                stats.set_near_delay_ms(delay_ms);
            }
            emit_runtime_json(
                ctx.status_json,
                json!({
                    "type": "near_delay_changed",
                    "near_delay_ms": delay_ms,
                    "near_delay_samples": delay_samples,
                }),
            );
        }
        RuntimeControlCommand::SetInitialDelayMs(delay_ms) => {
            ctx.chain.set_stream_delay_ms(delay_ms);
            emit_runtime_json(
                ctx.status_json,
                json!({
                    "type": "initial_delay_changed",
                    "initial_delay_ms": delay_ms,
                }),
            );
        }
        RuntimeControlCommand::SetAec3Ns { enabled, level } => {
            let ns_value = toml::Value::Boolean(enabled);
            let level_value = toml::Value::String(level.clone());
            let result = ctx
                .chain
                .set_runtime_param("aec3", "ns", &ns_value)
                .and_then(|ns_applied| {
                    ctx.chain
                        .set_runtime_param("aec3", "ns_level", &level_value)
                        .map(|level_applied| ns_applied + level_applied)
                });
            match result {
                Ok(applied) if applied > 0 => emit_runtime_json(
                    ctx.status_json,
                    json!({
                        "type": "aec3_ns_changed",
                        "ns": enabled,
                        "ns_level": level,
                    }),
                ),
                Ok(_) => emit_control_error(
                    ctx.status_json,
                    Some("set_aec3_ns"),
                    "aec3 is not present in the active chain",
                ),
                Err(err) => emit_control_error(
                    ctx.status_json,
                    Some("set_aec3_ns"),
                    format!("failed to update AEC3 NS: {err:#}"),
                ),
            }
        }
        RuntimeControlCommand::SetAec3Agc(enabled) => {
            let value = toml::Value::Boolean(enabled);
            match ctx.chain.set_runtime_param("aec3", "agc", &value) {
                Ok(applied) if applied > 0 => emit_runtime_json(
                    ctx.status_json,
                    json!({
                        "type": "aec3_agc_changed",
                        "agc": enabled,
                    }),
                ),
                Ok(_) => emit_control_error(
                    ctx.status_json,
                    Some("set_aec3_agc"),
                    "aec3 is not present in the active chain",
                ),
                Err(err) => emit_control_error(
                    ctx.status_json,
                    Some("set_aec3_agc"),
                    format!("failed to update AEC3 AGC: {err:#}"),
                ),
            }
        }
        RuntimeControlCommand::SetLocalvqeNoiseGate {
            enabled,
            threshold_dbfs,
        } => {
            let gate_value = toml::Value::Boolean(enabled);
            let threshold_value = toml::Value::Float(f64::from(threshold_dbfs));
            let result = ctx
                .chain
                .set_runtime_param("localvqe", "noise_gate", &gate_value)
                .and_then(|gate_applied| {
                    ctx.chain
                        .set_runtime_param(
                            "localvqe",
                            "noise_gate_threshold_dbfs",
                            &threshold_value,
                        )
                        .map(|threshold_applied| gate_applied + threshold_applied)
                });
            match result {
                Ok(applied) if applied > 0 => emit_runtime_json(
                    ctx.status_json,
                    json!({
                        "type": "localvqe_noise_gate_changed",
                        "noise_gate": enabled,
                        "noise_gate_threshold_dbfs": threshold_dbfs,
                    }),
                ),
                Ok(_) => emit_control_error(
                    ctx.status_json,
                    Some("set_localvqe_noise_gate"),
                    "localvqe is not present in the active chain",
                ),
                Err(err) => emit_control_error(
                    ctx.status_json,
                    Some("set_localvqe_noise_gate"),
                    format!("failed to update LocalVQE noise gate: {err:#}"),
                ),
            }
        }
    }
}

pub(super) fn delay_ms_to_samples(ms: u32, sample_rate: u32) -> usize {
    ((u64::from(ms) * u64::from(sample_rate) + 500) / 1000) as usize
}

fn is_valid_ns_level(level: &str) -> bool {
    matches!(
        level.to_ascii_lowercase().as_str(),
        "low" | "moderate" | "high" | "veryhigh" | "very_high" | "very-high"
    )
}

fn retune_near_delay_buffer(delay: &mut VecDeque<f32>, target_samples: usize) {
    if target_samples == 0 {
        delay.clear();
        return;
    }
    while delay.len() > target_samples {
        let _ = delay.pop_front();
    }
    while delay.len() < target_samples {
        delay.push_front(0.0);
    }
}

fn emit_control_error(
    status_json: bool,
    command: Option<&'static str>,
    message: impl Into<String>,
) {
    emit_runtime_json(
        status_json,
        json!({
            "type": "control_error",
            "cmd": command,
            "message": message.into(),
        }),
    );
}

fn emit_runtime_json(status_json: bool, value: Value) {
    if status_json {
        // 审计 B-02:命令回执在音频处理线程发出,走异步发射器,不同步碰 stdout。
        emit_stdout_line(value.to_string());
    } else {
        eprintln!("{value}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_control_command_parses_frontend_json() {
        let start =
            parse_runtime_control_command(r#"{"cmd":"start_diagnostics","max_seconds":10}"#)
                .unwrap();
        match start {
            RuntimeControlCommand::StartDiagnostics { max_seconds } => {
                assert_eq!(max_seconds, Some(10));
            }
            other => panic!("expected start_diagnostics, got {other:?}"),
        }

        let stop = parse_runtime_control_command(r#"{"cmd":"stop_diagnostics"}"#).unwrap();
        assert!(matches!(stop, RuntimeControlCommand::StopDiagnostics));

        let set_level =
            parse_runtime_control_command(r#"{"cmd":"set_output_level","level":75}"#).unwrap();
        assert!(matches!(
            set_level,
            RuntimeControlCommand::SetOutputLevel(75)
        ));

        let err =
            parse_runtime_control_command(r#"{"cmd":"set_output_level","level":101}"#).unwrap_err();
        assert!(err.to_string().contains("<= 100"));

        let set_bypass =
            parse_runtime_control_command(r#"{"cmd":"set_bypass","enabled":true}"#).unwrap();
        assert!(matches!(set_bypass, RuntimeControlCommand::SetBypass(true)));

        let clear_bypass =
            parse_runtime_control_command(r#"{"cmd":"set_bypass","enabled":false}"#).unwrap();
        assert!(matches!(
            clear_bypass,
            RuntimeControlCommand::SetBypass(false)
        ));

        let err =
            parse_runtime_control_command(r#"{"cmd":"set_bypass","enabled":"yes"}"#).unwrap_err();
        assert!(err.to_string().contains("boolean field `enabled`"));

        let set_delay =
            parse_runtime_control_command(r#"{"cmd":"set_near_delay_ms","near_delay_ms":25}"#)
                .unwrap();
        assert!(matches!(
            set_delay,
            RuntimeControlCommand::SetNearDelayMs(25)
        ));

        let err =
            parse_runtime_control_command(r#"{"cmd":"set_near_delay_ms","near_delay_ms":501}"#)
                .unwrap_err();
        assert!(err.to_string().contains("<= 500"));

        let set_initial =
            parse_runtime_control_command(r#"{"cmd":"set_initial_delay_ms","initial_delay_ms":8}"#)
                .unwrap();
        assert!(matches!(
            set_initial,
            RuntimeControlCommand::SetInitialDelayMs(8)
        ));

        let clear_initial =
            parse_runtime_control_command(r#"{"cmd":"set_initial_delay_ms","initial_delay_ms":0}"#)
                .unwrap();
        assert!(matches!(
            clear_initial,
            RuntimeControlCommand::SetInitialDelayMs(0)
        ));

        let err = parse_runtime_control_command(
            r#"{"cmd":"set_initial_delay_ms","initial_delay_ms":501}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("between 0 and 500"));

        let set_ns =
            parse_runtime_control_command(r#"{"cmd":"set_aec3_ns","ns":true,"ns_level":"high"}"#)
                .unwrap();
        assert!(matches!(
            set_ns,
            RuntimeControlCommand::SetAec3Ns {
                enabled: true,
                ref level
            } if level == "high"
        ));

        let err =
            parse_runtime_control_command(r#"{"cmd":"set_aec3_ns","ns":true,"ns_level":"max"}"#)
                .unwrap_err();
        assert!(err.to_string().contains("ns_level"));

        let set_agc =
            parse_runtime_control_command(r#"{"cmd":"set_aec3_agc","agc":false}"#).unwrap();
        assert!(matches!(set_agc, RuntimeControlCommand::SetAec3Agc(false)));

        let set_localvqe_noise_gate = parse_runtime_control_command(
            r#"{"cmd":"set_localvqe_noise_gate","noise_gate":true,"noise_gate_threshold_dbfs":-42.5}"#,
        )
        .unwrap();
        assert!(matches!(
            set_localvqe_noise_gate,
            RuntimeControlCommand::SetLocalvqeNoiseGate {
                enabled: true,
                threshold_dbfs
            } if (threshold_dbfs - -42.5).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn runtime_control_capabilities_match_frontend_commands() {
        assert!(SUPPORTED_RUNTIME_CONTROLS.contains(&"start_diagnostics"));
        assert!(SUPPORTED_RUNTIME_CONTROLS.contains(&"stop_diagnostics"));
        assert!(SUPPORTED_RUNTIME_CONTROLS.contains(&"set_output_level"));
        assert!(SUPPORTED_RUNTIME_CONTROLS.contains(&"set_bypass"));
        assert!(SUPPORTED_RUNTIME_CONTROLS.contains(&"set_near_delay_ms"));
        assert!(SUPPORTED_RUNTIME_CONTROLS.contains(&"set_initial_delay_ms"));
        assert!(SUPPORTED_RUNTIME_CONTROLS.contains(&"set_aec3_ns"));
        assert!(SUPPORTED_RUNTIME_CONTROLS.contains(&"set_aec3_agc"));
        assert!(SUPPORTED_RUNTIME_CONTROLS.contains(&"set_localvqe_noise_gate"));
    }

    #[test]
    fn near_delay_buffer_retune_expands_trims_and_clears() {
        let mut delay = VecDeque::from(vec![1.0, 2.0]);

        retune_near_delay_buffer(&mut delay, 4);
        assert_eq!(
            delay.iter().copied().collect::<Vec<_>>(),
            vec![0.0, 0.0, 1.0, 2.0]
        );

        retune_near_delay_buffer(&mut delay, 2);
        assert_eq!(delay.iter().copied().collect::<Vec<_>>(), vec![1.0, 2.0]);

        retune_near_delay_buffer(&mut delay, 0);
        assert!(delay.is_empty());
    }
}
