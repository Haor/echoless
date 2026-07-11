use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;
use serde_json::Value;

use crate::bin_resolve::find_localvqe_library_in_dir;
use crate::localvqe::{
    localvqe_model_pin, migrate_legacy_localvqe_models_from_base, verify_localvqe_model_file,
    LocalVqeModelPin,
};
use crate::platform::{
    browser_open_command, cleanup_run_config, default_diag_dir, ensure_diagnostics_dir,
    validate_browser_url, validate_open_path, write_toml_create_new, write_transient_config_toml,
};
use crate::proc::{
    cancel_run_start, command_output_with_timeout, commit_run_finalization, commit_run_start,
    install_test_generation, is_active_run, parse_jsonl_line_event, push_tail_line,
    reserve_run_start, run_state_guard, shutdown_target_bounded, terminate_run,
    ChildShutdownTarget, JsonlLineEvent, RunChild, RunControlWriter, RunFinalization, RunState,
};
use crate::sidecar::write_run_control_line;
use crate::sidecar::{attach_run_id, bypass_control_line};
use crate::tray::{set_tray_prefs_inner, TrayPrefs};

static DATA_ROOT_ENV_LOCK: Mutex<()> = Mutex::new(());

fn unique_temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn with_test_data_root<T>(root: &Path, run: impl FnOnce() -> T) -> T {
    let _guard = DATA_ROOT_ENV_LOCK.lock().unwrap();
    let previous = std::env::var_os(echoless_paths::DATA_ROOT_ENV_VAR);
    std::env::set_var(echoless_paths::DATA_ROOT_ENV_VAR, root);
    let result = run();
    if let Some(previous) = previous {
        std::env::set_var(echoless_paths::DATA_ROOT_ENV_VAR, previous);
    } else {
        std::env::remove_var(echoless_paths::DATA_ROOT_ENV_VAR);
    }
    result
}

fn complete_test_finalization(
    outcome: RunFinalization,
    config_path: &Path,
    on_active: impl FnOnce(),
) -> bool {
    let child = match outcome {
        RunFinalization::Stale => {
            cleanup_run_config(config_path);
            return false;
        }
        RunFinalization::ActiveWithoutChild => None,
        RunFinalization::ActiveWithChild(child)
        | RunFinalization::ActiveChildMismatch { child } => Some(child),
    };
    drop(child);
    cleanup_run_config(config_path);
    on_active();
    true
}

#[cfg(unix)]
fn slow_child_command() -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 2"]);
    command
}

#[cfg(windows)]
fn slow_child_command() -> Command {
    let mut command = Command::new("cmd");
    command.args(["/C", "ping -n 3 127.0.0.1 > nul"]);
    command
}

#[cfg(unix)]
fn exited_child_command() -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", "exit 0"]);
    command.stdin(Stdio::piped());
    command
}

#[cfg(windows)]
fn exited_child_command() -> Command {
    let mut command = Command::new("cmd");
    command.args(["/C", "exit 0"]);
    command.stdin(Stdio::piped());
    command
}

struct BlockingWriter {
    entered: Option<mpsc::Sender<()>>,
    release: mpsc::Receiver<()>,
    dropped: Option<mpsc::Sender<()>>,
}

impl Write for BlockingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(entered) = self.entered.take() {
            let _ = entered.send(());
            self.release.recv().map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "blocking writer release closed")
            })?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for BlockingWriter {
    fn drop(&mut self) {
        if let Some(dropped) = self.dropped.take() {
            let _ = dropped.send(());
        }
    }
}

struct GatedRecordingWriter {
    entered: Option<mpsc::Sender<()>>,
    release: mpsc::Receiver<()>,
    output: Arc<Mutex<Vec<u8>>>,
}

impl Write for GatedRecordingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(entered) = self.entered.take() {
            let _ = entered.send(());
            self.release.recv().map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "recording writer release closed")
            })?;
        }
        self.output.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct DropTrackingWriter(Option<mpsc::Sender<()>>);

impl Write for DropTrackingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for DropTrackingWriter {
    fn drop(&mut self) {
        if let Some(dropped) = self.0.take() {
            let _ = dropped.send(());
        }
    }
}

#[derive(Clone, Copy)]
enum WriterFailure {
    Write,
    Flush,
}

struct FailingWriter(WriterFailure);

impl Write for FailingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.0 {
            WriterFailure::Write => Err(io::Error::other("write boom")),
            WriterFailure::Flush => Ok(bytes.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.0 {
            WriterFailure::Write => Ok(()),
            WriterFailure::Flush => Err(io::Error::other("flush boom")),
        }
    }
}

#[derive(Default)]
struct StubbornShutdownTarget {
    try_wait_calls: usize,
    kill_calls: usize,
}

impl ChildShutdownTarget for StubbornShutdownTarget {
    fn try_wait_for_exit(&mut self) -> io::Result<bool> {
        self.try_wait_calls += 1;
        Ok(false)
    }

    fn force_kill(&mut self) -> io::Result<()> {
        self.kill_calls += 1;
        Ok(())
    }
}

#[test]
fn forced_child_shutdown_stays_bounded_when_kill_does_not_exit() {
    let mut target = StubbornShutdownTarget::default();
    let exited = shutdown_target_bounded(&mut target, 2, 3, || {});

    assert!(!exited);
    assert_eq!(target.kill_calls, 1);
    assert_eq!(target.try_wait_calls, 5);
}

