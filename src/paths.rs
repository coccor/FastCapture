//! Where FastCapture keeps its own files: settings under `%AppData%\FastCapture`,
//! per-PC state (queue, index, drafts, logs) under `%LocalAppData%\FastCapture`.

use std::path::PathBuf;

const APP_DIR: &str = "FastCapture";

/// `%AppData%\FastCapture`, or `None` when `APPDATA` isn't set.
pub fn roaming() -> Option<PathBuf> {
    folder("APPDATA")
}

/// `%LocalAppData%\FastCapture`, or `None` when `LOCALAPPDATA` isn't set.
pub fn local() -> Option<PathBuf> {
    folder("LOCALAPPDATA")
}

/// `%LocalAppData%\FastCapture\log`.
pub fn log_dir() -> Option<PathBuf> {
    local().map(|local| local.join("log"))
}

fn folder(variable: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(value).join(APP_DIR))
}
