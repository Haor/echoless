// Echoless GUI 的 Tauri 后端:
//   - 平台探测(标题栏镜像)
//   - 把 `echoless` CLI 作为 sidecar 调用,只消费 JSON / JSONL 契约
//   - run 的 --status-json 以 JSONL 流式解析,经事件推给前端
//
// 契约真理源:docs/CLI.md + CLI `--json` 实测。
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(target_os = "windows")]
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle,
};
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};
#[cfg(target_os = "macos")]
use tauri_plugin_decorum::WebviewWindowExt;

/// 运行中的 echoless run 子进程 + 它专属的「正在被主动停止」标记。
/// 每个子进程独立持有 stopping flag,其 stdout reader 退出时据此判断本次退出是
/// 主动停/重启(intentional)还是子进程自己崩了(crash),供前端区分。
struct RunChild {
    child: Child,
    stopping: Arc<AtomicBool>,
    config_path: PathBuf,
}
/// 当前运行中的 echoless run 子进程(同一时刻最多一个)。
struct RunState(Mutex<Option<RunChild>>);

#[cfg(target_os = "windows")]
struct TrayIconState(Mutex<Option<TrayIcon>>);

/// Windows tray preferences pushed by the frontend at startup and on change.
/// 只剩「关闭到托盘」——最小化到托盘已退役(用户定案 2026-07-05)。
struct TrayPrefs {
    close_to_tray: AtomicBool,
}

impl Default for TrayPrefs {
    fn default() -> Self {
        Self {
            close_to_tray: AtomicBool::new(false),
        }
    }
}

fn run_state_guard(state: &RunState) -> MutexGuard<'_, Option<RunChild>> {
    // Keep the GUI backend recoverable after an unrelated panic while holding the run lock.
    state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(target_os = "windows")]
fn tray_icon_state_guard(state: &TrayIconState) -> MutexGuard<'_, Option<TrayIcon>> {
    state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn terminate_run(state: &RunState) {
    let child_opt = {
        let mut guard = run_state_guard(state);
        guard.take()
    };
    if let Some(mut rc) = child_opt {
        rc.stopping.store(true, Ordering::SeqCst);
        let _ = rc.child.kill();
        let _ = rc.child.wait();
        cleanup_run_config(&rc.config_path);
    }
}

fn mark_run_exited(state: &RunState, config_path: &Path) {
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
    if let Some(mut rc) = child_opt {
        let _ = rc.child.wait();
        cleanup_run_config(&rc.config_path);
    } else {
        cleanup_run_config(config_path);
    }
}

fn set_tray_prefs_inner(prefs: &TrayPrefs, close_to_tray: bool) {
    #[cfg(target_os = "windows")]
    {
        prefs.close_to_tray.store(close_to_tray, Ordering::SeqCst);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = close_to_tray;
        prefs.close_to_tray.store(false, Ordering::SeqCst);
    }
}

fn tray_pref_enabled(value: &AtomicBool) -> bool {
    let stored = value.load(Ordering::SeqCst);
    #[cfg(target_os = "windows")]
    {
        stored
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = stored;
        false
    }
}

fn close_to_tray_enabled(prefs: &TrayPrefs) -> bool {
    tray_pref_enabled(&prefs.close_to_tray)
}

fn tray_tooltip(running: bool) -> &'static str {
    if running {
        "Echoless — RUNNING"
    } else {
        "Echoless — STOPPED"
    }
}

fn update_tray_tooltip(app: &tauri::AppHandle, running: bool) {
    let tooltip = tray_tooltip(running);
    #[cfg(target_os = "windows")]
    {
        let tray_state = app.state::<TrayIconState>();
        let tray = tray_icon_state_guard(&tray_state).clone();
        if let Some(tray) = tray {
            let _ = tray.set_tooltip(Some(tooltip));
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, tooltip);
    }
}

#[cfg(target_os = "windows")]
const TRAY_ID: &str = "main-tray";
#[cfg(target_os = "windows")]
const TRAY_MENU_SHOW: &str = "show";
#[cfg(target_os = "windows")]
const TRAY_MENU_QUIT: &str = "quit";

#[cfg(target_os = "windows")]
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg(target_os = "windows")]
fn register_windows_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, TRAY_MENU_SHOW, "Show", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, TRAY_MENU_QUIT, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &separator, &quit_item])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip(tray_tooltip(false))
        .on_menu_event(|app, event| match event.id().as_ref() {
            TRAY_MENU_SHOW => show_main_window(app),
            TRAY_MENU_QUIT => {
                let state = app.state::<RunState>();
                terminate_run(&state);
                update_tray_tooltip(app, false);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    let tray = builder.build(app)?;
    let tray_state = app.state::<TrayIconState>();
    *tray_icon_state_guard(&tray_state) = Some(tray);
    Ok(())
}

const TAURI_TARGET_TRIPLE: &str = env!("TAURI_ENV_TARGET_TRIPLE");
const JSON_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const VALIDATE_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const PROBE_DELAY_TIMEOUT: Duration = Duration::from_secs(45);
const NVAFX_INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MODEL_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn exe_suffix() -> &'static str {
    if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    }
}

