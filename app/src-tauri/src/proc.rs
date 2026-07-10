use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
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
pub(crate) const RUN_CONTROL_ACK_TIMEOUT: Duration = Duration::from_millis(250);
const RUN_CONTROL_QUEUE_CAPACITY: usize = 8;
const RUN_CONTROL_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(100);
const CHILD_GRACEFUL_SHUTDOWN_POLL_ATTEMPTS: usize = 40;
const CHILD_FORCED_SHUTDOWN_POLL_ATTEMPTS: usize = 8;
const CHILD_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(25);
pub(crate) type RunId = u64;

struct RunControlRequest {
    line: String,
    ack: mpsc::SyncSender<Result<(), String>>,
    phase: Arc<AtomicU8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum RunControlPhase {
    Queued,
    Writing,
    Cancelled,
}

enum RunControlMessage {
    Write(RunControlRequest),
    Shutdown,
}

#[derive(Clone)]
pub(crate) struct RunControlSender {
    sender: mpsc::SyncSender<RunControlMessage>,
    closing: Arc<AtomicBool>,
}

impl RunControlSender {
    pub(crate) fn send_line_with_timeout(
        &self,
        line: &str,
        timeout: Duration,
    ) -> Result<(), String> {
        if self.closing.load(Ordering::SeqCst) {
            return Err("run control writer is not running".to_string());
        }
        let (ack, confirmation) = mpsc::sync_channel(1);
        let phase = Arc::new(AtomicU8::new(RunControlPhase::Queued as u8));
        let request = RunControlMessage::Write(RunControlRequest {
            line: line.to_string(),
            ack,
            phase: phase.clone(),
        });
        match self.sender.try_send(request) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                return Err("run control queue is full".to_string());
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err("run control writer is not running".to_string());
            }
        }

        match confirmation.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let cancelled = match phase.compare_exchange(
                    RunControlPhase::Queued as u8,
                    RunControlPhase::Cancelled as u8,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => true,
                    Err(value) => value == RunControlPhase::Cancelled as u8,
                };
                if cancelled {
                    Err(format!(
                        "run control writer timed out after {}ms; command was cancelled before writing",
                        timeout.as_millis()
                    ))
                } else {
                    Err(format!(
                        "run control writer timed out after {}ms while writing; outcome is unknown",
                        timeout.as_millis()
                    ))
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("run control writer exited before acknowledging command".to_string())
            }
        }
    }
}

struct WriterDone(Option<mpsc::Sender<()>>);

impl Drop for WriterDone {
    fn drop(&mut self) {
        if let Some(done) = self.0.take() {
            let _ = done.send(());
        }
    }
}

type WriterIoCanceller = Arc<dyn Fn(&JoinHandle<()>) + Send + Sync>;

pub(crate) struct RunControlWriter {
    sender: Option<mpsc::SyncSender<RunControlMessage>>,
    closing: Arc<AtomicBool>,
    done: mpsc::Receiver<()>,
    join: Option<JoinHandle<()>>,
    cancel_io: WriterIoCanceller,
}

#[cfg(windows)]
fn cancel_synchronous_writer_io(thread: &JoinHandle<()>) {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    extern "system" {
        #[link_name = "CancelSynchronousIo"]
        fn cancel_synchronous_io(thread: *mut c_void) -> i32;
    }

    // SAFETY: `thread` owns a valid Windows thread handle for this call, and the
    // API neither retains nor closes it. FALSE/ERROR_NOT_FOUND only means that no
    // matching synchronous I/O was pending, which is safe to ignore here.
    let _ = unsafe { cancel_synchronous_io(thread.as_raw_handle()) };
}

#[cfg(not(windows))]
fn cancel_synchronous_writer_io(_thread: &JoinHandle<()>) {}

impl RunControlWriter {
    pub(crate) fn spawn<W>(writer: W) -> Result<Self, String>
    where
        W: Write + Send + 'static,
    {
        Self::spawn_with_capacity(writer, RUN_CONTROL_QUEUE_CAPACITY)
    }

    pub(crate) fn spawn_with_capacity<W>(writer: W, capacity: usize) -> Result<Self, String>
    where
        W: Write + Send + 'static,
    {
        Self::spawn_with_capacity_and_cancel(
            writer,
            capacity,
            Arc::new(cancel_synchronous_writer_io),
        )
    }

    #[cfg(test)]
    pub(crate) fn spawn_with_capacity_and_canceller<W, F>(
        writer: W,
        capacity: usize,
        cancel_io: F,
    ) -> Result<Self, String>
    where
        W: Write + Send + 'static,
        F: Fn(&JoinHandle<()>) + Send + Sync + 'static,
    {
        Self::spawn_with_capacity_and_cancel(writer, capacity, Arc::new(cancel_io))
    }

