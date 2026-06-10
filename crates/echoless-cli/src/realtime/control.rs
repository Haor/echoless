use std::io::BufRead;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::thread;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use echoless_core::{output_level_gain_db, DiagnosticsConfig, MAX_OUTPUT_LEVEL};

use super::{DiagnosticDoneReason, DiagnosticRecorder, DiagnosticRecorderConfig, RealtimeStats};
use echoless_processors::ProcessorChain;

#[derive(Debug)]
pub(super) enum RuntimeControlCommand {
    StartDiagnostics {
        record_dir: String,
        max_seconds: Option<u32>,
    },
    StopDiagnostics,
    SetOutputLevel(u32),
}

pub(super) const SUPPORTED_RUNTIME_CONTROLS: &[&str] =
    &["start_diagnostics", "stop_diagnostics", "set_output_level"];

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
            let record_dir = value
                .get("record_dir")
                .and_then(Value::as_str)
                .context("start_diagnostics requires string field `record_dir`")?
                .to_string();
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
            Ok(RuntimeControlCommand::StartDiagnostics {
                record_dir,
                max_seconds,
            })
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
        other => bail!("unknown runtime control command `{other}`"),
    }
}

pub(super) struct RuntimeControlContext<'a> {
    pub(super) diagnostic: &'a mut Option<DiagnosticRecorder>,
    pub(super) stats: Option<&'a mut RealtimeStats>,
    pub(super) chain: &'a ProcessorChain,
    pub(super) sample_rate: u32,
    pub(super) reference_channels: u16,
    pub(super) frame_ms: u32,
    pub(super) near_delay_ms: u32,
    pub(super) output_level: &'a mut u32,
    pub(super) status_json: bool,
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
                        output_level: ctx.output_level,
                        status_json: ctx.status_json,
                    },
                );
            }
            Ok(RuntimeControlEvent::Error(message)) => {
                emit_control_error(ctx.status_json, None, message);
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                *control = None;
                break;
            }
        }
    }
}

struct RuntimeControlCommandContext<'a> {
    diagnostic: &'a mut Option<DiagnosticRecorder>,
    stats: Option<&'a mut RealtimeStats>,
    chain: &'a ProcessorChain,
    sample_rate: u32,
    reference_channels: u16,
    frame_ms: u32,
    near_delay_ms: u32,
    output_level: &'a mut u32,
    status_json: bool,
}

fn handle_runtime_control_command(
    command: RuntimeControlCommand,
    ctx: RuntimeControlCommandContext<'_>,
) {
    match command {
        RuntimeControlCommand::StartDiagnostics {
            record_dir,
            max_seconds,
        } => {
            if record_dir.trim().is_empty() {
                emit_control_error(
                    ctx.status_json,
                    Some("start_diagnostics"),
                    "record_dir must not be empty",
                );
                return;
            }
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
            let cfg = DiagnosticsConfig {
                record_dir: Some(record_dir),
                max_seconds,
            };
            let node_stats = ctx.chain.stats();
            match DiagnosticRecorder::new(DiagnosticRecorderConfig {
                cfg: &cfg,
                sample_rate: ctx.sample_rate,
                reference_channels: ctx.reference_channels,
                frame_ms: ctx.frame_ms,
                near_delay_ms: ctx.near_delay_ms,
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
                    "record_dir did not create a diagnostics recorder",
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
        println!("{value}");
    } else {
        eprintln!("{value}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_control_command_parses_frontend_json() {
        let start = parse_runtime_control_command(
            r#"{"cmd":"start_diagnostics","record_dir":"/tmp/diag","max_seconds":10}"#,
        )
        .unwrap();
        match start {
            RuntimeControlCommand::StartDiagnostics {
                record_dir,
                max_seconds,
            } => {
                assert_eq!(record_dir, "/tmp/diag");
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
    }

    #[test]
    fn runtime_control_capabilities_match_frontend_commands() {
        assert!(SUPPORTED_RUNTIME_CONTROLS.contains(&"start_diagnostics"));
        assert!(SUPPORTED_RUNTIME_CONTROLS.contains(&"stop_diagnostics"));
        assert!(SUPPORTED_RUNTIME_CONTROLS.contains(&"set_output_level"));
    }
}