fn push_file_candidate(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if !candidates.iter().any(|existing| existing == &path) {
        candidates.push(path);
    }
}

fn resource_path(app: Option<&tauri::AppHandle>, relative: &str) -> Option<PathBuf> {
    app.and_then(|handle| {
        handle
            .path()
            .resolve(relative, tauri::path::BaseDirectory::Resource)
            .ok()
    })
}

fn current_exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

fn transient_config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("runtime-configs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn transient_config_path(dir: &Path, label: &str, attempt: usize) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    dir.join(format!(
        "echoless-{label}-{}-{nanos}-{attempt}.toml",
        std::process::id()
    ))
}

fn write_toml_create_new(path: &Path, toml_text: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("创建配置文件失败: {}: {e}", path.display()))?;
    if let Err(err) = file.write_all(toml_text.as_bytes()) {
        drop(file);
        cleanup_run_config(path);
        return Err(format!("写入配置文件失败: {}: {err}", path.display()));
    }
    if let Err(err) = file.flush() {
        drop(file);
        cleanup_run_config(path);
        return Err(format!("刷新配置文件失败: {}: {err}", path.display()));
    }
    Ok(())
}

fn write_transient_config_toml(
    dir: &Path,
    label: &str,
    toml_text: &str,
) -> Result<PathBuf, String> {
    for attempt in 0..16 {
        let path = transient_config_path(dir, label, attempt);
        match write_toml_create_new(&path, toml_text) {
            Ok(()) => return Ok(path),
            Err(err) if path.exists() => {
                if attempt == 15 {
                    return Err(err);
                }
            }
            Err(err) => return Err(err),
        }
    }
    Err("无法创建唯一配置文件".to_string())
}

fn cleanup_run_config(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

/// 解析 echoless CLI 路径。顺序刻意区分 dev / Tauri build / packaged app:
///   1. ECHOLESS_BIN(开发者显式覆盖);
///   2. Tauri externalBin 被 tauri-build 复制到当前可执行文件旁的 `echoless`;
///   3. Tauri Resource 目录中的候选;
///   4. dev 生成的 `src-tauri/binaries/echoless-<target-triple>`;
///   5. root target release/debug 回退。
fn echoless_bin(app: Option<&tauri::AppHandle>) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var("ECHOLESS_BIN") {
        push_file_candidate(&mut candidates, PathBuf::from(p));
    }

    let exe_name = format!("echoless{}", exe_suffix());
    if let Some(dir) = current_exe_dir() {
        push_file_candidate(&mut candidates, dir.join(&exe_name));
        push_file_candidate(
            &mut candidates,
            dir.join(format!("echoless-{}{}", TAURI_TARGET_TRIPLE, exe_suffix())),
        );
    }

    for rel in [
        format!("echoless{}", exe_suffix()),
        format!("binaries/echoless{}", exe_suffix()),
        format!("binaries/echoless-{}{}", TAURI_TARGET_TRIPLE, exe_suffix()),
    ] {
        if let Some(path) = resource_path(app, &rel) {
            push_file_candidate(&mut candidates, path);
        }
    }

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")); // .../echoless/app/src-tauri
    push_file_candidate(
        &mut candidates,
        manifest
            .join("binaries")
            .join(format!("echoless-{}{}", TAURI_TARGET_TRIPLE, exe_suffix())),
    );
    push_file_candidate(
        &mut candidates,
        manifest
            .join("../../target/release")
            .join(format!("echoless{}", exe_suffix())),
    );
    push_file_candidate(
        &mut candidates,
        manifest
            .join("../../target/debug")
            .join(format!("echoless{}", exe_suffix())),
    );

    candidates
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .ok_or_else(|| {
            format!(
                "echoless CLI not found; tried: {}",
                candidates
                    .iter()
                    .map(|p| p.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" | ")
            )
        })
}

fn process_tap_helper_bin(app: Option<&tauri::AppHandle>, cli: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var("ECHOLESS_PROCESS_TAP_HELPER") {
        push_file_candidate(&mut candidates, PathBuf::from(p));
    }

    if let Some(dir) = cli.parent() {
        for name in ["echoless-process-tap-poc", "echoless-process-tap"] {
            push_file_candidate(&mut candidates, dir.join(name));
        }
    }

    for rel in [
        "resources/helpers/echoless-process-tap-poc",
        "resources/helpers/echoless-process-tap",
    ] {
        if let Some(path) = resource_path(app, rel) {
            push_file_candidate(&mut candidates, path);
        }
    }

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")); // .../echoless/app/src-tauri
    for base in manifest.ancestors() {
        let candidate = base
            .join("tools")
            .join("macos-process-tap-poc")
            .join(".build")
            .join("echoless-process-tap-poc");
        push_file_candidate(&mut candidates, candidate);
    }

    candidates.into_iter().find(|path| path.is_file())
}

