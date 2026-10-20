//! Opens the Settings window on its own, for looking at it by hand:
//! `cargo run --example settings_window -- <fastcapture.ini path>`. Saving or Cancel closes it.

#![windows_subsystem = "windows"]

use fastcapture::ui::settings;
use std::path::PathBuf;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, TranslateMessage,
};

fn main() {
    let path = PathBuf::from(std::env::args_os().nth(1).expect("a settings file path"));
    // As the tray app does: the file's theme, and Windows' changes followed.
    let (chosen, _) = fastcapture::config::settings::SettingsFile::load(&path).settings();
    fastcapture::ui::theme::set_preference(chosen.theme);
    if settings::open(settings::Open {
        settings: path,
        claude_key: fastcapture::secrets::target("example-claude"),
        openai_key: fastcapture::secrets::target("example-openai"),
        usage: fastcapture::runner::usage::path(),
        saved: Box::new(|| {}),
        open_file: Box::new(|| {}),
    })
    .is_none()
    {
        return;
    }
    let mut message: MSG = unsafe { std::mem::zeroed() };
    while settings::find().is_some()
        && unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0
    {
        if settings::dialog_message(&message) {
            continue;
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
