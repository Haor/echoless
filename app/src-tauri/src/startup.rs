use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::{AppHandle, State};
#[cfg(target_os = "windows")]
use tauri_plugin_autostart::ManagerExt;

pub(crate) const AUTOSTART_ARG: &str = "--autostart";
// Critical path upper bound: command discovery (30s) + config validation (60s)
// + sidecar readiness margin. A shorter watchdog can misclassify a valid login.
pub(crate) const AUTOSTART_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug)]
pub(crate) struct StartupLaunch {
    autostart: bool,
    settled: AtomicBool,
}

impl StartupLaunch {
    pub(crate) fn detect() -> Self {
        Self::from_args(std::env::args(), cfg!(target_os = "windows"))
    }

    pub(crate) fn from_args<I, S>(args: I, is_windows: bool) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            autostart: is_windows && args.into_iter().any(|arg| arg.as_ref() == AUTOSTART_ARG),
            settled: AtomicBool::new(false),
        }
    }

    pub(crate) fn is_autostart(&self) -> bool {
        self.autostart
    }

    fn mode(&self) -> &'static str {
        if self.autostart {
            "autostart"
        } else {
            "manual"
        }
    }

    pub(crate) fn settle(&self) {
        self.settled.store(true, Ordering::SeqCst);
    }

    pub(crate) fn watchdog_pending(&self) -> bool {
        self.autostart && !self.settled.load(Ordering::SeqCst)
    }
}

pub(crate) fn should_focus_existing_instance(args: &[String]) -> bool {
    !args.iter().any(|arg| arg == AUTOSTART_ARG)
}

#[tauri::command]
pub(crate) fn get_startup_mode(launch: State<StartupLaunch>) -> &'static str {
    launch.mode()
}

#[tauri::command]
pub(crate) fn settle_startup_launch(launch: State<StartupLaunch>) {
    launch.settle();
}

#[tauri::command]
pub(crate) fn get_autostart_enabled(app: AppHandle) -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        app.autolaunch().is_enabled().map_err(|err| err.to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        Ok(false)
    }
}

#[tauri::command]
pub(crate) fn set_autostart_enabled(app: AppHandle, enabled: bool) -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        let manager = app.autolaunch();
        if enabled {
            manager.enable().map_err(|err| err.to_string())?;
        } else {
            manager.disable().map_err(|err| err.to_string())?;
        }
        manager.is_enabled().map_err(|err| err.to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, enabled);
        Err("autostart is only supported on Windows".to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        should_focus_existing_instance, StartupLaunch, AUTOSTART_ARG, AUTOSTART_WATCHDOG_TIMEOUT,
    };

    #[test]
    fn detects_windows_autostart_without_affecting_other_platforms() {
        let args = ["Echoless.exe", AUTOSTART_ARG];

        assert!(StartupLaunch::from_args(args, true).is_autostart());
        assert!(!StartupLaunch::from_args(args, false).is_autostart());
        assert!(!StartupLaunch::from_args(["Echoless.exe"], true).is_autostart());
    }

    #[test]
    fn delayed_autostart_does_not_focus_an_existing_manual_instance() {
        assert!(!should_focus_existing_instance(&[
            "Echoless.exe".into(),
            AUTOSTART_ARG.into(),
        ]));
        assert!(should_focus_existing_instance(&["Echoless.exe".into()]));
    }

    #[test]
    fn autostart_watchdog_stays_pending_until_frontend_settles_launch() {
        let launch = StartupLaunch::from_args(["Echoless.exe", AUTOSTART_ARG], true);

        assert!(launch.watchdog_pending());
        launch.settle();
        assert!(!launch.watchdog_pending());
    }

    #[test]
    fn autostart_watchdog_covers_the_full_bounded_startup_path() {
        assert!(AUTOSTART_WATCHDOG_TIMEOUT >= Duration::from_secs(120));
    }
}
