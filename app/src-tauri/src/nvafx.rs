use std::io::{BufRead, BufReader, Read};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;
use tauri::Emitter;

use crate::bin_resolve::{echoless_command, suppress_child_console};
use crate::proc::{
    command_output_with_timeout, command_status_error, parse_jsonl_line_event, push_tail_line,
    run_json_async, run_json_blocking, JsonlLineEvent, JSON_COMMAND_TIMEOUT,
    NVAFX_DOWNLOAD_TIMEOUT, NVAFX_INSTALL_TIMEOUT, STREAM_TAIL_LIMIT_BYTES,
};

/// NVIDIA AFX / RTX AEC 引擎就绪探针。
/// 返回 { ok, report: { runtime_dir, runtime_dir_source, gpus[], selected_arch, checks[] } }。
/// macOS/Linux 上后端会返回 ok=false + platform unsupported 检查项(诚实降级)。
#[tauri::command]
pub(crate) async fn nvafx_doctor(app: tauri::AppHandle) -> Result<Value, String> {
    let args: Vec<String> = vec!["nvafx".into(), "doctor".into(), "--json".into()];
    run_json_async(app, args, JSON_COMMAND_TIMEOUT, "nvafx doctor").await
}

/// NVAFX runtime 安装:校验+解压 common zip 与按架构选的 model zip,然后回传安装后的 doctor 报告。
/// 实际只在 Windows 生效(CLI `nvafx install` 在非 Windows 会 bail);mac/Linux 上返回 Err。
#[tauri::command]
pub(crate) async fn nvafx_install(
    app: tauri::AppHandle,
    common_zip: String,
    model_zip: String,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let args: Vec<String> = vec![
            "nvafx".into(),
            "install".into(),
            "--common-zip".into(),
            common_zip,
            "--model-zip".into(),
            model_zip,
        ];
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut command = echoless_command(Some(&app))?;
        command.args(&arg_refs);
        let out =
            command_output_with_timeout(&mut command, NVAFX_INSTALL_TIMEOUT, "nvafx install")?;
        if !out.status.success() {
            return Err(command_status_error("nvafx install", &out));
        }

        // 安装后用 doctor --json 验证,回传报告供前端重算状态。
        let dargs: Vec<String> = vec!["nvafx".into(), "doctor".into(), "--json".into()];
        let darg_refs: Vec<&str> = dargs.iter().map(String::as_str).collect();
        run_json_blocking(Some(&app), &darg_refs, JSON_COMMAND_TIMEOUT, "nvafx doctor")
    })
    .await
    .map_err(|e| format!("nvafx install task join failed: {e}"))?
}

/// 从公共 GitHub release 下载 common+架构 model zip,然后安装并回传 doctor。
/// shell `echoless nvafx download-install --json`;该子命令需打印
/// `{ok, report}` doctor JSON 到 stdout。后端(Codex)实现该子命令后此处即生效;
/// 未实现前 CLI 会非 0 退出,错误经 stderr 透传给前端。
#[tauri::command]
pub(crate) async fn nvafx_download_install(app: tauri::AppHandle) -> Result<Value, String> {
    // 流式版:一次性命令,但要边跑边转发下载进度。stderr 上 CLI 会打
    // nvafx_download_progress JSONL(→ echoless://nvafx-progress 事件)与人读日志;
    // stdout 累积到进程结束才是最终的 { ok, report } JSON。
    tauri::async_runtime::spawn_blocking(move || {
        let args: Vec<String> = vec!["nvafx".into(), "download-install".into(), "--json".into()];
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut command = echoless_command(Some(&app))?;
        command.args(&arg_refs);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        suppress_child_console(&mut command);
        let mut child = command
            .spawn()
            .map_err(|e| format!("spawn nvafx download-install failed: {e}"))?;

        // stderr reader:进度 JSONL → 事件;其余行 → 日志 + 记入 tail(报错用)。
        let herr = child.stderr.take().map(|stderr| {
            let app = app.clone();
            std::thread::spawn(move || {
                let mut tail = String::new();
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    match parse_jsonl_line_event(&line) {
                        JsonlLineEvent::Json(v)
                            if v.get("event").and_then(|e| e.as_str())
                                == Some("nvafx_download_progress") =>
                        {
                            let _ = app.emit("echoless://nvafx-progress", v);
                        }
                        _ => {
                            push_tail_line(&mut tail, &line, STREAM_TAIL_LIMIT_BYTES);
                            let _ = app.emit("echoless://log", line);
                        }
                    }
                }
                tail
            })
        });

        // stdout reader:累积最终 JSON。
        let hout = child.stdout.take().map(|stdout| {
            std::thread::spawn(move || {
                let mut s = String::new();
                let _ = BufReader::new(stdout).read_to_string(&mut s);
                s
            })
        });

        // 带超时等待(下载 ~1 GB,用较宽的 NVAFX_DOWNLOAD_TIMEOUT)。超时返回 None。
        let started = Instant::now();
        let exit_status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if started.elapsed() >= NVAFX_DOWNLOAD_TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(format!("wait nvafx download-install failed: {e}")),
            }
        };

        // 无论成功/超时,kill 后管道 EOF、reader 线程自然收尾;join 拿到 stdout 与
        // stderr 尾巴(诊断用),避免线程游离。
        let stdout_str = hout.and_then(|h| h.join().ok()).unwrap_or_default();
        let stderr_tail = herr.and_then(|h| h.join().ok()).unwrap_or_default();
        let Some(status) = exit_status else {
            return Err(format!(
                "nvafx download-install timed out after {}s; {}",
                NVAFX_DOWNLOAD_TIMEOUT.as_secs(),
                stderr_tail.trim()
            ));
        };
        if !status.success() {
            return Err(format!(
                "nvafx download-install failed with status {status}; {}",
                stderr_tail.trim()
            ));
        }
        serde_json::from_str(&stdout_str).map_err(|e| {
            format!("parse nvafx download-install json failed: {e}; raw: {stdout_str}")
        })
    })
    .await
    .map_err(|e| format!("nvafx download-install task join failed: {e}"))?
}