    fn spawn_with_capacity_and_cancel<W>(
        writer: W,
        capacity: usize,
        cancel_io: WriterIoCanceller,
    ) -> Result<Self, String>
    where
        W: Write + Send + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let (done_sender, done) = mpsc::channel();
        let closing = Arc::new(AtomicBool::new(false));
        let worker_closing = closing.clone();
        let join = std::thread::Builder::new()
            .name("echoless-control-writer".to_string())
            .spawn(move || {
                let _done = WriterDone(Some(done_sender));
                let mut writer = writer;
                while let Ok(message) = receiver.recv() {
                    if worker_closing.load(Ordering::SeqCst) {
                        break;
                    }
                    match message {
                        RunControlMessage::Shutdown => break,
                        RunControlMessage::Write(request) => {
                            if request
                                .phase
                                .compare_exchange(
                                    RunControlPhase::Queued as u8,
                                    RunControlPhase::Writing as u8,
                                    Ordering::SeqCst,
                                    Ordering::SeqCst,
                                )
                                .is_err()
                            {
                                continue;
                            }
                            let result = write_run_control(&mut writer, &request.line);
                            let failed = result.is_err();
                            let _ = request.ack.send(result);
                            if failed || worker_closing.load(Ordering::SeqCst) {
                                break;
                            }
                        }
                    }
                }
            })
            .map_err(|err| format!("spawn run control writer failed: {err}"))?;
        Ok(Self {
            sender: Some(sender),
            closing,
            done,
            join: Some(join),
            cancel_io,
        })
    }

    pub(crate) fn sender(&self) -> Option<RunControlSender> {
        self.sender.as_ref().map(|sender| RunControlSender {
            sender: sender.clone(),
            closing: self.closing.clone(),
        })
    }

    fn begin_shutdown(&mut self) {
        self.closing.store(true, Ordering::SeqCst);
        if let Some(sender) = self.sender.take() {
            let _ = sender.try_send(RunControlMessage::Shutdown);
        }
    }

    fn wait_for_done(&mut self, timeout: Duration) -> bool {
        if self.join.is_none() {
            return true;
        }
        let completed = match self.done.recv_timeout(timeout) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => true,
            Err(mpsc::RecvTimeoutError::Timeout) => false,
        };
        if completed {
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
        completed
    }

    pub(crate) fn shutdown_with_timeout(&mut self, timeout: Duration) -> bool {
        self.begin_shutdown();
        self.wait_for_done(timeout)
    }

    fn finish_after_child_shutdown(&mut self, timeout: Duration) {
        if let Some(join) = self.join.as_ref() {
            (self.cancel_io)(join);
        }
        if !self.wait_for_done(timeout) {
            drop(self.join.take());
        }
    }
}

impl Drop for RunControlWriter {
    fn drop(&mut self) {
        self.begin_shutdown();
        if !self.wait_for_done(RUN_CONTROL_SHUTDOWN_TIMEOUT) {
            drop(self.join.take());
        }
    }
}

fn write_run_control(writer: &mut impl Write, line: &str) -> Result<(), String> {
    writer
        .write_all(line.as_bytes())
        .and_then(|_| writer.write_all(b"\n"))
        .map_err(|err| format!("write run control failed: {err}"))?;
    writer
        .flush()
        .map_err(|err| format!("flush run control failed: {err}"))
}

/// 运行中的 echoless run 子进程 + 它专属的「正在被主动停止」标记。
/// 每个子进程独立持有 stopping flag,其 stdout reader 退出时据此判断本次退出是
/// 主动停/重启(intentional)还是子进程自己崩了(crash),供前端区分。
pub(crate) struct RunChild {
    pub(crate) run_id: RunId,
    pub(crate) child: Child,
    pub(crate) stopping: Arc<AtomicBool>,
    pub(crate) config_path: PathBuf,
    pub(crate) control_writer: Option<RunControlWriter>,
}

/// RAII 兜底(审计 B-01/B-34):RunChild 无论从哪条路径被丢弃，都先请求
/// writer 在短超时内 drop stdin；若 writer 阻塞则继续有限 child shutdown/kill，
/// kill 后仅在 done signal 已到达时 join，否则安全 detach。
impl Drop for RunChild {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        let writer_finished = match self.control_writer.as_mut() {
            Some(writer) => writer.shutdown_with_timeout(RUN_CONTROL_SHUTDOWN_TIMEOUT),
            None => true,
        };
        shutdown_child_gracefully(&mut self.child);
        if !writer_finished {
            if let Some(writer) = self.control_writer.as_mut() {
                writer.finish_after_child_shutdown(RUN_CONTROL_SHUTDOWN_TIMEOUT);
            }
        }
        crate::platform::cleanup_run_config(&self.config_path);
    }
}