fn find_localvqe_library_in_dir(dir: &Path) -> Option<PathBuf> {
    let mut matches = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let is_match = if cfg!(target_os = "windows") {
                name.eq_ignore_ascii_case("localvqe.dll")
            } else if cfg!(target_os = "macos") {
                name.starts_with("liblocalvqe") && name.ends_with(".dylib")
            } else {
                name.starts_with("liblocalvqe") && name.contains(".so")
            };
            if is_match {
                matches.push(path);
            }
        }
    }
    matches.sort();
    matches.into_iter().next()
}

fn localvqe_library_path(app: Option<&tauri::AppHandle>, cli: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var("ECHOLESS_LOCALVQE_LIBRARY") {
        push_file_candidate(&mut candidates, PathBuf::from(p));
    }

    // 产品决策(2026-07-05 修正):native runtime 随包分发,只有模型走 HF 下载。
    // 打包 Resource 目录 → dev 的 src-tauri/resources → 品牌数据根(下载兜底)。
    if let Some(resource_native) = resource_path(app, "resources/localvqe/native") {
        if let Some(path) = find_localvqe_library_in_dir(&resource_native) {
            push_file_candidate(&mut candidates, path);
        }
    }
    let manifest_native = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("localvqe")
        .join("native");
    if let Some(path) = find_localvqe_library_in_dir(&manifest_native) {
        push_file_candidate(&mut candidates, path);
    }

    if let Some(path) = find_localvqe_library_in_dir(&localvqe_native_dir_path()) {
        push_file_candidate(&mut candidates, path);
    }

    if let Some(dir) = cli.parent() {
        if let Some(path) = find_localvqe_library_in_dir(dir) {
            push_file_candidate(&mut candidates, path);
        }
        let localvqe_dir = dir.join("localvqe");
        if let Some(path) = find_localvqe_library_in_dir(&localvqe_dir) {
            push_file_candidate(&mut candidates, path);
        }
    }

    candidates.into_iter().find(|path| path.is_file())
}

fn prepend_env_path(command: &mut Command, key: &str, dir: &Path) {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(existing) = std::env::var_os(key) {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        command.env(key, joined);
    }
}

fn suppress_child_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

fn echoless_command(app: Option<&tauri::AppHandle>) -> Result<Command, String> {
    let cli = echoless_bin(app)?;
    let mut command = Command::new(&cli);
    if let Some(helper) = process_tap_helper_bin(app, &cli) {
        command.env("ECHOLESS_PROCESS_TAP_HELPER", helper);
    }
    if let Some(library) = localvqe_library_path(app, &cli) {
        if let Some(dir) = library.parent() {
            prepend_env_path(&mut command, "PATH", dir);
            prepend_env_path(&mut command, "LD_LIBRARY_PATH", dir);
            prepend_env_path(&mut command, "DYLD_LIBRARY_PATH", dir);
            prepend_env_path(&mut command, "DYLD_FALLBACK_LIBRARY_PATH", dir);
        }
        command.env("ECHOLESS_LOCALVQE_LIBRARY", library);
    }
    Ok(command)
}

