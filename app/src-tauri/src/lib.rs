// Echoless GUI 的 Tauri 后端:
//   - 平台探测(标题栏镜像)
//   - 把 `echoless` CLI 作为 sidecar 调用,只消费 JSON / JSONL 契约
//   - run 的 --status-json 以 JSONL 流式解析,经事件推给前端
//
// 契约真理源:echoless/docs/frontend/*.md + CLI 实测。
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tauri::{
    Emitter, Manager, State, TitleBarStyle, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
#[cfg(target_os = "macos")]
use tauri_plugin_decorum::WebviewWindowExt;

/// 运行中的 echoless run 子进程 + 它专属的「正在被主动停止」标记。
/// 每个子进程独立持有 stopping flag,其 stdout reader 退出时据此判断本次退出是
/// 主动停/重启(intentional)还是子进程自己崩了(crash),供前端区分。
struct RunChild {
    child: Child,
    stopping: Arc<AtomicBool>,
}
/// 当前运行中的 echoless run 子进程(同一时刻最多一个)。
struct RunState(Mutex<Option<RunChild>>);

const TAURI_TARGET_TRIPLE: &str = env!("TAURI_ENV_TARGET_TRIPLE");

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

/// 跑一次性 JSON 子命令(devices / processors / config validate),返回解析后的 JSON。
fn run_json(app: Option<&tauri::AppHandle>, args: &[&str]) -> Result<Value, String> {
    let out = echoless_command(app)?
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
fn list_devices(app: tauri::AppHandle) -> Result<Value, String> {
    run_json(Some(&app), &["devices", "--json"])
}

#[tauri::command]
fn list_processors(app: tauri::AppHandle) -> Result<Value, String> {
    run_json(Some(&app), &["processors", "--json"])
}

#[tauri::command]
fn doctor_audio(app: tauri::AppHandle) -> Result<Value, String> {
    run_json(Some(&app), &["doctor", "audio", "--json"])
}

/// 用户点击「请求系统音频权限」时调用:跑一次极短 Process Tap probe 触发 macOS 授权弹窗,
/// 回传 system_audio_permission + system_audio_permission_probe。普通 doctor 不会触发弹窗。
#[tauri::command]
fn request_system_audio(app: tauri::AppHandle) -> Result<Value, String> {
    run_json(
        Some(&app),
        &["doctor", "audio", "--request-system-audio", "--json"],
    )
}

/// 主动近端延迟侦测 / AEC 链路诊断。shell `echoless probe-delay --json`:播放一串蜂鸣、
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
        let out = echoless_command(Some(&app))?
            .args(&arg_refs)
            .output()
            .map_err(|e| format!("spawn echoless failed: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("probe-delay 失败: {err}"));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        serde_json::from_str::<Value>(&stdout)
            .map_err(|e| format!("parse json failed: {e}; raw: {stdout}"))
    })
    .await
    .map_err(|e| format!("probe task join failed: {e}"))?
}

// ---- LocalVQE 模型管理(打包默认模型 + 从官方 HF repo 下载选择) ----
const LOCALVQE_HF_BASE: &str = "https://huggingface.co/LocalAI-io/LocalVQE/resolve/main/";

/// 下载模型的本地目录:<app_local_data>/localvqe/models(自动创建)。
fn localvqe_models_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let base = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    let dir = base.join("localvqe").join("models");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // 目录说明:用户手动放入的 .gguf 与应用内下载的模型都落在这里,引擎页自动检测。
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

fn collect_gguf(dir: &Path, source: &str, out: &mut Vec<Value>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("gguf") {
                continue;
            }
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if out.iter().any(|m| m["filename"] == name) {
                    continue; // 下载目录优先,避免与打包资源重名重复
                }
                out.push(serde_json::json!({
                    "filename": name,
                    "path": p.to_string_lossy(),
                    "source": source,
                }));
            }
        }
    }
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