pub(crate) trait ChildShutdownTarget {
    fn try_wait_for_exit(&mut self) -> std::io::Result<bool>;
    fn force_kill(&mut self) -> std::io::Result<()>;
}

struct ProcessChildShutdownTarget<'a>(&'a mut Child);

impl ChildShutdownTarget for ProcessChildShutdownTarget<'_> {
    fn try_wait_for_exit(&mut self) -> std::io::Result<bool> {
        self.0.try_wait().map(|status| status.is_some())
    }

    fn force_kill(&mut self) -> std::io::Result<()> {
        self.0.kill()
    }
}

fn poll_child_exit(
    target: &mut impl ChildShutdownTarget,
    attempts: usize,
    pause: &mut impl FnMut(),
) -> bool {
    for _ in 0..attempts {
        match target.try_wait_for_exit() {
            Ok(true) => return true,
            Ok(false) => pause(),
            Err(_) => return false,
        }
    }
    false
}

pub(crate) fn shutdown_target_bounded(
    target: &mut impl ChildShutdownTarget,
    graceful_attempts: usize,
    forced_attempts: usize,
    mut pause: impl FnMut(),
) -> bool {
    if poll_child_exit(target, graceful_attempts, &mut pause) {
        return true;
    }
    let _ = target.force_kill();
    poll_child_exit(target, forced_attempts, &mut pause)
}

/// 优雅停机协议(审计 B-01):正常 RunChild 路径已由 control writer 关闭 stdin；
/// 启动失败等 writer 尚未接管的路径仍在这里关闭 child.stdin。优雅等待与 kill 后
/// reaping 都只做有界 try_wait 轮询，任何 OS kill/wait 异常都不能无限阻塞 GUI。
pub(crate) fn shutdown_child_gracefully(child: &mut Child) {
    drop(child.stdin.take());
    let mut target = ProcessChildShutdownTarget(child);
    let _ = shutdown_target_bounded(
        &mut target,
        CHILD_GRACEFUL_SHUTDOWN_POLL_ATTEMPTS,
        CHILD_FORCED_SHUTDOWN_POLL_ATTEMPTS,
        || std::thread::sleep(CHILD_SHUTDOWN_POLL_INTERVAL),
    );
}

/// 当前运行中的 echoless run 子进程及其代际。
/// 主动停止会在同一短临界区摘走代际和 child,使 reader 尾部 status/exit
/// 立即失效；reader 自然退出则在主线程按代际提交最终状态。
pub(crate) struct RunStateInner {
    pub(crate) child: Option<RunChild>,
    pub(crate) active_run_id: Option<RunId>,
    pub(crate) starting_run_id: Option<RunId>,
    next_run_id: RunId,
}

impl Default for RunStateInner {
    fn default() -> Self {
        Self {
            child: None,
            active_run_id: None,
            starting_run_id: None,
            next_run_id: 1,
        }
    }
}

pub(crate) struct RunState(pub(crate) Mutex<RunStateInner>);

impl Default for RunState {
    fn default() -> Self {
        Self(Mutex::new(RunStateInner::default()))
    }
}

pub(crate) fn run_state_guard(state: &RunState) -> MutexGuard<'_, RunStateInner> {
    // Keep the GUI backend recoverable after an unrelated panic while holding the run lock.
    state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn allocate_run_id(state: &mut RunStateInner) -> Result<RunId, String> {
    let run_id = state.next_run_id;
    state.next_run_id = state
        .next_run_id
        .checked_add(1)
        .ok_or("run id space exhausted")?;
    Ok(run_id)
}

/// 在单个短临界区内摘走旧进程并预留下一代。调用者必须在锁外回收返回的
/// child，并完成配置写入、binary resolve 与 spawn 等外部准备。
pub(crate) fn reserve_run_start(state: &RunState) -> Result<(RunId, Option<RunChild>), String> {
    let mut guard = run_state_guard(state);
    let run_id = allocate_run_id(&mut guard)?;
    let old_child = guard.child.take();
    guard.active_run_id = None;
    guard.starting_run_id = Some(run_id);
    Ok((run_id, old_child))
}

