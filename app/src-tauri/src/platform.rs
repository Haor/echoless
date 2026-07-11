use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::Manager;

pub(crate) fn transient_config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
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

pub(crate) fn write_toml_create_new(path: &Path, toml_text: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("failed to create config file {}: {e}", path.display()))?;
    if let Err(err) = file.write_all(toml_text.as_bytes()) {
        drop(file);
        cleanup_run_config(path);
        return Err(format!(
            "failed to write config file {}: {err}",
            path.display()
        ));
    }
    if let Err(err) = file.flush() {
        drop(file);
        cleanup_run_config(path);
        return Err(format!(
            "failed to flush config file {}: {err}",
            path.display()
        ));
    }
    Ok(())
}

pub(crate) fn write_transient_config_toml(
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
    Err("failed to create a unique config file".to_string())
}

pub(crate) fn cleanup_run_config(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

/// 在系统默认浏览器打开外部链接(驱动 / VC++ 下载页)。
#[tauri::command]
pub(crate) fn open_url(url: String) -> Result<(), String> {
    let url = validate_browser_url(&url)?;
    let (prog, args) = browser_open_command(&url);
    Command::new(prog)
        .args(&args)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn validate_browser_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("URL must not be empty".to_string());
    }
    if trimmed
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err("URL must not contain whitespace or control characters".to_string());
    }
    // 系统设置深链只允许跳到隐私面板;不要把整个 scheme 当作通用白名单。
    // 放行两种:隐私根面板 `?Privacy`(系统录音无稳定锚点,只能回退根面板),
    // 以及带锚点的子面板 `?Privacy_Microphone` 等(`?Privacy_` 前缀)。
    if trimmed.starts_with("x-apple.systempreferences:") {
        const PRIVACY_ROOT: &str =
            "x-apple.systempreferences:com.apple.preference.security?Privacy";
        if trimmed == PRIVACY_ROOT || trimmed.starts_with(&format!("{PRIVACY_ROOT}_")) {
            return Ok(trimmed.to_string());
        }
        return Err("only the system privacy settings pane is allowed".to_string());
    }
    let parsed = tauri::Url::parse(trimmed).map_err(|_| "URL is not valid".to_string())?;
    if parsed.scheme() != "https" {
        return Err("only https URLs are allowed".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("URL credentials are not allowed".to_string());
    }
    if parsed.port().is_some_and(|port| port != 443) {
        return Err("only the default HTTPS port is allowed".to_string());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "URL is missing a host".to_string())?
        .trim_end_matches('.');
    if !is_allowed_browser_host(host) {
        return Err("URL host is not in the allow list".to_string());
    }

    Ok(parsed.into())
}

fn is_allowed_browser_host(host: &str) -> bool {
    const ALLOWED: &[&str] = &[
        "aka.ms",
        "developer.nvidia.com",
        "existential.audio",
        "github.com",
        "huggingface.co",
        "learn.microsoft.com",
        "nvidia.com",
        "vb-audio.com",
    ];
    ALLOWED
        .iter()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
}

pub(crate) fn browser_open_command(url: &str) -> (&'static str, Vec<String>) {
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
pub(crate) fn default_diag_dir() -> String {
    echoless_paths::diagnostics_dir()
        .to_string_lossy()
        .to_string()
}

/// 在系统文件管理器里打开固定诊断录制目录。
#[tauri::command]
pub(crate) fn open_diagnostics_dir() -> Result<(), String> {
    let path = ensure_diagnostics_dir()?;
    open_path(path.to_string_lossy().to_string())
}

pub(crate) fn ensure_diagnostics_dir() -> Result<PathBuf, String> {
    let path = echoless_paths::diagnostics_dir();
    std::fs::create_dir_all(&path).map_err(|e| {
        format!(
            "failed to create diagnostics directory {}: {e}",
            path.display()
        )
    })?;
    Ok(path)
}

/// 在系统文件管理器里打开目录。
#[tauri::command]
pub(crate) fn open_path(path: String) -> Result<(), String> {
    let p = validate_open_path(&path)?;
    #[cfg(target_os = "macos")]
    let prog = "open";
    #[cfg(target_os = "windows")]
    let prog = "explorer";
    #[cfg(target_os = "linux")]
    let prog = "xdg-open";
    Command::new(prog)
        .arg(&p)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn validate_open_path(path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    let canonical = p
        .canonicalize()
        .map_err(|e| format!("directory does not exist or is not accessible: {e}"))?;
    if !canonical.is_dir() {
        return Err("only directories can be opened".to_string());
    }
    let allowed_roots = allowed_open_path_roots();
    if allowed_roots
        .iter()
        .any(|root| canonical == *root || canonical.starts_with(root))
    {
        return Ok(canonical);
    }
    Err("directory is outside the Echoless allowed scope".to_string())
}

fn allowed_open_path_roots() -> Vec<PathBuf> {
    let (brand_root, _) = echoless_paths::brand_data_root();
    [
        brand_root.clone(),
        brand_root.join("diagnostics"),
        crate::localvqe::localvqe_models_dir_path(),
    ]
    .into_iter()
    .filter_map(|path| path.canonicalize().ok())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::validate_browser_url;

    #[test]
    fn privacy_root_and_anchored_subpanels_allowed() {
        // 系统录音无稳定锚点 → 回退隐私根面板;必须放行(S-01)。
        let root = "x-apple.systempreferences:com.apple.preference.security?Privacy";
        assert_eq!(validate_browser_url(root).as_deref(), Ok(root));
        // 麦克风等带锚点子面板照常放行。
        let mic = "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";
        assert_eq!(validate_browser_url(mic).as_deref(), Ok(mic));
    }

    #[test]
    fn non_privacy_settings_deeplinks_rejected() {
        // 非隐私面板的系统设置深链一律拒绝(不把整个 scheme 当白名单)。
        assert!(
            validate_browser_url("x-apple.systempreferences:com.apple.preference.network").is_err()
        );
        // `?Privacy` 之外的查询前缀也拒绝(防止 `?PrivacyEvil` 之类的伪前缀)。
        assert!(validate_browser_url(
            "x-apple.systempreferences:com.apple.preference.security?PrivacyEvil"
        )
        .is_err());
    }
}