fn command_output_with_timeout(
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

fn command_status_error(label: &str, out: &Output) -> String {
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

fn parse_json_output(label: &str, out: Output) -> Result<Value, String> {
    if !out.status.success() {
        return Err(command_status_error(label, &out));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).map_err(|e| format!("parse json failed: {e}; raw: {stdout}"))
}

/// 跑一次性 JSON 子命令(devices / processors / config validate),返回解析后的 JSON。
fn run_json_blocking(
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

async fn run_json_async(
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

#[tauri::command]
fn get_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

#[tauri::command]
async fn list_devices(app: tauri::AppHandle) -> Result<Value, String> {
    run_json_async(
        app,
        vec!["devices".into(), "--json".into(), "--fast".into()],
        JSON_COMMAND_TIMEOUT,
        "devices",
    )
    .await
}

#[tauri::command]
async fn list_processors(app: tauri::AppHandle) -> Result<Value, String> {
    run_json_async(
        app,
        vec!["processors".into(), "--json".into()],
        JSON_COMMAND_TIMEOUT,
        "processors",
    )
    .await
}

#[tauri::command]
async fn doctor_audio(app: tauri::AppHandle) -> Result<Value, String> {
    run_json_async(
        app,
        vec![
            "doctor".into(),
            "audio".into(),
            "--json".into(),
            "--fast-devices".into(),
        ],
        JSON_COMMAND_TIMEOUT,
        "doctor audio",
    )
    .await
}

/// 用户点击「请求系统音频权限」时调用:跑一次极短 Process Tap probe 触发 macOS 授权弹窗,
/// 回传 system_audio_permission + system_audio_permission_probe。普通 doctor 不会触发弹窗。
#[tauri::command]
async fn request_system_audio(app: tauri::AppHandle) -> Result<Value, String> {
    run_json_async(
        app,
        vec![
            "doctor".into(),
            "audio".into(),
            "--fast-devices".into(),
            "--request-system-audio".into(),
            "--json".into(),
        ],
        JSON_COMMAND_TIMEOUT,
        "request system audio",
    )
    .await
}

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
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                let _ = app_ev.emit("echoless://probe-progress", v);
            }
            let mut tail = tail_writer.lock().unwrap();
            tail.push_str(&line);
            tail.push('\n');
            if tail.len() > 4096 {
                let cut = tail.len() - 4096;
                tail.drain(..cut);
            }
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
/// 当前后端只支持 macOS Process Tap;其它平台 CLI 会非 0 退出,错误经 stderr 透传给前端。
#[tauri::command]
async fn probe_delay(
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

// ---- LocalVQE model/native management: brand data root + HF downloads ----
// revision 跟 main:完整性由每文件 sha256 pin 保证,新上传的文件无需改代码即可下载。
// (曾 pin 具体 commit,但该 rev 在 HF 上不存在导致下载全挂。)
const LOCALVQE_HF_REVISION: &str = "main";

#[derive(Clone, Copy)]
struct LocalVqeModelPin {
    filename: &'static str,
    sha256: &'static str,
    size: u64,
}

const LOCALVQE_MODEL_PINS: &[LocalVqeModelPin] = &[
    LocalVqeModelPin {
        filename: "localvqe-v1-1.3M-f32.gguf",
        sha256: "d5eaf577449d0f920d8ee5e1042b8ddc7b6627313a042c62e2ada1b42719ab30",
        size: 5_162_720,
    },
    LocalVqeModelPin {
        filename: "localvqe-v1.2-1.3M-f32.gguf",
        sha256: "4856ecf5f522b23fb2bc5caeac81f323c0ef1c4c156a9c7d40a6adbe092ba9ce",
        size: 5_173_088,
    },
    LocalVqeModelPin {
        filename: "localvqe-v1.3-4.8M-f32.gguf",
        sha256: "c4f7912485c32cfc206c536f2f050b52513f2f613fdbc616391f6b26ab1d51ec",
        size: 19_268_160,
    },
    LocalVqeModelPin {
        filename: "localvqe-v1.4-aec-200K-f32.gguf",
        sha256: "b6e43138588a83bfe903ab5e143b4020b91c1e1629f5a575ac5855ff0003c731",
        size: 2_924_224,
    },
];

fn localvqe_model_pin(filename: &str) -> Option<&'static LocalVqeModelPin> {
    LOCALVQE_MODEL_PINS
        .iter()
        .find(|pin| pin.filename == filename)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("打开文件失败: {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("读取文件失败: {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn verify_pinned_file(
    path: &Path,
    expected_sha256: &str,
    expected_size: u64,
    label: &str,
) -> Result<(), String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("读取文件信息失败: {}: {e}", path.display()))?
        .len();
    if size != expected_size {
        return Err(format!(
            "{label}大小不匹配: file={}, actual={}, expected={}",
            path.display(),
            size,
            expected_size
        ));
    }
    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(format!(
            "{label} SHA256 不匹配: file={}, actual={}, expected={}",
            path.display(),
            actual,
            expected_sha256
        ));
    }
    Ok(())
}

fn verify_localvqe_model_file(path: &Path, pin: &LocalVqeModelPin) -> Result<(), String> {
    verify_pinned_file(path, pin.sha256, pin.size, "LocalVQE 模型")
}

fn localvqe_data_dir_path() -> PathBuf {
    let (base, _) = echoless_paths::brand_data_root();
    base.join("localvqe")
}

fn localvqe_models_dir_path() -> PathBuf {
    localvqe_data_dir_path().join("models")
}

fn localvqe_native_dir_path() -> PathBuf {
    localvqe_data_dir_path().join("native")
}

fn localvqe_native_dir() -> Result<PathBuf, String> {
    let dir = localvqe_native_dir_path();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Local directory for downloaded models: <brand data root>/localvqe/models.
fn localvqe_models_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = localvqe_models_dir_path();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    migrate_legacy_localvqe_models(app, &dir);
    // User-supplied and in-app downloaded .gguf files both live here.
    let readme = dir.join("README.txt");
    if !readme.exists() {
        let _ = std::fs::write(
            &readme,
            "LocalVQE models\n\
             ===============\n\n\
             Put LocalVQE .gguf models in this folder. Models downloaded from\n\
             within Echoless also land here. Any .gguf found here is detected\n\
             automatically and can be selected on the Engine page.\n\n\
             Official models: https://huggingface.co/LocalAI-io/LocalVQE\n",
        );
    }
    Ok(dir)
}

fn migrate_legacy_localvqe_models(app: &tauri::AppHandle, dest_dir: &Path) {
    let Ok(legacy_base) = app.path().app_local_data_dir() else {
        return;
    };
    let legacy_dir = legacy_base.join("localvqe").join("models");
    if legacy_dir == dest_dir || !legacy_dir.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&legacy_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("gguf") {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        let dest = dest_dir.join(name);
        if dest.exists() {
            continue;
        }
        if let Err(rename_err) = std::fs::rename(&path, &dest) {
            if let Err(copy_err) =
                std::fs::copy(&path, &dest).and_then(|_| std::fs::remove_file(&path))
            {
                eprintln!(
                    "LocalVQE legacy model migration skipped: {} -> {}: rename={rename_err}; copy={copy_err}",
                    path.display(),
                    dest.display()
                );
            }
        }
    }
}

fn collect_gguf(dir: &Path) -> Vec<Value> {
    let mut models = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("gguf") {
                continue;
            }
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                models.push(serde_json::json!({
                    "filename": name,
                    "path": p.to_string_lossy(),
                    "source": "downloaded",
                }));
            }
        }
    }
    models.sort_by(|a, b| {
        a["filename"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["filename"].as_str().unwrap_or_default())
    });
    models
}

