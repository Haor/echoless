use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::bin_resolve::{echoless_command, suppress_child_console};

pub(crate) const STREAM_TAIL_LIMIT_BYTES: usize = 4096;
pub(crate) const JSON_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const VALIDATE_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const PROBE_DELAY_TIMEOUT: Duration = Duration::from_secs(45);
pub(crate) const NVAFX_INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
// download-install 含 ~1 GB 下载:10 分钟对慢速链路(< ~14 Mbps)会中途被杀,给 30 分钟。
pub(crate) const NVAFX_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub(crate) const MODEL_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// 运行中的 echoless run 子进程 + 它专属的「正在被主动停止」标记。
/// 每个子进程独立持有 stopping flag,其 stdout reader 退出时据此判断本次退出是
/// 主动停/重启(intentional)还是子进程自己崩了(crash),供前端区分。
pub(crate) struct RunChild {
    pub(crate) child: Child,
    pub(crate) stopping: Arc<AtomicBool>,
    pub(crate) config_path: PathBuf,
}

/// RAII 兜底(审计 B-01):RunChild 无论从哪条路径被丢弃(terminate_run、
/// mark_run_exited、start_run 幂等回收、ExitRequested),都走同一套
/// 优雅停机 + 临时配置清理,杜绝孤儿 CLI。
impl Drop for RunChild {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        shutdown_child_gracefully(&mut self.child);
        crate::platform::cleanup_run_config(&self.config_path);
    }
}

/// 优雅停机协议(审计 B-01):先关 stdin(CLI 侧「stdin EOF = 停机」契约,
/// 让 CLI 正常走 Drop 链回收 macOS Process Tap helper),限时等待退出,
/// 超时才兜底 kill。此前直接 kill(SIGKILL)会跳过 CLI 清理,macOS 上
/// 每次停止都遗留 helper 持续占用系统音频 tap(录音指示器长亮)。
pub(crate) fn shutdown_child_gracefully(child: &mut Child) {
    drop(child.stdin.take());
    for _ in 0..40 {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// 当前运行中的 echoless run 子进程(同一时刻最多一个)。
pub(crate) struct RunState(pub(crate) Mutex<Option<RunChild>>);

pub(crate) fn run_state_guard(state: &RunState) -> MutexGuard<'_, Option<RunChild>> {
    // Keep the GUI backend recoverable after an unrelated panic while holding the run lock.
    state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn terminate_run(state: &RunState) {
    // 锁外 drop:RunChild::Drop 的限时等待(最长 1s)不占 run_state 锁。
    let child_opt = {
        let mut guard = run_state_guard(state);
        guard.take()
    };
    drop(child_opt);
}

pub(crate) fn mark_run_exited(state: &RunState, config_path: &Path) {
    let child_opt = {
        let mut guard = run_state_guard(state);
        if guard
            .as_ref()
            .is_some_and(|rc| rc.config_path == config_path)
        {
            guard.take()
        } else {
            None
        }
    };
    if child_opt.is_none() {
        crate::platform::cleanup_run_config(config_path);
    }
    // Some(_) 由 RunChild::Drop 收尾(子进程已自行退出,try_wait 立即命中)。
}

pub(crate) fn command_output_with_timeout(
    command: &mut Command,
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    suppress_child_console(command);
    let mut child = command
        .spawn()
        .map_err(|e| format!("spawn {label} failed: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|e| format!("read {label} output failed: {e}"));
            }
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let output = child
                    .wait_with_output()
                    .map_err(|e| format!("wait timed out {label} failed: {e}"))?;
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "{label} timed out after {}s; stderr: {}",
                    timeout.as_secs(),
                    stderr.trim()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("wait {label} failed: {e}")),
        }
    }
}

pub(crate) fn command_status_error(label: &str, out: &Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let detail = if !stderr.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    };
    // 错误会直达前端状态条/卡片,截断防止长输出撑爆 UI。
    let detail: String = if detail.chars().count() > 240 {
        let head: String = detail.chars().take(240).collect();
        format!("{head}…")
    } else {
        detail.to_string()
    };
    format!(
        "{label} failed with status {}; output: {detail}",
        out.status
    )
}

pub(crate) fn parse_json_output(label: &str, out: Output) -> Result<Value, String> {
    if !out.status.success() {
        return Err(command_status_error(label, &out));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).map_err(|e| format!("parse json failed: {e}; raw: {stdout}"))
}

#[derive(Debug, PartialEq)]
pub(crate) enum JsonlLineEvent {
    Empty,
    Json(Value),
    Unparsed(String),
}

pub(crate) fn parse_jsonl_line_event(line: &str) -> JsonlLineEvent {
    if line.trim().is_empty() {
        return JsonlLineEvent::Empty;
    }
    match serde_json::from_str::<Value>(line) {
        Ok(value) => JsonlLineEvent::Json(value),
        Err(_) => JsonlLineEvent::Unparsed(line.to_string()),
    }
}

pub(crate) fn push_tail_line(tail: &mut String, line: &str, limit_bytes: usize) {
    tail.push_str(line);
    tail.push('\n');
    if tail.len() <= limit_bytes {
        return;
    }
    let cut_at_least = tail.len() - limit_bytes;
    let cut = tail
        .char_indices()
        .map(|(index, _)| index)
        .find(|index| *index >= cut_at_least)
        .unwrap_or(tail.len());
    tail.drain(..cut);
}

/// 跑一次性 JSON 子命令(devices / processors / config validate),返回解析后的 JSON。
pub(crate) fn run_json_blocking(
    app: Option<&tauri::AppHandle>,
    args: &[&str],
    timeout: Duration,
    label: &str,
) -> Result<Value, String> {
    let mut command = echoless_command(app)?;
    command.args(args);
    let out = command_output_with_timeout(&mut command, timeout, label)?;
    parse_json_output(label, out)
}

pub(crate) async fn run_json_async(
    app: tauri::AppHandle,
    args: Vec<String>,
    timeout: Duration,
    label: &'static str,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run_json_blocking(Some(&app), &arg_refs, timeout, label)
    })
    .await
    .map_err(|e| format!("{label} task join failed: {e}"))?
}
