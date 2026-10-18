//! Global hotkeys with `RegisterHotKey`. `WM_HOTKEY` arrives at the window with the
//! hotkey's id.

use super::last_error;
use crate::config::keymap::KeyStroke;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
};

pub(crate) fn register(window: HWND, id: i32, stroke: KeyStroke) -> crate::Result<()> {
    let modifiers = stroke.hotkey_modifiers() | MOD_NOREPEAT;
    if unsafe { RegisterHotKey(window, id, modifiers, u32::from(stroke.vk)) } == 0 {
        return Err(last_error("RegisterHotKey"));
    }
    Ok(())
}

pub(crate) fn unregister(window: HWND, id: i32) {
    unsafe { UnregisterHotKey(window, id) };
}
