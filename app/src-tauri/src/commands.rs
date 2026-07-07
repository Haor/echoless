use serde_json::Value;

use crate::proc::{run_json_async, JSON_COMMAND_TIMEOUT};

#[tauri::command]
pub(crate) fn get_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

#[tauri::command]
pub(crate) async fn list_devices(app: tauri::AppHandle) -> Result<Value, String> {
    run_json_async(
        app,
        vec!["devices".into(), "--json".into(), "--fast".into()],
        JSON_COMMAND_TIMEOUT,
        "devices",
    )
    .await
}

#[tauri::command]
pub(crate) async fn list_processors(app: tauri::AppHandle) -> Result<Value, String> {
    run_json_async(
        app,
        vec!["processors".into(), "--json".into()],
        JSON_COMMAND_TIMEOUT,
        "processors",
    )
    .await
}

#[tauri::command]
pub(crate) async fn doctor_audio(app: tauri::AppHandle) -> Result<Value, String> {
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
pub(crate) async fn request_system_audio(app: tauri::AppHandle) -> Result<Value, String> {
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

/// macOS 主机信息(机型 / 系统版本 / 芯片 / 内存 / 核心数),用于 NVAFX 不可用态右栏
/// 填充系统信息。字段缺失时返回 null,前端择有而显。仅 macOS;其它平台返回空对象。
#[tauri::command]
pub(crate) async fn mac_system_info() -> Value {
    #[cfg(target_os = "macos")]
    {
        mac_system_info_impl().await
    }
    #[cfg(not(target_os = "macos"))]
    {
        serde_json::json!({})
    }
}

#[cfg(target_os = "macos")]
async fn mac_system_info_impl() -> Value {
    use std::process::Command;

    // sysctl -n <key> → 去掉换行的字符串
    fn sysctl(key: &str) -> Option<String> {
        let out = Command::new("/usr/sbin/sysctl")
            .arg("-n")
            .arg(key)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    // sw_vers → macOS 产品版本(如 26.5.1);代号(Tahoe 等)由前端映射表补。
    let os_version = Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    // 机型:优先 system_profiler 的用户可读机型名(如 "MacBook Pro"),回退到 hw.model。
    let model = mac_model_name().or_else(|| sysctl("hw.model"));

    // 芯片:machdep.cpu.brand_string(Apple Silicon 上 = "Apple M4")
    let chip = sysctl("machdep.cpu.brand_string");

    // 内存(字节 → GB 取整)
    let memory_gb = sysctl("hw.memsize")
        .and_then(|s| s.parse::<u64>().ok())
        .map(|b| (b as f64 / 1_073_741_824.0).round() as u64);

    // 逻辑核心数
    let cores = sysctl("hw.ncpu").and_then(|s| s.parse::<u64>().ok());

    serde_json::json!({
        "model": model,
        "os_version": os_version,
        "chip": chip,
        "memory_gb": memory_gb,
        "cores": cores,
    })
}

/// system_profiler SPHardwareDataType -json → Machine Name / model 名。
/// system_profiler 较慢(~100ms),只在不可用态右栏用一次,可接受。
#[cfg(target_os = "macos")]
fn mac_model_name() -> Option<String> {
    use std::process::Command;
    let out = Command::new("/usr/sbin/system_profiler")
        .args(["SPHardwareDataType", "-json"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    let hw = v.get("SPHardwareDataType")?.get(0)?;
    hw.get("machine_name")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}
