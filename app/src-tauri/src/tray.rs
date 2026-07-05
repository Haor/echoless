use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "windows")]
use std::sync::Mutex;
#[cfg(target_os = "windows")]
use std::sync::MutexGuard;

use tauri::State;
#[cfg(target_os = "windows")]
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

#[cfg(target_os = "windows")]
use crate::proc::{terminate_run, RunState};

#[cfg(target_os = "windows")]
pub(crate) struct TrayIconState(pub(crate) Mutex<Option<TrayIcon>>);

/// Windows tray preferences pushed by the frontend at startup and on change.
/// 只剩「关闭到托盘」——最小化到托盘已退役(用户定案 2026-07-05)。
pub(crate) struct TrayPrefs {
    pub(crate) close_to_tray: AtomicBool,
}

impl Default for TrayPrefs {
    fn default() -> Self {
        Self {
            close_to_tray: AtomicBool::new(false),
        }
    }
}

#[cfg(target_os = "windows")]
fn tray_icon_state_guard(state: &TrayIconState) -> MutexGuard<'_, Option<TrayIcon>> {
    state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn set_tray_prefs_inner(prefs: &TrayPrefs, close_to_tray: bool) {
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

pub(crate) fn close_to_tray_enabled(prefs: &TrayPrefs) -> bool {
    tray_pref_enabled(&prefs.close_to_tray)
}

fn tray_tooltip(running: bool) -> &'static str {
    if running {
        "Echoless — RUNNING"
    } else {
        "Echoless — STOPPED"
    }
}

pub(crate) fn update_tray_tooltip(app: &tauri::AppHandle, running: bool) {
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
pub(crate) fn register_windows_tray(app: &mut tauri::App) -> tauri::Result<()> {
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

#[tauri::command]
pub(crate) fn set_tray_prefs(prefs: State<TrayPrefs>, close_to_tray: bool) {
    set_tray_prefs_inner(&prefs, close_to_tray);
}