fn collect_native_files(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    files.push(name.to_string());
                }
            }
        }
    }
    files.sort();
    files
}

/// List available LocalVQE models from the single local model directory.
#[tauri::command]
fn localvqe_assets(app: tauri::AppHandle) -> Result<Value, String> {
    let dir = localvqe_models_dir(&app)?;
    let models = collect_gguf(&dir);
    let native_dir = localvqe_native_dir()?;
    let cli = echoless_bin(Some(&app)).ok();
    let library = cli
        .as_deref()
        .and_then(|path| localvqe_library_path(Some(&app), path));
    let native_files = collect_native_files(&native_dir);
    let process_tap_helper = cli
        .as_deref()
        .and_then(|path| process_tap_helper_bin(Some(&app), path));
    Ok(serde_json::json!({
        "models_dir": dir.to_string_lossy(),
        "models": models,
        "native_ready": library.is_some(),
        "library_path": library.map(|p| p.to_string_lossy().to_string()),
        "native_dir": native_dir.to_string_lossy(),
        "native_files": native_files,
        "cli_path": cli.map(|p| p.to_string_lossy().to_string()),
        "process_tap_helper_path": process_tap_helper.map(|p| p.to_string_lossy().to_string()),
    }))
}

/// 从官方 HF repo 下载指定模型到本地目录,回传完整路径。用 curl(免新增依赖)。
#[tauri::command]
async fn download_localvqe_model(
    app: tauri::AppHandle,
    filename: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || download_localvqe_model_blocking(&app, &filename))
        .await
        .map_err(|e| format!("download LocalVQE model task join failed: {e}"))?
}

fn download_localvqe_model_blocking(
    app: &tauri::AppHandle,
    filename: &str,
) -> Result<String, String> {
    let pin =
        localvqe_model_pin(filename).ok_or_else(|| "unsupported LocalVQE model".to_string())?;
    let dir = localvqe_models_dir(app)?;
    let dest = dir.join(pin.filename);
    if dest.exists() {
        match verify_localvqe_model_file(&dest, pin) {
            Ok(()) => return Ok(dest.to_string_lossy().to_string()),
            Err(_) => {
                let _ = std::fs::remove_file(&dest);
            }
        }
    }

    let tmp = dir.join(format!("{}.part", pin.filename));
    let _ = std::fs::remove_file(&tmp);
    let url = format!(
        "https://huggingface.co/LocalAI-io/LocalVQE/resolve/{LOCALVQE_HF_REVISION}/{}",
        pin.filename
    );
    let mut curl = Command::new("curl");
    // -sS:去掉进度表(否则 curl 把整张进度表写进 stderr,报错时被原样灌进 UI)。
    curl.args(["-sSfL", "--retry", "2", "-o"])
        .arg(&tmp)
        .arg(&url);
    let out =
        command_output_with_timeout(&mut curl, MODEL_DOWNLOAD_TIMEOUT, "LocalVQE model download")?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "下载失败({url}): {}",
            command_status_error("curl", &out)
        ));
    }
    if let Err(err) = verify_localvqe_model_file(&tmp, pin) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().to_string())
}

/// NVIDIA AFX / RTX AEC 引擎就绪探针。
/// 返回 { ok, report: { runtime_dir, runtime_dir_source, gpus[], selected_arch, checks[] } }。
/// macOS/Linux 上后端会返回 ok=false + platform unsupported 检查项(诚实降级)。
#[tauri::command]
async fn nvafx_doctor(app: tauri::AppHandle, runtime_dir: Option<String>) -> Result<Value, String> {
    let mut args: Vec<String> = vec!["nvafx".into(), "doctor".into(), "--json".into()];
    if let Some(dir) = runtime_dir {
        if !dir.is_empty() {
            args.push("--runtime-dir".into());
            args.push(dir);
        }
    }
    run_json_async(app, args, JSON_COMMAND_TIMEOUT, "nvafx doctor").await
}

