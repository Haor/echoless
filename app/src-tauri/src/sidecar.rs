use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{Emitter, Manager, State};

use crate::bin_resolve::{echoless_command, suppress_child_console};
use crate::platform::{cleanup_run_config, transient_config_dir, write_transient_config_toml};
use crate::proc::{
    mark_run_exited, parse_jsonl_line_event, push_tail_line, run_json_async, run_state_guard,
    shutdown_child_gracefully, terminate_run, JsonlLineEvent, RunChild, RunState,
    PROBE_DELAY_TIMEOUT, STREAM_TAIL_LIMIT_BYTES, VALIDATE_COMMAND_TIMEOUT,
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
/// 约 15 秒、会外放蜂鸣 —— 故必须先停掉主 run(probe 内部自起子进程占用设备),由前端 gating。
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

#[tauri::command]
pub(crate) fn start_run(
    app: tauri::AppHandle,
    state: State<RunState>,
    toml_text: String,
    stats_interval_ms: Option<u32>,
) -> Result<(), String> {
    let mut guard = run_state_guard(&state);
    // 幂等启动:若有残留子进程(并发重启 / 上次崩溃遗留),经 RunChild::Drop
    // 优雅停机(stopping 标记使其 reader 判定为 intentional,不报崩溃)。
    drop(guard.take());
    let dir = transient_config_dir(&app)?;
    let path = write_transient_config_toml(&dir, "run", &toml_text)?;
    let config_arg = path.to_string_lossy().to_string();
    let interval = stats_interval_ms.unwrap_or(80).to_string();

    let mut command = echoless_command(Some(&app))?;
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
            cleanup_run_config(&path);
            crate::logging::log(
                "error",
                "sidecar",
                &format!("spawn echoless run failed: {err}"),
            );
            return Err(format!("spawn echoless run failed: {err}"));
        }
    };
    crate::logging::log("info", "sidecar", "echoless run started");

    // 本子进程专属的 stopping flag:被主动停/重启时置 true。
    let stopping = Arc::new(AtomicBool::new(false));

    // stdout = JSONL status events
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            shutdown_child_gracefully(&mut child);
            cleanup_run_config(&path);
            return Err("no stdout".to_string());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            shutdown_child_gracefully(&mut child);
            cleanup_run_config(&path);
            return Err("no stderr".to_string());
        }
    };
    let app_out = app.clone();
    let stop_reader = stopping.clone();
    let reader_config_path = path.clone();
    std::thread::spawn(move || {
        // CLI stdout 契约:只输出完整 JSONL status/control 行;坏行降级为日志保留证据。
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => match parse_jsonl_line_event(&line) {
                    JsonlLineEvent::Empty => {}
                    JsonlLineEvent::Json(v) => {
                        let _ = app_out.emit("echoless://status", v);
                    }
                    JsonlLineEvent::Unparsed(line) => {
                        crate::logging::log(
                            "warn",
                            "sidecar",
                            &format!("unparsed status line: {line}"),
                        );
                        let _ =
                            app_out.emit("echoless://log", format!("unparsed status line: {line}"));
                    }
                },
                Err(err) => {
                    crate::logging::log(
                        "error",
                        "sidecar",
                        &format!("failed to read echoless stdout: {err}"),
                    );
                    let _ = app_out.emit(
                        "echoless://log",
                        format!("failed to read echoless stdout: {err}"),
                    );
                    break;
                }
            }
        }
        // 退出归因:intentional=主动停/重启(本 flag 已被置 true);否则=子进程自己退出(崩溃)。
        let intentional = stop_reader.load(Ordering::SeqCst);
        crate::logging::log(
            if intentional { "info" } else { "error" },
            "sidecar",
            &format!("echoless run exited (intentional={intentional})"),
        );
        let run_state = app_out.state::<RunState>();
        mark_run_exited(&run_state, &reader_config_path);
        update_tray_tooltip(&app_out, false);
        let _ = app_out.emit(
            "echoless://exit",
            serde_json::json!({ "intentional": intentional }),
        );
    });

    // stderr = 人类日志(转发事件 + 落盘取证)
    let app_err = app.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(line) => {
                    if !line.trim().is_empty() {
                        crate::logging::log("info", "cli", &line);
                        let _ = app_err.emit("echoless://log", line);
                    }
                }
                Err(err) => {
                    let _ = app_err.emit(
                        "echoless://log",
                        format!("failed to read echoless stderr: {err}"),
                    );
                    break;
                }
            }
        }
    });

    *guard = Some(RunChild {
        child,
        stopping,
        config_path: path,
    });
    update_tray_tooltip(&app, true);
    Ok(())
}

/// 向运行中的 echoless run 子进程 stdin 写一行 JSON 控制命令。
/// 具体能力由 CLI started.supported_controls 上报。
#[tauri::command]
pub(crate) fn send_run_control(state: State<RunState>, line: String) -> Result<(), String> {
    write_run_control_line(&state, &line)
}

pub(crate) fn write_run_control_line(state: &RunState, line: &str) -> Result<(), String> {
    let mut guard = run_state_guard(state);
    let write_result = {
        let rc = guard.as_mut().ok_or("not running")?;
        let stdin = rc.child.stdin.as_mut().ok_or("no stdin")?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .map_err(|e| e.to_string())
    };
    if let Err(err) = write_result {
        let status = guard
            .as_mut()
            .and_then(|rc| rc.child.try_wait().ok().flatten());
        if let Some(status) = status {
            drop(guard.take());
            return Err(format!(
                "run process exited before control command was applied: {status}"
            ));
        }
        return Err(err);
    }

    let status = {
        let rc = guard.as_mut().ok_or("not running")?;
        rc.child
            .try_wait()
            .map_err(|e| format!("failed to check run process after control command: {e}"))?
    };
    if let Some(status) = status {
        drop(guard.take());
        return Err(format!(
            "run process exited before control command was applied: {status}"
        ));
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn set_bypass(state: State<RunState>, enabled: bool) -> Result<(), String> {
    let line = bypass_control_line(enabled);
    write_run_control_line(&state, &line)
}

pub(crate) fn bypass_control_line(enabled: bool) -> String {
    json!({
        "cmd": "set_bypass",
        "enabled": enabled,
    })
    .to_string()
}

#[tauri::command]
pub(crate) fn stop_run(app: tauri::AppHandle, state: State<RunState>) -> Result<(), String> {
    terminate_run(&state);
    update_tray_tooltip(&app, false);
    Ok(())
}
