// Echoless GUI 的 Tauri 后端:
//   - 平台探测(标题栏镜像)
//   - 把 `echoless` CLI 作为 sidecar 调用,只消费 JSON / JSONL 契约
//   - run 的 --status-json 以 JSONL 流式解析,经事件推给前端
//
// 契约真理源:docs/CLI.md + CLI `--json` 实测。
use std::sync::Mutex;

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
#[cfg(target_os = "macos")]
use tauri_plugin_decorum::WebviewWindowExt;

mod bin_resolve;
mod commands;
#[cfg(target_os = "macos")]
mod device_watch;
mod localvqe;
mod nvafx;
mod platform;
mod proc;
mod sidecar;
#[cfg(test)]
mod tests;
mod tray;

use commands::{
    doctor_audio, get_platform, list_devices, list_processors, mac_system_info,
    request_system_audio,
};
use localvqe::{download_localvqe_model, localvqe_assets};
use nvafx::{nvafx_doctor, nvafx_download_install, nvafx_install};
use platform::{default_diag_dir, open_path, open_url};
use proc::{terminate_run, RunState};
use sidecar::{probe_delay, send_run_control, set_bypass, start_run, stop_run, validate_config};
use tray::{close_to_tray_enabled, set_tray_prefs, TrayPrefs};
#[cfg(target_os = "windows")]
use tray::{register_windows_tray, TrayIconState};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_decorum::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(RunState(Mutex::new(None)))
        .manage(TrayPrefs::default());

    #[cfg(target_os = "windows")]
    let builder = builder.manage(TrayIconState(Mutex::new(None)));
    #[cfg(target_os = "macos")]
    let builder = builder.manage(device_watch::DeviceWatchState::default());

    builder
        .invoke_handler(tauri::generate_handler![
            get_platform,
            list_devices,
            list_processors,
            doctor_audio,
            request_system_audio,
            mac_system_info,
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
                    // 先隐藏:前端首屏就绪(booted)后再 show,彻底消除 WebView
                    // 初始化那一两帧的白闪(background_color 在部分平台盖不住)。
                    // 前端 booted 有 1.2s 硬封顶保证必触发;下方再加 Rust 兜底。
                    .visible(false);

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

            // 安全兜底:前端正常会在首屏就绪后经 core window show 权限亮窗;万一前端
            // 完全崩溃(打包后不应发生)也在 5s 后强制显示,绝不留一个永不出现的窗口。
            {
                let w = window.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    let _ = w.show();
                    let _ = w.set_focus();
                });
            }

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
            {
                let device_watch_state = app.state::<device_watch::DeviceWatchState>();
                device_watch::start(app.handle(), &device_watch_state);
            }
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
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Cmd+Q / 菜单退出 / Dock Quit 不产生 CloseRequested(审计 B-01):
            // 统一在 ExitRequested 回收 sidecar,避免孤儿 CLI 占用麦克风/虚拟麦。
            if let tauri::RunEvent::ExitRequested { .. } = event {
                terminate_run(&app_handle.state::<RunState>());
                #[cfg(target_os = "macos")]
                device_watch::stop(&app_handle.state::<device_watch::DeviceWatchState>());
            }
        });
}
