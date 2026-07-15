use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{Emitter, Manager, State};

use crate::bin_resolve::{echoless_command, suppress_child_console};
use crate::platform::{cleanup_run_config, transient_config_dir, write_transient_config_toml};
use crate::proc::{
    cancel_run_start, commit_run_finalization, commit_run_start, is_active_run,
    parse_jsonl_line_event, push_tail_line, reserve_run_start, run_control_sender, run_json_async,
    shutdown_child_gracefully, terminate_run, JsonlLineEvent, RunChild, RunControlWriter,
    RunFinalization, RunId, RunState, PROBE_DELAY_TIMEOUT, RUN_CONTROL_ACK_TIMEOUT,
    STREAM_TAIL_LIMIT_BYTES, VALIDATE_COMMAND_TIMEOUT,
};
use crate::tray::update_tray_tooltip;

/// 主动近端延迟侦测 / AEC 链路诊断。shell `echoless probe-delay --json`:播放一串蜂鸣、
/// probe-delay 专用 runner:stderr 的 JSONL 进度行实时转发为
/// `echoless://probe-progress` 事件(前端用 beep_train_start 把进度灯对齐真实播放时刻),
/// stdout 仍在进程结束后整体解析为最终 JSON 结果。
fn run_probe_streaming(
    app: &tauri::AppHandle,
    args: &[&str],
    timeout: Duration,
) -> Result<Value, String> {
    let label = "probe-delay";
    let mut command = echoless_command(Some(app))?;
    command.args(args);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    suppress_child_console(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| format!("spawn {label} failed: {e}"))?;
    let stderr = child.stderr.take().ok_or("probe stderr not captured")?;
    let app_ev = app.clone();
    // stderr 尾巴留存:CLI 失败时错误原因在 stderr(stdout 无 JSON)。
    let stderr_tail = Arc::new(Mutex::new(String::new()));
    let tail_writer = stderr_tail.clone();
    let reader = std::thread::spawn(move || {
        // probe-delay 进度契约:stderr 上每条进度都是完整 JSONL;坏行降级为日志保留证据。
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            match parse_jsonl_line_event(&line) {
                JsonlLineEvent::Empty => {}
                JsonlLineEvent::Json(v) => {
                    let _ = app_ev.emit("echoless://probe-progress", v);
                }
                JsonlLineEvent::Unparsed(line) => {
                    let _ = app_ev.emit("echoless://log", format!("unparsed probe line: {line}"));
                }
            }
            let mut tail = tail_writer.lock().unwrap();
            push_tail_line(&mut tail, &line, STREAM_TAIL_LIMIT_BYTES);
        }
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                let tail = stderr_tail.lock().unwrap().trim().to_string();
                return Err(format!(
                    "{label} timed out after {}s; stderr: {tail}",
                    timeout.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("wait {label} failed: {e}")),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("read {label} output failed: {e}"))?;
    let _ = reader.join();
    if !out.status.success() {
        let tail = stderr_tail.lock().unwrap().trim().to_string();
        // 与 command_status_error 相同的 240 字符截断(错误直达前端 UI)。
        let detail: String = if tail.chars().count() > 240 {
            format!("{}…", tail.chars().take(240).collect::<String>())
        } else {
            tail
        };
        return Err(format!(
            "{label} failed with status {}; output: {detail}",
            out.status
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).map_err(|e| format!("parse json failed: {e}; raw: {stdout}"))
}

/// 同时录 ref/mic、分析两路相对到达时差,返回 NearDelayProbeResult(含 recommended_near_delay_ms)。
/// 通常约 15 秒(首次 macOS 权限/Process Tap 启动可能更久)、会外放蜂鸣 —— 故必须先停掉
/// 主 run(probe 内部自起子进程占用设备),由前端 gating。
/// 支持 macOS / Windows / Linux(见 probe_delay.rs);不支持的平台 CLI 会非 0 退出,错误经 stderr 透传给前端。
#[tauri::command]
pub(crate) async fn probe_delay(
    app: tauri::AppHandle,
    mic: String,
    reference: String,
    output: String,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut args: Vec<String> = vec!["probe-delay".into(), "--json".into()];
        // selector 透传(含 "default",与 run 同一套解析);仅空串时省略走 CLI 内置默认。
        let opt = |flag: &str, v: &str, args: &mut Vec<String>| {
            if !v.is_empty() {
                args.push(flag.into());
                args.push(v.into());
            }
        };
        opt("--mic", &mic, &mut args);
        opt("--reference", &reference, &mut args);
        opt("--output", &output, &mut args);
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run_probe_streaming(&app, &arg_refs, PROBE_DELAY_TIMEOUT)
    })
    .await
    .map_err(|e| format!("probe task join failed: {e}"))?
}

#[tauri::command]
pub(crate) async fn validate_config(
    app: tauri::AppHandle,
    toml_text: String,
) -> Result<Value, String> {
    let dir = transient_config_dir(&app)?;
    let path = write_transient_config_toml(&dir, "validate", &toml_text)?;
    let config_arg = path.to_string_lossy().to_string();
    let result = run_json_async(
        app,
        vec![
            "config".into(),
            "validate".into(),
            "--config".into(),
            config_arg,
            "--json".into(),
        ],
        VALIDATE_COMMAND_TIMEOUT,
        "config validate",
    )
    .await;
    cleanup_run_config(&path);
    result
}

/// 仅由 `schedule_run_finalization` 投递到 Tauri 主线程。状态提交后再在锁外
/// 回收 child/配置并更新 tray/event，保证下一次主线程生命周期动作不会插队。
fn finalize_run_on_main_thread(
    app: &tauri::AppHandle,
    run_id: RunId,
    config_path: &Path,
    intentional: bool,
    recoverable: bool,
) {
    let outcome = commit_run_finalization(&app.state::<RunState>(), run_id);
    let child = match outcome {
        RunFinalization::Stale => {
            cleanup_run_config(config_path);
            return;
        }
        RunFinalization::ActiveWithoutChild => None,
        RunFinalization::ActiveWithChild(child) => Some(child),
        RunFinalization::ActiveChildMismatch { child } => {
            crate::logging::log(
                "error",
                "sidecar",
                &format!(
                    "run state mismatch while finalizing {run_id}: child belongs to {}; reaping it",
                    child.run_id
                ),
            );
            Some(child)
        }
    };

    drop(child);
    cleanup_run_config(config_path);
    update_tray_tooltip(app, false);
    let _ = app.emit(
        "echoless://exit",
        serde_json::json!({
            "run_id": run_id,
            "intentional": intentional,
            "recoverable": recoverable,
        }),
    );
}

/// stdout reader 的唯一 finalization 入口：fire-and-forget 投递整个代际提交和
/// GUI/event 收尾。调度失败时事件循环已不可用，仍诊断并幂等清理 reader 配置。
fn schedule_run_finalization(
    app: &tauri::AppHandle,
    run_id: RunId,
    config_path: PathBuf,
    intentional: bool,
    recoverable: bool,
) {
    let finalizer_app = app.clone();
    let fallback_config_path = config_path.clone();
    if let Err(err) = app.run_on_main_thread(move || {
        finalize_run_on_main_thread(
            &finalizer_app,
            run_id,
            &config_path,
            intentional,
            recoverable,
        );
    }) {
        crate::logging::log(
            "error",
            "sidecar",
            &format!("failed to schedule run {run_id} finalization on main thread: {err}"),
        );
        cleanup_run_config(&fallback_config_path);
    }
}

#[tauri::command]
pub(crate) fn start_run(
    app: tauri::AppHandle,
    state: State<RunState>,
    toml_text: String,
    stats_interval_ms: Option<u32>,
) -> Result<RunId, String> {
    // 锁内只摘状态并预留代际；旧 child 的优雅停机和所有启动准备都在锁外。
    let (run_id, old_child) = reserve_run_start(&state)?;
    drop(old_child);

    let dir = match transient_config_dir(&app) {
        Ok(dir) => dir,
        Err(err) => {
            cancel_run_start(&state, run_id);
            return Err(err);
        }
    };
    let path = match write_transient_config_toml(&dir, "run", &toml_text) {
        Ok(path) => path,
        Err(err) => {
            cancel_run_start(&state, run_id);
            return Err(err);
        }
    };
    let config_arg = path.to_string_lossy().to_string();
    let interval = stats_interval_ms.unwrap_or(80).to_string();

    let mut command = match echoless_command(Some(&app)) {
        Ok(command) => command,
        Err(err) => {
            cancel_run_start(&state, run_id);
            cleanup_run_config(&path);
            return Err(err);
        }
    };
    suppress_child_console(&mut command);
    let child_result = command
        .args([
            "run",
            "--config",
            &config_arg,
            "--status-json",
            "--stats-interval-ms",
            &interval,
        ])
        .stdin(Stdio::piped()) // 录制就地控制:start/stop_diagnostics 经 stdin JSONL 下发
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child_result {
        Ok(child) => child,
        Err(err) => {
            cancel_run_start(&state, run_id);
            cleanup_run_config(&path);
            crate::logging::log(
                "error",
                "sidecar",
                &format!("spawn echoless run failed: {err}"),
            );
            return Err(format!("spawn echoless run failed: {err}"));
        }
    };

    // 本子进程专属的 stopping flag:被主动停/重启时置 true。
    let stopping = Arc::new(AtomicBool::new(false));

    let stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            cancel_run_start(&state, run_id);
            shutdown_child_gracefully(&mut child);
            cleanup_run_config(&path);
            return Err("no stdin".to_string());
        }
    };
    let control_writer = match RunControlWriter::spawn(stdin) {
        Ok(writer) => writer,
        Err(err) => {
            cancel_run_start(&state, run_id);
            shutdown_child_gracefully(&mut child);
            cleanup_run_config(&path);
            return Err(err);
        }
    };

    // stdout = JSONL status events
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            cancel_run_start(&state, run_id);
            drop(control_writer);
            shutdown_child_gracefully(&mut child);
            cleanup_run_config(&path);
            return Err("no stdout".to_string());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            cancel_run_start(&state, run_id);
            drop(control_writer);
            shutdown_child_gracefully(&mut child);
            cleanup_run_config(&path);
            return Err("no stderr".to_string());
        }
    };

    let reader_config_path = path.clone();
    if !commit_run_start(
        &state,
        run_id,
        RunChild {
            run_id,
            child,
            stopping: stopping.clone(),
            config_path: path,
            control_writer: Some(control_writer),
        },
    ) {
        crate::logging::log(
            "info",
            "sidecar",
            &format!("discarded superseded run start {run_id}"),
        );
        return Err(format!("run start {run_id} was superseded or canceled"));
    }

    // readers 必须在 child/active generation 原子安装之后启动，避免首批
    // started/status 被 generation 判定误丢。
    let app_out = app.clone();
    let stop_reader = stopping.clone();
    std::thread::spawn(move || {
        let mut recoverable_stream_failure = false;
        // CLI stdout 契约:只输出完整 JSONL status/control 行;坏行降级为日志保留证据。
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => match parse_jsonl_line_event(&line) {
                    JsonlLineEvent::Empty => {}
                    JsonlLineEvent::Json(value) => {
                        recoverable_stream_failure |= is_recoverable_stream_failure(&value);
                        match attach_run_id(value, run_id) {
                            Ok(value) => {
                                let run_state = app_out.state::<RunState>();
                                if is_active_run(&run_state, run_id) {
                                    let _ = app_out.emit("echoless://status", value);
                                }
                            }
                            Err(value) => {
                                let run_state = app_out.state::<RunState>();
                                if is_active_run(&run_state, run_id) {
                                    crate::logging::log(
                                        "warn",
                                        "sidecar",
                                        &format!("non-object status JSON ignored: {value}"),
                                    );
                                    let _ = app_out.emit(
                                        "echoless://log",
                                        format!("non-object status JSON ignored: {value}"),
                                    );
                                }
                            }
                        }
                    }
                    JsonlLineEvent::Unparsed(line) => {
                        let run_state = app_out.state::<RunState>();
                        if is_active_run(&run_state, run_id) {
                            crate::logging::log(
                                "warn",
                                "sidecar",
                                &format!("unparsed status line: {line}"),
                            );
                            let _ = app_out
                                .emit("echoless://log", format!("unparsed status line: {line}"));
                        }
                    }
                },
                Err(err) => {
                    let run_state = app_out.state::<RunState>();
                    if is_active_run(&run_state, run_id) {
                        crate::logging::log(
                            "error",
                            "sidecar",
                            &format!("failed to read echoless stdout: {err}"),
                        );
                        let _ = app_out.emit(
                            "echoless://log",
                            format!("failed to read echoless stdout: {err}"),
                        );
                    }
                    break;
                }
            }
        }
        // 退出归因:intentional=主动停/重启(本 flag 已被置 true);否则=子进程自己退出(崩溃)。
        let intentional = stop_reader.load(Ordering::SeqCst);
        crate::logging::log(
            if intentional { "info" } else { "error" },
            "sidecar",
            &format!("echoless run {run_id} exited (intentional={intentional})"),
        );
        schedule_run_finalization(
            &app_out,
            run_id,
            reader_config_path,
            intentional,
            recoverable_stream_failure,
        );
    });

    // stderr = 人类日志(转发事件 + 落盘取证)
    let app_err = app.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(line) => {
                    if !line.trim().is_empty() {
                        // Persist first, then gate only the UI event by generation. The stdout
                        // finalizer can clear the active run before this reader receives the
                        // trailing error line; dropping it here would erase the crash evidence.
                        let level = if line.contains(" stream error:") {
                            "error"
                        } else {
                            "info"
                        };
                        crate::logging::log(level, "cli", &format!("run {run_id}: {line}"));
                        let run_state = app_err.state::<RunState>();
                        if is_active_run(&run_state, run_id) {
                            let _ = app_err.emit("echoless://log", line);
                        }
                    }
                }
                Err(err) => {
                    crate::logging::log(
                        "error",
                        "sidecar",
                        &format!("failed to read echoless run {run_id} stderr: {err}"),
                    );
                    let run_state = app_err.state::<RunState>();
                    if is_active_run(&run_state, run_id) {
                        let _ = app_err.emit(
                            "echoless://log",
                            format!("failed to read echoless stderr: {err}"),
                        );
                    }
                    break;
                }
            }
        }
    });

    crate::logging::log("info", "sidecar", "echoless run started");
    if is_active_run(&state, run_id) {
        update_tray_tooltip(&app, true);
    }
    Ok(run_id)
}