/// 仅当 reservation 仍属于本代时，原子安装 child 与 active generation。
/// 迟到或不一致的 child 在 guard 释放后立即走 RunChild::Drop 回收。
pub(crate) fn commit_run_start(state: &RunState, run_id: RunId, child: RunChild) -> bool {
    let mut child = Some(child);
    let installed = {
        let mut guard = run_state_guard(state);
        let matches_reservation = guard.starting_run_id == Some(run_id);
        let matches_child = child
            .as_ref()
            .is_some_and(|candidate| candidate.run_id == run_id);
        if !matches_reservation
            || !matches_child
            || guard.active_run_id.is_some()
            || guard.child.is_some()
        {
            false
        } else {
            guard.starting_run_id = None;
            guard.active_run_id = Some(run_id);
            guard.child = child.take();
            true
        }
    };
    drop(child);
    installed
}

/// 仅撤销调用者自己的 reservation，不能让旧启动失败覆盖更新一代。
pub(crate) fn cancel_run_start(state: &RunState, run_id: RunId) -> bool {
    let mut guard = run_state_guard(state);
    if guard.starting_run_id != Some(run_id) {
        return false;
    }
    guard.starting_run_id = None;
    true
}

/// 在短临界区内快照代际，再于锁外执行副作用。快照之后允许代际变化；
/// status/exit payload 自带 run_id，由前端拒绝晚到事件。
pub(crate) fn is_active_run(state: &RunState, run_id: RunId) -> bool {
    run_state_guard(state).active_run_id == Some(run_id)
}

/// 只在短锁内复制控制队列 sender；enqueue 与 ack 等待由调用者在锁外完成。
pub(crate) fn run_control_sender(state: &RunState) -> Result<RunControlSender, String> {
    let guard = run_state_guard(state);
    let child = guard.child.as_ref().ok_or("not running")?;
    if guard.active_run_id != Some(child.run_id) {
        return Err("run state generation mismatch".to_string());
    }
    child
        .control_writer
        .as_ref()
        .and_then(RunControlWriter::sender)
        .ok_or("run control writer is not running".to_string())
}

pub(crate) fn terminate_run(state: &RunState) -> Option<RunId> {
    // 锁内只提交停止状态；RunChild::Drop 的 stdin EOF、限时等待、kill 与
    // 临时配置清理全部在锁外执行。
    let (run_id, child_opt) = {
        let mut guard = run_state_guard(state);
        guard.starting_run_id = None;
        (guard.active_run_id.take(), guard.child.take())
    };
    drop(child_opt);
    run_id
}

/// reader 代际提交的纯状态结果。外部资源回收、配置清理和 GUI/event 副作用
/// 由 sidecar 的私有主线程 finalizer 消费，不能从任意线程把 callback 带进锁域。
#[must_use = "finalization outcomes own child cleanup and must be consumed"]
pub(crate) enum RunFinalization {
    Stale,
    ActiveWithoutChild,
    ActiveWithChild(RunChild),
    ActiveChildMismatch { child: RunChild },
}

/// 在短临界区内提交 reader finalization：只判断代际、摘 child、清 active。
/// 即使状态出现 active/child 代际不一致，也会摘走 child 交给调用者诊断并回收，
/// 不把无法再由正常 reader 命中的进程永久遗留在 RunState。
pub(crate) fn commit_run_finalization(state: &RunState, run_id: RunId) -> RunFinalization {
    let mut guard = run_state_guard(state);
    if guard.active_run_id != Some(run_id) {
        return RunFinalization::Stale;
    }

    guard.active_run_id = None;
    match guard.child.take() {
        None => RunFinalization::ActiveWithoutChild,
        Some(child) if child.run_id == run_id => RunFinalization::ActiveWithChild(child),
        Some(child) => RunFinalization::ActiveChildMismatch { child },
    }
}

#[cfg(test)]
pub(crate) fn install_test_generation(state: &RunState, run_id: RunId) {
    let mut guard = run_state_guard(state);
    guard.active_run_id = Some(run_id);
    guard.starting_run_id = None;
    if guard.next_run_id <= run_id {
        guard.next_run_id = run_id.saturating_add(1);
    }
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
                kill_child_tree(&mut child);
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

#[cfg(windows)]
fn kill_child_tree(child: &mut Child) {
    let pid = child.id().to_string();
    let mut taskkill = Command::new("taskkill");
    taskkill.args(["/PID", &pid, "/T", "/F"]);
    suppress_child_console(&mut taskkill);
    let _ = taskkill.status();
    let _ = child.kill();
}

#[cfg(not(windows))]
fn kill_child_tree(child: &mut Child) {
    let _ = child.kill();
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