/// 列出可用 LocalVQE 模型(下载目录 + 打包资源里的 .gguf),供引擎页选择。
#[tauri::command]
fn localvqe_assets(app: tauri::AppHandle) -> Result<Value, String> {
    let dir = localvqe_models_dir(&app)?;
    let mut models: Vec<Value> = vec![];
    collect_gguf(&dir, "downloaded", &mut models);
    if let Ok(res) = app.path().resolve(
        "resources/localvqe/models",
        tauri::path::BaseDirectory::Resource,
    ) {
        collect_gguf(&res, "bundled", &mut models);
    }
    let cli = echoless_bin(Some(&app)).ok();
    let library = cli
        .as_deref()
        .and_then(|path| localvqe_library_path(Some(&app), path));
    let native_dir = library
        .as_ref()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let native_files = native_dir
        .as_deref()
        .map(collect_native_files)
        .unwrap_or_default();
    let process_tap_helper = cli
        .as_deref()
        .and_then(|path| process_tap_helper_bin(Some(&app), path));
    Ok(serde_json::json!({
        "models_dir": dir.to_string_lossy(),
        "models": models,
        "native_ready": library.is_some(),
        "library_path": library.map(|p| p.to_string_lossy().to_string()),
        "native_dir": native_dir.map(|p| p.to_string_lossy().to_string()),
        "native_files": native_files,
        "cli_path": cli.map(|p| p.to_string_lossy().to_string()),
        "process_tap_helper_path": process_tap_helper.map(|p| p.to_string_lossy().to_string()),
    }))
}

/// 从官方 HF repo 下载指定模型到本地目录,回传完整路径。用 curl(免新增依赖)。
#[tauri::command]
fn download_localvqe_model(app: tauri::AppHandle, filename: String) -> Result<String, String> {
    // 限定已知模型名,防止任意写入 / 路径穿越。
    if !filename.ends_with(".gguf") || filename.contains('/') || filename.contains("..") {
        return Err("bad filename".into());
    }
    let dir = localvqe_models_dir(&app)?;
    let dest = dir.join(&filename);
    let tmp = dir.join(format!("{filename}.part"));
    let url = format!("{LOCALVQE_HF_BASE}{filename}");
    let status = Command::new("curl")
        .args(["-fL", "--retry", "2", "-o"])
        .arg(&tmp)
        .arg(&url)
        .status()
        .map_err(|e| format!("curl 启动失败: {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("下载失败({url})"));
    }
    std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().to_string())
}

/// NVIDIA AFX / RTX AEC 引擎就绪探针。
/// 返回 { ok, report: { runtime_dir, runtime_dir_source, gpus[], selected_arch, checks[] } }。
/// macOS/Linux 上后端会返回 ok=false + platform unsupported 检查项(诚实降级)。
#[tauri::command]
fn nvafx_doctor(app: tauri::AppHandle, runtime_dir: Option<String>) -> Result<Value, String> {
    let mut args: Vec<&str> = vec!["nvafx", "doctor", "--json"];
    if let Some(dir) = runtime_dir.as_deref() {
        if !dir.is_empty() {
            args.push("--runtime-dir");
            args.push(dir);
        }
    }
    run_json(Some(&app), &args)
}

