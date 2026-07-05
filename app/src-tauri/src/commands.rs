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