#[test]
fn command_timeout_kills_hung_child() {
    let mut command = slow_child_command();
    let started = Instant::now();
    let err =
        command_output_with_timeout(&mut command, Duration::from_millis(80), "slow child test")
            .unwrap_err();

    assert!(err.contains("timed out"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn run_state_guard_recovers_poisoned_lock() {
    let state = RunState::default();
    let _ = std::panic::catch_unwind(|| {
        let _guard = state.0.lock().expect("test lock should start healthy");
        panic!("poison run state");
    });

    assert!(state.0.is_poisoned());
    let guard = run_state_guard(&state);
    assert!(guard.child.is_none());
    assert!(guard.active_run_id.is_none());
}

#[test]
fn bypass_control_line_matches_runtime_contract() {
    let enabled: Value = serde_json::from_str(&bypass_control_line(true)).unwrap();
    assert_eq!(enabled["cmd"], "set_bypass");
    assert_eq!(enabled["enabled"], true);

    let disabled: Value = serde_json::from_str(&bypass_control_line(false)).unwrap();
    assert_eq!(disabled["cmd"], "set_bypass");
    assert_eq!(disabled["enabled"], false);
}

#[test]
fn jsonl_line_event_classifies_status_lines() {
    assert_eq!(parse_jsonl_line_event("   "), JsonlLineEvent::Empty);
    assert_eq!(
        parse_jsonl_line_event(r#"{"type":"status","ok":true}"#),
        JsonlLineEvent::Json(json!({"type": "status", "ok": true}))
    );
    assert_eq!(
        parse_jsonl_line_event("not json"),
        JsonlLineEvent::Unparsed("not json".to_string())
    );
}

#[test]
fn push_tail_line_truncates_without_splitting_utf8() {
    let mut tail = String::new();
    push_tail_line(&mut tail, "ascii-prefix", 32);
    push_tail_line(&mut tail, "错误错误错误错误错误", 16);

    assert!(tail.len() <= 16, "{tail:?}");
    assert!(tail.ends_with('\n'));
    assert!(std::str::from_utf8(tail.as_bytes()).is_ok());
}

#[test]
fn default_diag_dir_uses_brand_data_root() {
    let root = unique_temp_dir("echoless-diag-root");
    with_test_data_root(&root, || {
        assert_eq!(PathBuf::from(default_diag_dir()), root.join("diagnostics"));
    });
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ensure_diagnostics_dir_creates_only_the_fixed_directory() {
    let root = unique_temp_dir("echoless-diag-create-root");
    let external = unique_temp_dir("echoless-diag-create-external");
    let sentinel = external.join("sentinel.txt");
    std::fs::write(&sentinel, b"keep").unwrap();
    with_test_data_root(&root, || {
        let diagnostics = ensure_diagnostics_dir().unwrap();
        assert_eq!(diagnostics, root.join("diagnostics"));
        assert!(diagnostics.is_dir());
    });
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep");
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(external);
}

#[test]
fn migrate_legacy_localvqe_models_moves_only_missing_gguf_files() {
    let legacy_base = unique_temp_dir("echoless-legacy-localvqe");
    let legacy_models = legacy_base.join("localvqe").join("models");
    std::fs::create_dir_all(&legacy_models).unwrap();
    let dest = unique_temp_dir("echoless-localvqe-dest");

    std::fs::write(legacy_models.join("move-me.gguf"), b"new").unwrap();
    std::fs::write(legacy_models.join("keep-existing.gguf"), b"legacy").unwrap();
    std::fs::write(legacy_models.join("notes.txt"), b"ignore").unwrap();
    std::fs::write(dest.join("keep-existing.gguf"), b"dest").unwrap();

    migrate_legacy_localvqe_models_from_base(&legacy_base, &dest);

    assert_eq!(std::fs::read(dest.join("move-me.gguf")).unwrap(), b"new");
    assert!(!legacy_models.join("move-me.gguf").exists());
    assert_eq!(
        std::fs::read(dest.join("keep-existing.gguf")).unwrap(),
        b"dest"
    );
    assert!(legacy_models.join("keep-existing.gguf").exists());
    assert!(!dest.join("notes.txt").exists());

    let _ = std::fs::remove_dir_all(legacy_base);
    let _ = std::fs::remove_dir_all(dest);
}

#[test]
fn terminate_run_clears_generation_and_rejects_tail_status() {
    let dir = unique_temp_dir("echoless-terminate-run");
    let config_path = dir.join("run.toml");
    std::fs::write(&config_path, "stub = true").unwrap();
    let stopping = Arc::new(AtomicBool::new(false));
    let child = slow_child_command().spawn().unwrap();
    let state = RunState::default();
    {
        let mut guard = run_state_guard(&state);
        guard.active_run_id = Some(1);
        guard.child = Some(RunChild {
            run_id: 1,
            child,
            stopping: stopping.clone(),
            config_path: config_path.clone(),
            control_writer: None,
        });
    }

    assert_eq!(terminate_run(&state), Some(1));

    assert!(stopping.load(Ordering::SeqCst));
    let guard = run_state_guard(&state);
    assert!(guard.child.is_none());
    assert_eq!(guard.active_run_id, None);
    assert!(!config_path.exists());
    drop(guard);

    let tail_effects = AtomicUsize::new(0);
    if is_active_run(&state, 1) {
        tail_effects.fetch_add(1, Ordering::SeqCst);
    }
    assert_eq!(tail_effects.load(Ordering::SeqCst), 0);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn status_side_effect_does_not_hold_run_state_lock() {
    let state = Arc::new(RunState::default());
    install_test_generation(&state, 1);
    let (effect_entered_tx, effect_entered_rx) = mpsc::channel();
    let (release_effect_tx, release_effect_rx) = mpsc::channel();

    let worker_state = state.clone();
    let status_worker = std::thread::spawn(move || {
        let active = is_active_run(&worker_state, 1);
        if active {
            let _ = effect_entered_tx.send(());
            let _ = release_effect_rx.recv();
        }
        active
    });

    effect_entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("status side effect did not start");
    let (lock_acquired_tx, lock_acquired_rx) = mpsc::channel();
    let lock_state = state.clone();
    let lock_attempt = std::thread::spawn(move || {
        let _guard = run_state_guard(&lock_state);
        let _ = lock_acquired_tx.send(());
    });

    let acquired_while_effect_blocked = lock_acquired_rx
        .recv_timeout(Duration::from_millis(200))
        .is_ok();
    let _ = release_effect_tx.send(());
    assert!(status_worker.join().unwrap());
    lock_attempt.join().unwrap();

    assert!(
        acquired_while_effect_blocked,
        "RunState stayed locked while the status side effect was blocked"
    );
}

#[test]
fn start_reservation_releases_state_before_old_child_drop() {
    let dir = unique_temp_dir("echoless-start-reservation-lock");
    let config_path = dir.join("run.toml");
    std::fs::write(&config_path, "run = 1").unwrap();
    let stopping = Arc::new(AtomicBool::new(false));
    let child = slow_child_command().spawn().unwrap();
    let state = Arc::new(RunState::default());
    install_test_generation(&state, 1);
    run_state_guard(&state).child = Some(RunChild {
        run_id: 1,
        child,
        stopping: stopping.clone(),
        config_path: config_path.clone(),
        control_writer: None,
    });

    let (reserved_tx, reserved_rx) = mpsc::channel();
    let (drop_finished_tx, drop_finished_rx) = mpsc::channel();
    let worker_state = state.clone();
    let starter = std::thread::spawn(move || {
        let (run_id, old_child) = reserve_run_start(&worker_state).unwrap();
        let _ = reserved_tx.send(run_id);
        drop(old_child);
        let _ = drop_finished_tx.send(());
        run_id
    });

    assert_eq!(reserved_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 2);
    let stopping_deadline = Instant::now() + Duration::from_secs(1);
    while !stopping.load(Ordering::SeqCst) && Instant::now() < stopping_deadline {
        std::thread::yield_now();
    }
    assert!(
        stopping.load(Ordering::SeqCst),
        "old child drop did not start"
    );
    assert!(
        drop_finished_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "old child drop did not remain blocked long enough to test lock scope"
    );

    let (lock_acquired_tx, lock_acquired_rx) = mpsc::channel();
    let lock_state = state.clone();
    let lock_attempt = std::thread::spawn(move || {
        let _guard = run_state_guard(&lock_state);
        let _ = lock_acquired_tx.send(());
    });
    assert!(
        lock_acquired_rx
            .recv_timeout(Duration::from_millis(200))
            .is_ok(),
        "RunState stayed locked while the old child was stopping"
    );

    lock_attempt.join().unwrap();
    assert_eq!(starter.join().unwrap(), 2);
    assert!(!config_path.exists());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn expired_start_reservation_reaps_late_child() {
    let state = RunState::default();
    let (expired_run_id, old_child) = reserve_run_start(&state).unwrap();
    assert!(old_child.is_none());
    let (current_run_id, old_child) = reserve_run_start(&state).unwrap();
    assert!(old_child.is_none());

    let dir = unique_temp_dir("echoless-expired-start");
    let config_path = dir.join("run.toml");
    std::fs::write(&config_path, "run = 1").unwrap();
    let stopping = Arc::new(AtomicBool::new(false));
    let child = slow_child_command().spawn().unwrap();
    let installed = commit_run_start(
        &state,
        expired_run_id,
        RunChild {
            run_id: expired_run_id,
            child,
            stopping: stopping.clone(),
            config_path: config_path.clone(),
            control_writer: None,
        },
    );

    assert!(!installed);
    assert!(stopping.load(Ordering::SeqCst));
    assert!(!config_path.exists());
    let guard = run_state_guard(&state);
    assert_eq!(guard.starting_run_id, Some(current_run_id));
    assert!(guard.active_run_id.is_none());
    assert!(guard.child.is_none());
    drop(guard);

    assert_eq!(terminate_run(&state), None);
    assert_eq!(run_state_guard(&state).starting_run_id, None);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn committed_start_keeps_active_and_child_generations_consistent() {
    let state = RunState::default();
    let (run_id, old_child) = reserve_run_start(&state).unwrap();
    assert!(old_child.is_none());
    let dir = unique_temp_dir("echoless-committed-start");
    let config_path = dir.join("run.toml");
    std::fs::write(&config_path, "run = 1").unwrap();
    let stopping = Arc::new(AtomicBool::new(false));
    let child = slow_child_command().spawn().unwrap();

    assert!(commit_run_start(
        &state,
        run_id,
        RunChild {
            run_id,
            child,
            stopping: stopping.clone(),
            config_path: config_path.clone(),
            control_writer: None,
        },
    ));
    {
        let guard = run_state_guard(&state);
        assert_eq!(guard.starting_run_id, None);
        assert_eq!(guard.active_run_id, Some(run_id));
        assert_eq!(guard.child.as_ref().map(|child| child.run_id), Some(run_id));
    }

    assert_eq!(terminate_run(&state), Some(run_id));
    assert!(stopping.load(Ordering::SeqCst));
    assert!(!config_path.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn start_cancellation_only_clears_matching_reservation() {
    let state = RunState::default();
    let (expired_run_id, old_child) = reserve_run_start(&state).unwrap();
    assert!(old_child.is_none());
    let (current_run_id, old_child) = reserve_run_start(&state).unwrap();
    assert!(old_child.is_none());

    assert!(!cancel_run_start(&state, expired_run_id));
    assert_eq!(
        run_state_guard(&state).starting_run_id,
        Some(current_run_id)
    );
    assert!(cancel_run_start(&state, current_run_id));
    assert_eq!(run_state_guard(&state).starting_run_id, None);
}

#[test]
fn write_run_control_line_reports_exited_writer_before_stop_reaps_child() {
    let dir = unique_temp_dir("echoless-run-control-exited");
    let config_path = dir.join("run.toml");
    std::fs::write(&config_path, "stub = true").unwrap();
    let stopping = Arc::new(AtomicBool::new(false));
    let mut child = exited_child_command().spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    let control_writer = RunControlWriter::spawn_with_capacity(stdin, 1).unwrap();
    child.wait().unwrap();
    let state = RunState::default();
    {
        let mut guard = run_state_guard(&state);
        guard.active_run_id = Some(1);
        guard.child = Some(RunChild {
            run_id: 1,
            child,
            stopping: stopping.clone(),
            config_path: config_path.clone(),
            control_writer: Some(control_writer),
        });
    }

    let err = write_run_control_line(&state, &bypass_control_line(true)).unwrap_err();

    assert!(err.contains("write run control failed"), "{err}");
    assert_eq!(terminate_run(&state), Some(1));
    assert!(stopping.load(Ordering::SeqCst));
    assert!(run_state_guard(&state).child.is_none());
    assert!(!config_path.exists());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn blocked_control_writer_times_out_without_holding_run_state_lock() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let control_writer = RunControlWriter::spawn_with_capacity(
        BlockingWriter {
            entered: Some(entered_tx),
            release: release_rx,
            dropped: None,
        },
        1,
    )
    .unwrap();
    let dir = unique_temp_dir("echoless-control-lock");
    let config_path = dir.join("run.toml");
    std::fs::write(&config_path, "run = 1").unwrap();
    let state = Arc::new(RunState::default());
    install_test_generation(&state, 1);
    run_state_guard(&state).child = Some(RunChild {
        run_id: 1,
        child: exited_child_command().spawn().unwrap(),
        stopping: Arc::new(AtomicBool::new(false)),
        config_path: config_path.clone(),
        control_writer: Some(control_writer),
    });

    let writer_state = state.clone();
    let writer = std::thread::spawn(move || {
        let started = Instant::now();
        let result = write_run_control_line(&writer_state, "control");
        (started.elapsed(), result)
    });
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("writer did not block");

    let (lock_tx, lock_rx) = mpsc::channel();
    let lock_state = state.clone();
    let lock_attempt = std::thread::spawn(move || {
        let _guard = run_state_guard(&lock_state);
        let _ = lock_tx.send(());
    });
    assert!(
        lock_rx.recv_timeout(Duration::from_millis(100)).is_ok(),
        "RunState stayed locked while the control writer was blocked"
    );

    let (elapsed, result) = writer.join().unwrap();
    let err = result.unwrap_err();
    assert!(err.contains("timed out"), "{err}");
    assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
    let _ = release_tx.send(());
    lock_attempt.join().unwrap();
    assert_eq!(terminate_run(&state), Some(1));
    assert!(!config_path.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn queued_control_timeout_cancels_request_before_writer_executes_it() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let output = Arc::new(Mutex::new(Vec::new()));
    let mut writer = RunControlWriter::spawn_with_capacity(
        GatedRecordingWriter {
            entered: Some(entered_tx),
            release: release_rx,
            output: output.clone(),
        },
        2,
    )
    .unwrap();
    let sender = writer.sender().unwrap();
    let first_sender = sender.clone();
    let first = std::thread::spawn(move || {
        first_sender.send_line_with_timeout("first", Duration::from_secs(1))
    });
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("writer did not block");

    let err = sender
        .send_line_with_timeout("cancelled", Duration::from_millis(30))
        .unwrap_err();
    assert!(err.contains("cancelled before writing"), "{err}");

    release_tx.send(()).unwrap();
    first.join().unwrap().unwrap();
    sender
        .send_line_with_timeout("third", Duration::from_millis(200))
        .unwrap();
    assert!(writer.shutdown_with_timeout(Duration::from_millis(200)));
    assert_eq!(output.lock().unwrap().as_slice(), b"first\nthird\n");
}

#[test]
fn writing_control_timeout_reports_unknown_outcome() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut writer = RunControlWriter::spawn_with_capacity(
        BlockingWriter {
            entered: Some(entered_tx),
            release: release_rx,
            dropped: None,
        },
        1,
    )
    .unwrap();
    let sender = writer.sender().unwrap();

    let err = sender
        .send_line_with_timeout("writing", Duration::from_millis(30))
        .unwrap_err();
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("writer did not start writing");
    assert!(err.contains("outcome is unknown"), "{err}");

    release_tx.send(()).unwrap();
    assert!(writer.shutdown_with_timeout(Duration::from_millis(200)));
}

#[test]
fn full_control_queue_returns_diagnostic_error() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut writer = RunControlWriter::spawn_with_capacity(
        BlockingWriter {
            entered: Some(entered_tx),
            release: release_rx,
            dropped: None,
        },
        1,
    )
    .unwrap();
    let sender = writer.sender().unwrap();
    let first_sender = sender.clone();
    let first = std::thread::spawn(move || {
        first_sender.send_line_with_timeout("first", Duration::from_secs(1))
    });
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("writer did not block");

    let queued_err = sender
        .send_line_with_timeout("second", Duration::from_millis(30))
        .unwrap_err();
    assert!(queued_err.contains("timed out"), "{queued_err}");
    let full_err = sender
        .send_line_with_timeout("third", Duration::from_millis(30))
        .unwrap_err();
    assert!(full_err.contains("queue is full"), "{full_err}");

    assert!(!writer.shutdown_with_timeout(Duration::from_millis(30)));
    let _ = release_tx.send(());
    assert!(first.join().unwrap().is_ok());
    assert!(writer.shutdown_with_timeout(Duration::from_millis(200)));
    drop(sender);
}

#[test]
fn normal_control_shutdown_drops_stdin_owner_and_finishes_writer() {
    let (dropped_tx, dropped_rx) = mpsc::channel();
    let mut writer =
        RunControlWriter::spawn_with_capacity(DropTrackingWriter(Some(dropped_tx)), 1).unwrap();
    let sender = writer.sender().unwrap();

    sender
        .send_line_with_timeout("control", Duration::from_millis(200))
        .unwrap();
    assert!(writer.shutdown_with_timeout(Duration::from_millis(200)));
    dropped_rx
        .recv_timeout(Duration::from_millis(200))
        .expect("stdin owner was not dropped");

    let err = sender
        .send_line_with_timeout("late", Duration::from_millis(30))
        .unwrap_err();
    assert!(err.contains("writer is not running"), "{err}");
}

#[test]
fn blocked_writer_does_not_make_run_child_drop_unbounded() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (writer_dropped_tx, writer_dropped_rx) = mpsc::channel();
    let writer = RunControlWriter::spawn_with_capacity(
        BlockingWriter {
            entered: Some(entered_tx),
            release: release_rx,
            dropped: Some(writer_dropped_tx),
        },
        1,
    )
    .unwrap();
    let sender = writer.sender().unwrap();
    let request = std::thread::spawn(move || {
        sender.send_line_with_timeout("control", Duration::from_secs(2))
    });
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("writer did not block");

    let dir = unique_temp_dir("echoless-blocked-writer-drop");
    let config_path = dir.join("run.toml");
    std::fs::write(&config_path, "run = 1").unwrap();
    let run_child = RunChild {
        run_id: 1,
        child: exited_child_command().spawn().unwrap(),
        stopping: Arc::new(AtomicBool::new(false)),
        config_path: config_path.clone(),
        control_writer: Some(writer),
    };
    let (drop_tx, drop_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let started = Instant::now();
        drop(run_child);
        let _ = drop_tx.send(started.elapsed());
    });

    let elapsed = drop_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("RunChild drop blocked indefinitely");
    assert!(elapsed < Duration::from_millis(800), "{elapsed:?}");
    assert!(!config_path.exists());

    let _ = release_tx.send(());
    assert!(request.join().unwrap().is_ok());
    writer_dropped_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("detached writer did not finish after release");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn child_shutdown_cancels_blocked_writer_and_joins_after_release() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (cancelled_tx, cancelled_rx) = mpsc::channel();
    let (writer_dropped_tx, writer_dropped_rx) = mpsc::channel();
    let writer = RunControlWriter::spawn_with_capacity_and_canceller(
        BlockingWriter {
            entered: Some(entered_tx),
            release: release_rx,
            dropped: Some(writer_dropped_tx),
        },
        1,
        move |_| {
            let _ = cancelled_tx.send(());
            let _ = release_tx.send(());
        },
    )
    .unwrap();
    let sender = writer.sender().unwrap();
    let request = std::thread::spawn(move || {
        sender.send_line_with_timeout("control", Duration::from_secs(2))
    });
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("writer did not block");

    let dir = unique_temp_dir("echoless-cancel-writer-drop");
    let config_path = dir.join("run.toml");
    std::fs::write(&config_path, "run = 1").unwrap();
    let run_child = RunChild {
        run_id: 1,
        child: exited_child_command().spawn().unwrap(),
        stopping: Arc::new(AtomicBool::new(false)),
        config_path: config_path.clone(),
        control_writer: Some(writer),
    };

    drop(run_child);

    cancelled_rx
        .recv_timeout(Duration::from_millis(200))
        .expect("blocked writer I/O was not cancelled");
    writer_dropped_rx
        .recv_timeout(Duration::from_millis(200))
        .expect("cancelled writer was not joined before RunChild drop returned");
    assert!(request.join().unwrap().is_ok());
    assert!(!config_path.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn control_writer_reports_write_and_flush_errors() {
    for (failure, expected) in [
        (WriterFailure::Write, "write run control failed: write boom"),
        (WriterFailure::Flush, "flush run control failed: flush boom"),
    ] {
        let mut writer = RunControlWriter::spawn_with_capacity(FailingWriter(failure), 1).unwrap();
        let sender = writer.sender().unwrap();
        let err = sender
            .send_line_with_timeout("control", Duration::from_millis(200))
            .unwrap_err();
        assert!(err.contains(expected), "{err}");
        assert!(writer.shutdown_with_timeout(Duration::from_millis(200)));

        let exited_err = sender
            .send_line_with_timeout("late", Duration::from_millis(30))
            .unwrap_err();
        assert!(exited_err.contains("writer is not running"), "{exited_err}");
    }
}

#[test]
fn finalization_side_effect_does_not_hold_run_state_lock() {
    let state = Arc::new(RunState::default());
    install_test_generation(&state, 1);
    let config_dir = unique_temp_dir("echoless-finalize-lock");
    let config = config_dir.join("run.toml");
    std::fs::write(&config, "run = 1").unwrap();
    let side_effects = Arc::new(AtomicUsize::new(0));
    let (effect_entered_tx, effect_entered_rx) = mpsc::channel();
    let (release_effect_tx, release_effect_rx) = mpsc::channel();

    let worker_state = state.clone();
    let worker_effects = side_effects.clone();
    let worker_config = config.clone();
    let finalizer = std::thread::spawn(move || {
        let outcome = commit_run_finalization(&worker_state, 1);
        complete_test_finalization(outcome, &worker_config, || {
            worker_effects.fetch_add(1, Ordering::SeqCst);
            let _ = effect_entered_tx.send(());
            let _ = release_effect_rx.recv();
        })
    });

    let effect_entered = effect_entered_rx.recv_timeout(Duration::from_secs(1));
    let (lock_acquired_tx, lock_acquired_rx) = mpsc::channel();
    let lock_state = state.clone();
    let lock_attempt = std::thread::spawn(move || {
        let _guard = run_state_guard(&lock_state);
        let _ = lock_acquired_tx.send(());
    });

    let acquired_while_effect_blocked = lock_acquired_rx
        .recv_timeout(Duration::from_secs(1))
        .is_ok();
    let released = release_effect_tx.send(()).is_ok();
    let finalization_result = finalizer.join();
    let lock_result = lock_attempt.join();

    assert!(
        effect_entered.is_ok(),
        "finalization did not reach its GUI side effect"
    );
    assert!(released, "GUI side effect stopped waiting before release");
    assert!(finalization_result.unwrap());
    assert!(lock_result.is_ok());
    assert!(
        acquired_while_effect_blocked,
        "RunState stayed locked while the GUI side effect was blocked"
    );
    assert_eq!(side_effects.load(Ordering::SeqCst), 1);

    let _ = std::fs::remove_dir_all(config_dir);
}

#[test]
fn current_generation_finalization_runs_once_and_cleans_before_side_effects() {
    let state = RunState::default();
    install_test_generation(&state, 7);
    let config_dir = unique_temp_dir("echoless-current-finalize");
    let config = config_dir.join("run.toml");
    std::fs::write(&config, "run = 7").unwrap();
    let side_effects = AtomicUsize::new(0);
    let config_present_during_effect = AtomicBool::new(false);

    let first = commit_run_finalization(&state, 7);
    let first_was_active = complete_test_finalization(first, &config, || {
        config_present_during_effect.store(config.exists(), Ordering::SeqCst);
        side_effects.fetch_add(1, Ordering::SeqCst);
    });
    let second = commit_run_finalization(&state, 7);
    let second_was_active = complete_test_finalization(second, &config, || {
        side_effects.fetch_add(1, Ordering::SeqCst);
    });

    assert!(first_was_active);
    assert!(!second_was_active);
    assert!(!config_present_during_effect.load(Ordering::SeqCst));
    assert!(!config.exists());
    assert_eq!(side_effects.load(Ordering::SeqCst), 1);
    assert_eq!(run_state_guard(&state).active_run_id, None);

    let _ = std::fs::remove_dir_all(config_dir);
}

#[test]
fn active_child_generation_mismatch_is_extracted_for_diagnostic_reaping() {
    let config_dir = unique_temp_dir("echoless-finalize-mismatch");
    let reader_config = config_dir.join("run-7.toml");
    let child_config = config_dir.join("run-8.toml");
    std::fs::write(&reader_config, "run = 7").unwrap();
    std::fs::write(&child_config, "run = 8").unwrap();
    let stopping = Arc::new(AtomicBool::new(false));
    let child = slow_child_command().spawn().unwrap();
    let state = RunState::default();
    {
        let mut guard = run_state_guard(&state);
        guard.active_run_id = Some(7);
        guard.child = Some(RunChild {
            run_id: 8,
            child,
            stopping: stopping.clone(),
            config_path: child_config.clone(),
            control_writer: None,
        });
    }

    let outcome = commit_run_finalization(&state, 7);
    let mismatched_child_run_id = match &outcome {
        RunFinalization::ActiveChildMismatch { child } => Some(child.run_id),
        _ => None,
    };
    let state_was_cleared = {
        let guard = run_state_guard(&state);
        guard.active_run_id.is_none() && guard.child.is_none()
    };
    let finalized = complete_test_finalization(outcome, &reader_config, || {});

    assert_eq!(mismatched_child_run_id, Some(8));
    assert!(state_was_cleared);
    assert!(finalized);
    assert!(stopping.load(Ordering::SeqCst));
    assert!(!reader_config.exists());
    assert!(!child_config.exists());

    let _ = std::fs::remove_dir_all(config_dir);
}

#[test]
fn stale_reader_cleans_only_its_config_and_keeps_new_generation() {
    let state = RunState::default();
    install_test_generation(&state, 2);
    let config_dir = unique_temp_dir("echoless-stale-run-a");
    let old_config = config_dir.join("run.toml");
    std::fs::write(&old_config, "run = 1").unwrap();
    let side_effects = AtomicUsize::new(0);

    let outcome = commit_run_finalization(&state, 1);
    let was_active = complete_test_finalization(outcome, &old_config, || {
        side_effects.fetch_add(1, Ordering::SeqCst);
    });

    assert!(!was_active);
    assert_eq!(run_state_guard(&state).active_run_id, Some(2));
    assert_eq!(side_effects.load(Ordering::SeqCst), 0);
    assert!(!old_config.exists());

    let _ = std::fs::remove_dir_all(config_dir);
}

#[test]
fn serialized_main_thread_queues_next_generation_until_finalization_finishes() {
    type MainThreadTask = Box<dyn FnOnce() + Send>;

    let state = Arc::new(RunState::default());
    install_test_generation(&state, 1);
    let config_dir = unique_temp_dir("echoless-main-thread-order");
    let old_config = config_dir.join("run.toml");
    std::fs::write(&old_config, "run = 1").unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));
    let (effect_entered_tx, effect_entered_rx) = mpsc::channel();
    let (release_effect_tx, release_effect_rx) = mpsc::channel();
    let (finalized_tx, finalized_rx) = mpsc::channel();
    let (task_tx, task_rx) = mpsc::channel::<MainThreadTask>();
    let dispatcher = std::thread::spawn(move || {
        while let Ok(task) = task_rx.recv() {
            task();
        }
    });

    let finalize_state = state.clone();
    let finalize_config = old_config.clone();
    let finalize_order = order.clone();
    let queued_a = task_tx
        .send(Box::new(move || {
            let outcome = commit_run_finalization(&finalize_state, 1);
            let finalized = complete_test_finalization(outcome, &finalize_config, || {
                finalize_order
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push("a-enter");
                let _ = effect_entered_tx.send(());
                let _ = release_effect_rx.recv();
                finalize_order
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push("a-exit");
            });
            let _ = finalized_tx.send(finalized);
        }))
        .is_ok();

    let effect_entered = effect_entered_rx.recv_timeout(Duration::from_secs(1));
    let (installed_tx, installed_rx) = mpsc::channel();
    let install_state = state.clone();
    let install_order = order.clone();
    // Queue B on the same serial dispatcher used by A; installing it directly from this
    // test thread would model an interleaving that production's main thread cannot create.
    let queued_b = task_tx
        .send(Box::new(move || {
            install_test_generation(&install_state, 2);
            install_order
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push("b-install");
            let _ = installed_tx.send(());
        }))
        .is_ok();

    let active_before_release = state.0.try_lock().map(|guard| guard.active_run_id).ok();
    let released = release_effect_tx.send(()).is_ok();
    let finalized = finalized_rx.recv_timeout(Duration::from_secs(1));
    let installed = installed_rx.recv_timeout(Duration::from_secs(1));
    drop(task_tx);
    let dispatcher_result = dispatcher.join();

    assert!(queued_a);
    assert!(
        effect_entered.is_ok(),
        "generation A did not reach its main-thread side effect"
    );
    assert!(queued_b);
    assert_eq!(
        active_before_release,
        Some(None),
        "generation B ran while generation A still occupied the main-thread dispatcher"
    );
    assert!(released);
    assert!(finalized.unwrap());
    assert!(installed.is_ok());
    assert!(dispatcher_result.is_ok());
    assert_eq!(
        order.lock().unwrap().as_slice(),
        ["a-enter", "a-exit", "b-install"]
    );
    assert_eq!(run_state_guard(&state).active_run_id, Some(2));

    let _ = std::fs::remove_dir_all(config_dir);
}

#[test]
fn run_id_injection_requires_object_and_overrides_cli_value() {
    assert_eq!(
        attach_run_id(json!({"type": "status", "run_id": 999}), 7).unwrap(),
        json!({"type": "status", "run_id": 7})
    );
    assert_eq!(attach_run_id(json!(["status"]), 7), Err(json!(["status"])));
}

#[test]
fn tray_prefs_default_false_and_follow_platform_gate() {
    let prefs = TrayPrefs::default();
    assert!(!prefs.close_to_tray.load(Ordering::SeqCst));

    set_tray_prefs_inner(&prefs, true);

    #[cfg(target_os = "windows")]
    assert!(prefs.close_to_tray.load(Ordering::SeqCst));
    #[cfg(not(target_os = "windows"))]
    assert!(!prefs.close_to_tray.load(Ordering::SeqCst));
}

#[test]
fn finds_platform_localvqe_native_library() {
    let dir = unique_temp_dir("echoless-localvqe-native");
    let name = if cfg!(target_os = "windows") {
        "localvqe.dll"
    } else if cfg!(target_os = "macos") {
        "liblocalvqe.0.1.0.dylib"
    } else {
        "liblocalvqe.so"
    };
    let expected = dir.join(name);
    std::fs::write(&expected, b"stub").unwrap();
    std::fs::write(dir.join("not-localvqe.txt"), b"stub").unwrap();
    std::fs::write(dir.join("readme.solutions"), b"stub").unwrap();
    std::fs::write(dir.join("liblocalvqe.so.notes"), b"stub").unwrap();

    assert_eq!(
        find_localvqe_library_in_dir(&dir).as_deref(),
        Some(expected.as_path())
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn validates_only_allowlisted_browser_urls() {
    assert_eq!(
        validate_browser_url(" https://vb-audio.com/Cable/?x=1 ").unwrap(),
        "https://vb-audio.com/Cable/?x=1"
    );
    assert_eq!(
        validate_browser_url("https://www.nvidia.com/Download/index.aspx").unwrap(),
        "https://www.nvidia.com/Download/index.aspx"
    );
    assert_eq!(
        validate_browser_url("https://aka.ms/vs/17/release/vc_redist.x64.exe").unwrap(),
        "https://aka.ms/vs/17/release/vc_redist.x64.exe"
    );
    assert_eq!(
        validate_browser_url("HTTPS://GITHUB.COM:443/Haor/Echoless").unwrap(),
        "https://github.com/Haor/Echoless"
    );
    assert!(validate_browser_url("https://github.com./Haor/Echoless").is_ok());
    // 系统设置深链白名单(隐私面板跳转)。
    assert_eq!(
        validate_browser_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture"
        )
        .unwrap(),
        "x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture"
    );

    for bad in [
        "",
        "https://",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "mailto:test@example.com",
        "/Applications/Echoless.app",
        "https://example.com/a b",
        "https://example.com/\ncmd",
        "http://vb-audio.com/Cable/",
        "https://vb-audio.com.evil.example/Cable/",
        "https://evil.example\\@github.com/Haor/Echoless",
        "https://user:password@github.com/Haor/Echoless",
        "https://github.com@evil.example/Haor/Echoless",
        "https://github.com%2f.evil.example/Haor/Echoless",
        "https://github.com%40evil.example/Haor/Echoless",
        "https://github.com:444/Haor/Echoless",
        "x-apple.systempreferences:com.apple.preference.security?General",
    ] {
        assert!(validate_browser_url(bad).is_err(), "{bad}");
    }
}

#[test]
fn validate_open_path_stays_under_brand_data_root() {
    let root = unique_temp_dir("echoless-open-path-root");
    let diagnostics = root.join("diagnostics").join("session-1");
    let models = root.join("localvqe").join("models");
    let external = unique_temp_dir("echoless-open-path-external");
    std::fs::create_dir_all(&diagnostics).unwrap();
    std::fs::create_dir_all(&models).unwrap();

    with_test_data_root(&root, || {
        assert_eq!(
            validate_open_path(diagnostics.to_str().unwrap()).unwrap(),
            diagnostics.canonicalize().unwrap()
        );
        assert_eq!(
            validate_open_path(models.to_str().unwrap()).unwrap(),
            models.canonicalize().unwrap()
        );
        assert!(validate_open_path(root.join("missing").to_str().unwrap()).is_err());
        assert!(validate_open_path(external.to_str().unwrap()).is_err());
    });

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(external);
}

#[test]
fn browser_open_command_avoids_windows_cmd_shell() {
    let (prog, args) = browser_open_command("https://example.com");
    #[cfg(target_os = "windows")]
    {
        assert_eq!(prog, "rundll32.exe");
        assert!(!args.iter().any(|arg| arg == "cmd" || arg == "/C"));
    }
    #[cfg(target_os = "macos")]
    assert_eq!(
        (prog, args),
        ("open", vec!["https://example.com".to_string()])
    );
    #[cfg(target_os = "linux")]
    assert_eq!(
        (prog, args),
        ("xdg-open", vec!["https://example.com".to_string()])
    );
}

#[test]
fn config_writer_uses_create_new_and_refuses_existing_path() {
    let dir = unique_temp_dir("echoless-config-create-new");
    let path = dir.join("existing.toml");
    std::fs::write(&path, "old = true").unwrap();

    let err = write_toml_create_new(&path, "new = true").unwrap_err();
    assert!(err.contains("failed to create config file"), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "old = true");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn transient_config_writer_creates_unique_files() {
    let dir = unique_temp_dir("echoless-transient-config");
    let first = write_transient_config_toml(&dir, "run", "one = true").unwrap();
    let second = write_transient_config_toml(&dir, "run", "two = true").unwrap();

    assert_ne!(first, second);
    assert_ne!(
        first.file_name().and_then(|name| name.to_str()),
        Some("echoless-run.toml")
    );
    assert_ne!(
        second.file_name().and_then(|name| name.to_str()),
        Some("echoless-run.toml")
    );
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "one = true");
    assert_eq!(std::fs::read_to_string(&second).unwrap(), "two = true");

    cleanup_run_config(&first);
    cleanup_run_config(&second);
    assert!(!first.exists());
    assert!(!second.exists());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn localvqe_model_pins_reject_unknown_filenames() {
    assert!(localvqe_model_pin("localvqe-v1.3-4.8M-f32.gguf").is_some());
    assert!(localvqe_model_pin("localvqe-v1.2-1.3M-f32.gguf").is_some());
    assert!(localvqe_model_pin("../localvqe-v1.3-4.8M-f32.gguf").is_none());
    assert!(localvqe_model_pin("localvqe-v1.3-4.8M-f32.gguf.part").is_none());
    assert!(localvqe_model_pin("unknown.gguf").is_none());
}

#[test]
fn localvqe_model_verification_checks_size_and_sha256() {
    let dir = unique_temp_dir("echoless-localvqe-model-verify");
    let path = dir.join("model.gguf");
    std::fs::write(&path, b"abc").unwrap();

    let good = LocalVqeModelPin {
        filename: "model.gguf",
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        size: 3,
    };
    verify_localvqe_model_file(&path, &good).unwrap();

    let wrong_hash = LocalVqeModelPin {
        sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        ..good
    };
    let err = verify_localvqe_model_file(&path, &wrong_hash)
        .unwrap_err()
        .to_string();
    assert!(err.contains("SHA256 mismatch"), "{err}");

    let wrong_size = LocalVqeModelPin { size: 4, ..good };
    let err = verify_localvqe_model_file(&path, &wrong_size)
        .unwrap_err()
        .to_string();
    assert!(err.contains("size mismatch"), "{err}");

    let _ = std::fs::remove_dir_all(dir);
}