/// NVAFX runtime 安装:校验+解压 common zip 与按架构选的 model zip,然后回传安装后的 doctor 报告。
/// 实际只在 Windows 生效(CLI `nvafx install` 在非 Windows 会 bail);mac/Linux 上返回 Err。
#[tauri::command]
fn nvafx_install(
    app: tauri::AppHandle,
    common_zip: String,
    model_zip: String,
    runtime_dir: Option<String>,
) -> Result<Value, String> {
    let rdir = runtime_dir.filter(|d| !d.is_empty());
    let mut args: Vec<&str> = vec![
        "nvafx",
        "install",
        "--common-zip",
        &common_zip,
        "--model-zip",
        &model_zip,
    ];
    if let Some(dir) = rdir.as_deref() {
        args.push("--runtime-dir");
        args.push(dir);
    }
    let out = echoless_command(Some(&app))?
        .args(&args)
        .output()
        .map_err(|e| format!("spawn echoless failed: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("nvafx install 失败: {err}"));
    }
    // 安装后用 doctor --json 验证,回传报告供前端重算状态。
    let mut dargs: Vec<&str> = vec!["nvafx", "doctor", "--json"];
    if let Some(dir) = rdir.as_deref() {
        dargs.push("--runtime-dir");
        dargs.push(dir);
    }
    run_json(Some(&app), &dargs)
}

/// 从公共 GitHub release 下载 common+架构 model zip,然后安装并回传 doctor。
/// shell `echoless nvafx download-install [--runtime-dir D] --json`;该子命令需打印
/// `{ok, report}` doctor JSON 到 stdout。后端(Codex)实现该子命令后此处即生效;
/// 未实现前 CLI 会非 0 退出,错误经 stderr 透传给前端。
#[tauri::command]
fn nvafx_download_install(
    app: tauri::AppHandle,
    runtime_dir: Option<String>,
) -> Result<Value, String> {
    let rdir = runtime_dir.filter(|d| !d.is_empty());
    let mut args: Vec<&str> = vec!["nvafx", "download-install", "--json"];
    if let Some(dir) = rdir.as_deref() {
        args.push("--runtime-dir");
        args.push(dir);
    }
    let out = echoless_command(Some(&app))?
        .args(&args)
        .output()
        .map_err(|e| format!("spawn echoless failed: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("nvafx download-install 失败: {err}"));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).map_err(|e| format!("parse json failed: {e}; raw: {stdout}"))
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
fn validate_config(app: tauri::AppHandle, toml_text: String) -> Result<Value, String> {
    let path = std::env::temp_dir().join("echoless-validate.toml");
    std::fs::write(&path, toml_text).map_err(|e| e.to_string())?;
    run_json(
        Some(&app),
        &[
            "config",
            "validate",
            "--config",
            path.to_str().ok_or("bad temp path")?,
            "--json",
        ],
    )
}

#[tauri::command]
fn start_run(
    app: tauri::AppHandle,
    state: State<RunState>,
    toml_text: String,
    stats_interval_ms: Option<u32>,
) -> Result<(), String> {
    let mut guard = state.0.lock().unwrap();
    // 幂等启动:若有残留子进程(并发重启 / 上次崩溃遗留),先标记 intentional 再杀掉,
    // 避免 "already running" 卡死。其 reader 退出会被判定为 intentional,不报崩溃。
    if let Some(mut prev) = guard.take() {
        prev.stopping.store(true, Ordering::SeqCst);
        let _ = prev.child.kill();
        let _ = prev.child.wait();
    }
    let path = std::env::temp_dir().join("echoless-run.toml");
    std::fs::write(&path, toml_text).map_err(|e| e.to_string())?;
    let interval = stats_interval_ms.unwrap_or(80).to_string();

    let mut command = echoless_command(Some(&app))?;
    let mut child = command
        .args([
            "run",
            "--config",
            path.to_str().ok_or("bad temp path")?,
            "--status-json",
            "--stats-interval-ms",
            &interval,
        ])
        .stdin(Stdio::piped()) // 录制就地控制:start/stop_diagnostics 经 stdin JSONL 下发
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn echoless run failed: {e}"))?;

    // 本子进程专属的 stopping flag:被主动停/重启时置 true。
    let stopping = Arc::new(AtomicBool::new(false));

    // stdout = JSONL status events
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let app_out = app.clone();
    let stop_reader = stopping.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                let _ = app_out.emit("echoless://status", v);
            }
        }
        // 退出归因:intentional=主动停/重启(本 flag 已被置 true);否则=子进程自己退出(崩溃)。
        let intentional = stop_reader.load(Ordering::SeqCst);
        let _ = app_out.emit(
            "echoless://exit",
            serde_json::json!({ "intentional": intentional }),
        );
    });

    // stderr = 人类日志
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let app_err = app.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = app_err.emit("echoless://log", line);
        }
    });

    *guard = Some(RunChild { child, stopping });
    Ok(())
}

/// 向运行中的 echoless run 子进程 stdin 写一行 JSON 控制命令
/// (start_diagnostics / stop_diagnostics)。就地起停录制,不重启音频管线。
#[tauri::command]
fn send_run_control(state: State<RunState>, line: String) -> Result<(), String> {
    let mut guard = state.0.lock().unwrap();
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
fn stop_run(state: State<RunState>) -> Result<(), String> {
    let mut guard = state.0.lock().unwrap();
    if let Some(mut rc) = guard.take() {
        rc.stopping.store(true, Ordering::SeqCst); // 主动停 → 其 reader 退出判为 intentional
        let _ = rc.child.kill();
        let _ = rc.child.wait();
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_decorum::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(RunState(Mutex::new(None)))
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
            stop_run
        ])
        .setup(|app| {
            // 默认打开基线 1040×640(布局按此定稿);可缩放,设合理 min/max 防止过小/过大破版。
            let mut builder =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("Echoless")
                    .inner_size(1040.0, 640.0)
                    .min_inner_size(960.0, 600.0)
                    .max_inner_size(1600.0, 1100.0)
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
                if let Some(mut rc) = child_opt {
                    rc.stopping.store(true, Ordering::SeqCst);
                    let _ = rc.child.kill();
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
}