pub(crate) fn attach_run_id(mut value: Value, run_id: RunId) -> Result<Value, Value> {
    let Some(object) = value.as_object_mut() else {
        return Err(value);
    };
    object.insert("run_id".to_string(), json!(run_id));
    Ok(value)
}

pub(crate) fn is_recoverable_stream_failure(value: &Value) -> bool {
    value.as_object().is_some_and(|event| {
        event.get("type").and_then(Value::as_str) == Some("stream_error")
            && event.get("fatal").and_then(Value::as_bool) == Some(true)
            && event.get("recoverable").and_then(Value::as_bool) == Some(true)
    })
}

/// 向运行中的 echoless run 子进程 stdin 写一行 JSON 控制命令。
/// 具体能力由 CLI started.supported_controls 上报。
#[tauri::command]
pub(crate) async fn send_run_control(
    state: State<'_, RunState>,
    line: String,
) -> Result<(), String> {
    enqueue_run_control(&state, line).await
}

#[cfg(test)]
pub(crate) fn write_run_control_line(state: &RunState, line: &str) -> Result<(), String> {
    run_control_sender(state)?.send_line_with_timeout(line, RUN_CONTROL_ACK_TIMEOUT)
}

async fn enqueue_run_control(state: &RunState, line: String) -> Result<(), String> {
    let sender = run_control_sender(state)?;
    tauri::async_runtime::spawn_blocking(move || {
        sender.send_line_with_timeout(&line, RUN_CONTROL_ACK_TIMEOUT)
    })
    .await
    .map_err(|err| format!("run control task join failed: {err}"))?
}

#[tauri::command]
pub(crate) async fn set_bypass(state: State<'_, RunState>, enabled: bool) -> Result<(), String> {
    let line = bypass_control_line(enabled);
    enqueue_run_control(&state, line).await
}

pub(crate) fn bypass_control_line(enabled: bool) -> String {
    json!({
        "cmd": "set_bypass",
        "enabled": enabled,
    })
    .to_string()
}

#[tauri::command]
pub(crate) fn stop_run(
    app: tauri::AppHandle,
    state: State<RunState>,
) -> Result<Option<RunId>, String> {
    let run_id = terminate_run(&state);
    if let Some(run_id) = run_id {
        update_tray_tooltip(&app, false);
        let _ = app.emit(
            "echoless://exit",
            serde_json::json!({ "run_id": run_id, "intentional": true }),
        );
    }
    Ok(run_id)
}
