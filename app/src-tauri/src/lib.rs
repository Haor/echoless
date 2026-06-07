// Echoless GUI 的 Tauri 后端:
//   - 平台探测(标题栏镜像)
//   - 把 `echoless` CLI 作为 sidecar 调用,只消费 JSON / JSONL 契约
//   - run 的 --status-json 以 JSONL 流式解析,经事件推给前端
//
// 契约真理源:echoless/docs/frontend/*.md + CLI 实测。
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use serde_json::Value;
use tauri::{
    Emitter, Manager, State, TitleBarStyle, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
#[cfg(target_os = "macos")]
use tauri_plugin_decorum::WebviewWindowExt;

/// 当前运行中的 echoless run 子进程(同一时刻最多一个)。
struct RunState(Mutex<Option<Child>>);

/// 解析 echoless 二进制路径:
///   1. 环境变量 ECHOLESS_BIN(打包后由 sidecar 资源注入)
///   2. dev 回退:相对本 crate 的 ../../target/release/echoless
fn echoless_bin() -> PathBuf {
    if let Ok(p) = std::env::var("ECHOLESS_BIN") {
        return PathBuf::from(p);
    }
    let manifest = env!("CARGO_MANIFEST_DIR"); // .../echoless/app/src-tauri
    let mut p = PathBuf::from(manifest);
    p.push("../../target/release/echoless");
    p
}

/// 跑一次性 JSON 子命令(devices / processors / config validate),返回解析后的 JSON。
fn run_json(args: &[&str]) -> Result<Value, String> {
    let out = Command::new(echoless_bin())
        .args(args)
        .output()
        .map_err(|e| format!("spawn echoless failed: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).map_err(|e| format!("parse json failed: {e}; raw: {stdout}"))
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
fn list_devices() -> Result<Value, String> {
    run_json(&["devices", "--json"])
}

#[tauri::command]
fn list_processors() -> Result<Value, String> {
    run_json(&["processors", "--json"])
}

#[tauri::command]
fn doctor_audio() -> Result<Value, String> {
    run_json(&["doctor", "audio", "--json"])
}

#[tauri::command]
fn validate_config(toml_text: String) -> Result<Value, String> {
    let path = std::env::temp_dir().join("echoless-validate.toml");
    std::fs::write(&path, toml_text).map_err(|e| e.to_string())?;
    run_json(&[
        "config",
        "validate",
        "--config",
        path.to_str().ok_or("bad temp path")?,
        "--json",
    ])
}

#[tauri::command]
fn start_run(
    app: tauri::AppHandle,
    state: State<RunState>,
    toml_text: String,
    stats_interval_ms: Option<u32>,
) -> Result<(), String> {
    let mut guard = state.0.lock().unwrap();
    if guard.is_some() {
        return Err("already running".into());
    }
    let path = std::env::temp_dir().join("echoless-run.toml");
    std::fs::write(&path, toml_text).map_err(|e| e.to_string())?;
    let interval = stats_interval_ms.unwrap_or(80).to_string();

    let mut child = Command::new(echoless_bin())
        .args([
            "run",
            "--config",
            path.to_str().ok_or("bad temp path")?,
            "--status-json",
            "--stats-interval-ms",
            &interval,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn echoless run failed: {e}"))?;

    // stdout = JSONL status events
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let app_out = app.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().flatten() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                let _ = app_out.emit("echoless://status", v);
            }
        }
        let _ = app_out.emit("echoless://exit", ());
    });

    // stderr = 人类日志
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let app_err = app.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().flatten() {
            let _ = app_err.emit("echoless://log", line);
        }
    });

    *guard = Some(child);
    Ok(())
}

#[tauri::command]
fn stop_run(state: State<RunState>) -> Result<(), String> {
    let mut guard = state.0.lock().unwrap();
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_decorum::init())
        .manage(RunState(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            get_platform,
            list_devices,
            list_processors,
            doctor_audio,
            validate_config,
            start_run,
            stop_run
        ])
        .setup(|app| {
            let mut builder =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("Echoless")
                    .inner_size(980.0, 600.0)
                    .min_inner_size(900.0, 560.0)
                    .resizable(true)
                    .visible(true);

            // 平台镜像标题栏(见 Design.md §5.1):
            //   macOS  → Overlay + 隐藏标题,保留系统红绿灯(OS 绘制,左上)
            //   其它   → 去原生装饰,自绘 caption 按钮(右上),恢复阴影/圆角
            #[cfg(target_os = "macos")]
            {
                builder = builder
                    .title_bar_style(TitleBarStyle::Overlay)
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
            let _ = &window;
            Ok(())
        })
        .on_window_event(|window, event| {
            // 关窗时确保杀掉 echoless 子进程,避免遗留进程占用音频设备。
            if let WindowEvent::CloseRequested { .. } = event {
                let child_opt = {
                    let state = window.state::<RunState>();
                    let taken = state.0.lock().ok().and_then(|mut g| g.take());
                    taken
                };
                if let Some(mut child) = child_opt {
                    let _ = child.kill();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