/// NVAFX runtime 安装:校验+解压 common zip 与按架构选的 model zip,然后回传安装后的 doctor 报告。
/// 实际只在 Windows 生效(CLI `nvafx install` 在非 Windows 会 bail);mac/Linux 上返回 Err。
#[tauri::command]
async fn nvafx_install(
    app: tauri::AppHandle,
    common_zip: String,
    model_zip: String,
    runtime_dir: Option<String>,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let rdir = runtime_dir.filter(|d| !d.is_empty());
        let mut args: Vec<String> = vec![
            "nvafx".into(),
            "install".into(),
            "--common-zip".into(),
            common_zip,
            "--model-zip".into(),
            model_zip,
        ];
        if let Some(dir) = rdir.as_deref() {
            args.push("--runtime-dir".into());
            args.push(dir.to_string());
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut command = echoless_command(Some(&app))?;
        command.args(&arg_refs);
        let out =
            command_output_with_timeout(&mut command, NVAFX_INSTALL_TIMEOUT, "nvafx install")?;
        if !out.status.success() {
            return Err(command_status_error("nvafx install", &out));
        }

        // 安装后用 doctor --json 验证,回传报告供前端重算状态。
        let mut dargs: Vec<String> = vec!["nvafx".into(), "doctor".into(), "--json".into()];
        if let Some(dir) = rdir.as_deref() {
            dargs.push("--runtime-dir".into());
            dargs.push(dir.to_string());
        }
        let darg_refs: Vec<&str> = dargs.iter().map(String::as_str).collect();
        run_json_blocking(Some(&app), &darg_refs, JSON_COMMAND_TIMEOUT, "nvafx doctor")
    })
    .await
    .map_err(|e| format!("nvafx install task join failed: {e}"))?
}

/// 从公共 GitHub release 下载 common+架构 model zip,然后安装并回传 doctor。
/// shell `echoless nvafx download-install [--runtime-dir D] --json`;该子命令需打印
/// `{ok, report}` doctor JSON 到 stdout。后端(Codex)实现该子命令后此处即生效;
/// 未实现前 CLI 会非 0 退出,错误经 stderr 透传给前端。
#[tauri::command]
async fn nvafx_download_install(
    app: tauri::AppHandle,
    runtime_dir: Option<String>,
) -> Result<Value, String> {
    let rdir = runtime_dir.filter(|d| !d.is_empty());
    let mut args: Vec<String> = vec!["nvafx".into(), "download-install".into(), "--json".into()];
    if let Some(dir) = rdir {
        args.push("--runtime-dir".into());
        args.push(dir);
    }
    run_json_async(app, args, NVAFX_INSTALL_TIMEOUT, "nvafx download-install").await
}

/// 在系统默认浏览器打开外部链接(驱动 / VC++ 下载页)。
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    let url = validate_browser_url(&url)?;
    let (prog, args) = browser_open_command(&url);
    Command::new(prog)
        .args(&args)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn validate_browser_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("URL 不能为空".to_string());
    }
    if trimmed
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err("URL 不能包含空白或控制字符".to_string());
    }
    // 系统设置深链(隐私面板跳转):固定 scheme 白名单。此前被 http(s) 门拒掉,
    // 「授予系统音频权限」按钮点了毫无反应(2026-07-05 修)。
    if trimmed.starts_with("x-apple.systempreferences:") {
        return Ok(trimmed.to_string());
    }
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err("仅允许打开 http(s) URL".to_string());
    }

    let host_start = trimmed
        .find("://")
        .map(|idx| idx + 3)
        .unwrap_or(trimmed.len());
    let host_end = trimmed[host_start..]
        .find(['/', '?', '#'])
        .map(|idx| host_start + idx)
        .unwrap_or(trimmed.len());
    if host_start == host_end {
        return Err("URL 缺少主机名".to_string());
    }

    Ok(trimmed.to_string())
}

fn browser_open_command(url: &str) -> (&'static str, Vec<String>) {
    #[cfg(target_os = "macos")]
    return ("open", vec![url.to_string()]);
    #[cfg(target_os = "windows")]
    return (
        "rundll32.exe",
        vec!["url.dll,FileProtocolHandler".to_string(), url.to_string()],
    );
    #[cfg(target_os = "linux")]
    return ("xdg-open", vec![url.to_string()]);
}

/// 诊断录制默认目录(绝对路径,session-* 会写在其下)。
#[tauri::command]
fn default_diag_dir() -> String {
    std::env::temp_dir()
        .join("echoless-diagnostics")
        .to_string_lossy()
        .to_string()
}

