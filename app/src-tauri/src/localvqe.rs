use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{Emitter, Manager};

use crate::bin_resolve::{echoless_bin, localvqe_library_path, process_tap_helper_bin};
use crate::proc::{command_output_with_timeout, command_status_error, MODEL_DOWNLOAD_TIMEOUT};

// ---- LocalVQE model/native management: brand data root + HF downloads ----
// revision 跟 main:完整性由每文件 sha256 pin 保证,新上传的文件无需改代码即可下载。
// (曾 pin 具体 commit,但该 rev 在 HF 上不存在导致下载全挂。)
const LOCALVQE_HF_REVISION: &str = "main";

#[derive(Clone, Copy)]
pub(crate) struct LocalVqeModelPin {
    pub(crate) filename: &'static str,
    pub(crate) sha256: &'static str,
    pub(crate) size: u64,
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

pub(crate) fn localvqe_model_pin(filename: &str) -> Option<&'static LocalVqeModelPin> {
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

pub(crate) fn verify_localvqe_model_file(
    path: &Path,
    pin: &LocalVqeModelPin,
) -> Result<(), String> {
    verify_pinned_file(path, pin.sha256, pin.size, "LocalVQE 模型")
}

fn localvqe_data_dir_path() -> PathBuf {
    let (base, _) = echoless_paths::brand_data_root();
    base.join("localvqe")
}

pub(crate) fn localvqe_models_dir_path() -> PathBuf {
    localvqe_data_dir_path().join("models")
}

pub(crate) fn localvqe_native_dir_path() -> PathBuf {
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
    // Always regenerate the README so the supported-file list / SHA-256 pins
    // track LOCALVQE_MODEL_PINS across app updates.
    let readme = dir.join("README.txt");
    let _ = std::fs::write(&readme, localvqe_models_readme());
    Ok(dir)
}

/// README.txt content for the models folder, listing every supported filename
/// and its pinned SHA-256 (downloads are verified against these).
fn localvqe_models_readme() -> String {
    let mut s = String::from(
        "LocalVQE models\n\
         ===============\n\n\
         Put LocalVQE .gguf models in this folder. Models downloaded from\n\
         within Echoless also land here. Any .gguf found here is detected\n\
         automatically and can be selected on the Engine page.\n\n\
         Official models: https://huggingface.co/LocalAI-io/LocalVQE\n",
    );
    s.push_str(&format!("Pinned revision: {LOCALVQE_HF_REVISION}\n\n"));
    s.push_str("Supported filenames (each download is verified against its SHA-256):\n\n");
    for pin in LOCALVQE_MODEL_PINS {
        s.push_str(&format!("  {}\n    sha256: {}\n", pin.filename, pin.sha256));
    }
    s
}

fn migrate_legacy_localvqe_models(app: &tauri::AppHandle, dest_dir: &Path) {
    let Ok(legacy_base) = app.path().app_local_data_dir() else {
        return;
    };
    migrate_legacy_localvqe_models_from_base(&legacy_base, dest_dir);
}

pub(crate) fn migrate_legacy_localvqe_models_from_base(legacy_base: &Path, dest_dir: &Path) {
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
pub(crate) fn localvqe_assets(app: tauri::AppHandle) -> Result<Value, String> {
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
pub(crate) async fn download_localvqe_model(
    app: tauri::AppHandle,
    filename: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || download_localvqe_model_blocking(&app, &filename))
        .await
        .map_err(|e| format!("download LocalVQE model task join failed: {e}"))?
}

// 同一文件的并发下载守卫:两个下载写同一个 {file}.part 会互相踩(Windows 上
// remove_file 对已打开的文件失败,-C - 续传会接到对方的半成品 → 大小/SHA 不匹配)。
// 同名下载进行中时直接拒绝,让前端提示「正在下载」而不是产出损坏文件。
static DOWNLOADS_IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn downloads_in_flight() -> &'static Mutex<HashSet<String>> {
    DOWNLOADS_IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 持有期间把 filename 记为「下载中」,drop 时(含 ? 提前返回)自动清除。
struct InFlightGuard(String);
impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = downloads_in_flight().lock() {
            set.remove(&self.0);
        }
    }
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

    // 抢占同名下载锁:已在下载则拒绝(见 DOWNLOADS_IN_FLIGHT 注释)。_inflight
    // 在函数返回时 drop、自动释放。
    {
        let mut inflight = downloads_in_flight()
            .lock()
            .map_err(|_| "download lock poisoned".to_string())?;
        if !inflight.insert(pin.filename.to_string()) {
            return Err(format!("模型 {} 正在下载中,请稍候", pin.filename));
        }
    }
    let _inflight = InFlightGuard(pin.filename.to_string());

    let tmp = dir.join(format!("{}.part", pin.filename));
    let _ = std::fs::remove_file(&tmp);
    let url = format!(
        "https://huggingface.co/LocalAI-io/LocalVQE/resolve/{LOCALVQE_HF_REVISION}/{}",
        pin.filename
    );
    let mut curl = Command::new("curl");
    // -sS:去掉进度表(否则 curl 把整张进度表写进 stderr,报错时被原样灌进 UI)。
    // 加固下载:
    //  --http1.1        HF CDN 偶发 HTTP/2 CANCEL(curl err 92),强制 1.1 规避。
    //  --retry-all-errors 默认只重试部分 HTTP 状态码;这个把传输层错误也纳入重试。
    //  --retry 5 / --retry-delay 2  比原来 2 次更耐抖动。
    //  -C -             断点续传:重试时从 .part 已下部分接着下,不从头再来。
    //  --connect-timeout 30  连不上时早点报错,别耗满 10 分钟总超时。
    curl.args([
        "-sSfL",
        "--http1.1",
        "--retry",
        "5",
        "--retry-delay",
        "2",
        "--retry-all-errors",
        "--connect-timeout",
        "30",
        "-C",
        "-",
        "-o",
    ])
    .arg(&tmp)
    .arg(&url);
    // 下载进度:另起 poller 线程轮询 .part 字节数 / pin.size,向前端发
    // `echoless://localvqe-progress`。curl 本身走已测的 command_output_with_timeout
    // 路径不动(console 抑制、超时、输出捕获照旧),进度只是旁路观测。
    let stop = Arc::new(AtomicBool::new(false));
    let poller = {
        let app = app.clone();
        let tmp = tmp.clone();
        let filename = pin.filename.to_string();
        let total = pin.size;
        let stop = stop.clone();
        std::thread::spawn(move || loop {
            let received = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
            // 校验/落盘前封顶 99%,完成态由前端在下载 resolve 后置 100。
            let pct = (received.min(total) * 100)
                .checked_div(total)
                .map(|p| (p as u32).min(99))
                .unwrap_or(0);
            let _ = app.emit(
                "echoless://localvqe-progress",
                serde_json::json!({
                    "filename": filename,
                    "pct": pct,
                    "received": received,
                    "total": total,
                }),
            );
            if stop.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        })
    };
    let out =
        command_output_with_timeout(&mut curl, MODEL_DOWNLOAD_TIMEOUT, "LocalVQE model download");
    stop.store(true, Ordering::Relaxed);
    let _ = poller.join();
    let out = out?;
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
