use std::env;
use std::path::PathBuf;

pub const BRAND_DIR_NAME: &str = "Echoless";
pub const DATA_ROOT_ENV_VAR: &str = "ECHOLESS_DATA_ROOT";

pub fn brand_data_root() -> (PathBuf, String) {
    if let Some(dir) = env::var_os(DATA_ROOT_ENV_VAR).filter(|value| !value.is_empty()) {
        return (PathBuf::from(dir), DATA_ROOT_ENV_VAR.to_string());
    }

    default_brand_data_root()
}

pub fn default_brand_data_root() -> (PathBuf, String) {
    #[cfg(windows)]
    {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        return (base.join(BRAND_DIR_NAME), "%LOCALAPPDATA%".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        let base = env::var_os("HOME")
            .map(|home| {
                PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
            })
            .unwrap_or_else(|| PathBuf::from("."));
        return (
            base.join(BRAND_DIR_NAME),
            "$HOME/Library/Application Support".to_string(),
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
            })
            .unwrap_or_else(|| PathBuf::from("."));
        return (base.join(BRAND_DIR_NAME), "XDG_DATA_HOME".to_string());
    }

    #[allow(unreachable_code)]
    (PathBuf::from(BRAND_DIR_NAME), "fallback".to_string())
}