/// 在系统文件管理器里打开目录(不存在则先创建)。
#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    let prog = "open";
    #[cfg(target_os = "windows")]
    let prog = "explorer";
    #[cfg(target_os = "linux")]
    let prog = "xdg-open";
    Command::new(prog)
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn validate_config(app: tauri::AppHandle, toml_text: String) -> Result<Value, String> {
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
fn start_run(
    app: tauri::AppHandle,
    state: State<RunState>,
    toml_text: String,
    stats_interval_ms: Option<u32>,
) -> Result<(), String> {
    let mut guard = run_state_guard(&state);
    // 幂等启动:若有残留子进程(并发重启 / 上次崩溃遗留),先标记 intentional 再杀掉,
    // 避免 "already running" 卡死。其 reader 退出会被判定为 intentional,不报崩溃。
    if let Some(mut prev) = guard.take() {
        prev.stopping.store(true, Ordering::SeqCst);
        let _ = prev.child.kill();
        let _ = prev.child.wait();
        cleanup_run_config(&prev.config_path);
    }
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
            return Err(format!("spawn echoless run failed: {err}"));
        }
    };

    // 本子进程专属的 stopping flag:被主动停/重启时置 true。
    let stopping = Arc::new(AtomicBool::new(false));

    // stdout = JSONL status events
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            cleanup_run_config(&path);
            return Err("no stdout".to_string());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            cleanup_run_config(&path);
            return Err("no stderr".to_string());
        }
    };
    let app_out = app.clone();
    let stop_reader = stopping.clone();
    let reader_config_path = path.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                        let _ = app_out.emit("echoless://status", v);
                    }
                }
                Err(err) => {
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
        let run_state = app_out.state::<RunState>();
        mark_run_exited(&run_state, &reader_config_path);
        update_tray_tooltip(&app_out, false);
        let _ = app_out.emit(
            "echoless://exit",
            serde_json::json!({ "intentional": intentional }),
        );
    });

    // stderr = 人类日志
    let app_err = app.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(line) => {
                    let _ = app_err.emit("echoless://log", line);
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
fn send_run_control(state: State<RunState>, line: String) -> Result<(), String> {
    write_run_control_line(&state, &line)
}

fn write_run_control_line(state: &RunState, line: &str) -> Result<(), String> {
    let mut guard = run_state_guard(state);
    let rc = guard.as_mut().ok_or("not running")?;
    let stdin = rc.child.stdin.as_mut().ok_or("no stdin")?;
    stdin
        .write_all(line.as_bytes())
        .map_err(|e| e.to_string())?;
    stdin.write_all(b"\n").map_err(|e| e.to_string())?;
    stdin.flush().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn set_bypass(state: State<RunState>, enabled: bool) -> Result<(), String> {
    let line = bypass_control_line(enabled);
    write_run_control_line(&state, &line)
}

fn bypass_control_line(enabled: bool) -> String {
    json!({
        "cmd": "set_bypass",
        "enabled": enabled,
    })
    .to_string()
}

#[tauri::command]
fn stop_run(app: tauri::AppHandle, state: State<RunState>) -> Result<(), String> {
    terminate_run(&state);
    update_tray_tooltip(&app, false);
    Ok(())
}

#[tauri::command]
fn set_tray_prefs(prefs: State<TrayPrefs>, close_to_tray: bool) {
    set_tray_prefs_inner(&prefs, close_to_tray);
}

// macOS 设备热插拔监听:CoreAudio 设备列表('dev#')变更即推事件给前端刷新。
// WKWebView 不触发 navigator.mediaDevices 的 devicechange,只能原生侧监听;
// Windows 的 WebView2(Chromium)会触发,前端已挂 devicechange,无需原生监听。
#[cfg(target_os = "macos")]
mod device_watch {
    use std::ffi::c_void;
    use tauri::Emitter;

    // CoreAudio/AudioHardware.h
    #[repr(C)]
    struct AudioObjectPropertyAddress {
        selector: u32,
        scope: u32,
        element: u32,
    }

    const SYSTEM_OBJECT: u32 = 1; // kAudioObjectSystemObject
    const DEVICES_ADDRESS: AudioObjectPropertyAddress = AudioObjectPropertyAddress {
        selector: u32::from_be_bytes(*b"dev#"), // kAudioHardwarePropertyDevices
        scope: u32::from_be_bytes(*b"glob"),    // kAudioObjectPropertyScopeGlobal
        element: 0,                             // kAudioObjectPropertyElementMain
    };

    type Listener = extern "C" fn(u32, u32, *const AudioObjectPropertyAddress, *mut c_void) -> i32;

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectAddPropertyListener(
            object_id: u32,
            address: *const AudioObjectPropertyAddress,
            listener: Listener,
            client_data: *mut c_void,
        ) -> i32;
    }

    // HAL 通知线程回调:只透传「变了」,枚举仍由前端调 list_devices 完成。
    extern "C" fn on_devices_changed(
        _object_id: u32,
        _num_addresses: u32,
        _addresses: *const AudioObjectPropertyAddress,
        client_data: *mut c_void,
    ) -> i32 {
        let app = unsafe { &*(client_data as *const tauri::AppHandle) };
        let _ = app.emit("echoless://devices-changed", ());
        0
    }

    pub fn start(app: &tauri::AppHandle) {
        // AppHandle 有意泄漏成 'static:监听与进程同生命周期,不注销。
        let client = Box::into_raw(Box::new(app.clone()));
        let status = unsafe {
            AudioObjectAddPropertyListener(
                SYSTEM_OBJECT,
                &DEVICES_ADDRESS,
                on_devices_changed,
                client as *mut c_void,
            )
        };
        if status != 0 {
            eprintln!("device watch: AudioObjectAddPropertyListener failed ({status})");
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_decorum::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(RunState(Mutex::new(None)))
        .manage(TrayPrefs::default());

    #[cfg(target_os = "windows")]
    let builder = builder.manage(TrayIconState(Mutex::new(None)));

    builder
        .invoke_handler(tauri::generate_handler![
            get_platform,
            list_devices,
            list_processors,
            doctor_audio,
            request_system_audio,
            probe_delay,
            localvqe_assets,
            download_localvqe_model,
            nvafx_doctor,
            nvafx_install,
            nvafx_download_install,
            open_url,
            default_diag_dir,
            open_path,
            validate_config,
            start_run,
            send_run_control,
            set_bypass,
            stop_run,
            set_tray_prefs
        ])
        .setup(|app| {
            // 默认打开基线 1040×640(v17 设计稿画布,布局按此定稿);
            // B1:min 锁到默认尺寸 —— plate 分格在更小窗口必然破版。
            // B3:builder 背景色 = 新色板 --bg #1d1d1b,resize 瞬间不露白边。
            let mut builder =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("Echoless")
                    .inner_size(1040.0, 640.0)
                    .min_inner_size(1040.0, 640.0)
                    .max_inner_size(1600.0, 1100.0)
                    .background_color(tauri::window::Color(0x1d, 0x1d, 0x1b, 0xff))
                    .resizable(true)
                    .visible(true);

            // 平台镜像标题栏(见 Design.md §5.1):
            //   macOS  → Overlay + 隐藏标题,保留系统红绿灯(OS 绘制,左上)
            //   其它   → 去原生装饰,自绘 caption 按钮(右上),恢复阴影/圆角
            #[cfg(target_os = "macos")]
            {
                builder = builder
                    .title_bar_style(tauri::TitleBarStyle::Overlay)
                    .hidden_title(true);
            }
            #[cfg(not(target_os = "macos"))]
            {
                builder = builder.decorations(false).shadow(true);
            }

            let window = builder.build()?;

            // macOS:把系统红绿灯垂直居中到 40px 标题栏,并与左侧内容对齐。
            #[cfg(target_os = "macos")]
            {
                let _ = window.set_traffic_lights_inset(16.0, 13.0);
            }
            #[cfg(target_os = "windows")]
            {
                register_windows_tray(app)?;
            }
            #[cfg(target_os = "macos")]
            device_watch::start(app.handle());
            let _ = &window;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let prefs = window.state::<TrayPrefs>();
                if close_to_tray_enabled(&prefs) {
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    let state = window.state::<RunState>();
                    terminate_run(&state);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
        let state = RunState(Mutex::new(None));
        let _ = std::panic::catch_unwind(|| {
            let _guard = state.0.lock().expect("test lock should start healthy");
            panic!("poison run state");
        });

        assert!(state.0.is_poisoned());
        let guard = run_state_guard(&state);
        assert!(guard.is_none());
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
    fn terminate_run_marks_stopping_waits_and_cleans_config() {
        let dir = unique_temp_dir("echoless-terminate-run");
        let config_path = dir.join("run.toml");
        std::fs::write(&config_path, "stub = true").unwrap();
        let stopping = Arc::new(AtomicBool::new(false));
        let child = slow_child_command().spawn().unwrap();
        let state = RunState(Mutex::new(Some(RunChild {
            child,
            stopping: stopping.clone(),
            config_path: config_path.clone(),
        })));

        terminate_run(&state);

        assert!(stopping.load(Ordering::SeqCst));
        assert!(run_state_guard(&state).is_none());
        assert!(!config_path.exists());

        let _ = std::fs::remove_dir_all(dir);
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

        assert_eq!(
            find_localvqe_library_in_dir(&dir).as_deref(),
            Some(expected.as_path())
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn validates_only_plain_http_browser_urls() {
        assert_eq!(
            validate_browser_url(" https://example.com/download?x=1 ").unwrap(),
            "https://example.com/download?x=1"
        );
        assert_eq!(
            validate_browser_url("http://example.com/#drivers").unwrap(),
            "http://example.com/#drivers"
        );
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
        ] {
            assert!(validate_browser_url(bad).is_err(), "{bad}");
        }
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
        assert!(err.contains("创建配置文件失败"), "{err}");
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
        assert!(err.contains("SHA256 不匹配"), "{err}");

        let wrong_size = LocalVqeModelPin { size: 4, ..good };
        let err = verify_localvqe_model_file(&path, &wrong_size)
            .unwrap_err()
            .to_string();
        assert!(err.contains("大小不匹配"), "{err}");

        let _ = std::fs::remove_dir_all(dir);
    }
}
