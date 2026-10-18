//! Small window helpers.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
};

/// A window handle that can cross threads, for posting messages back to the app's window.
#[derive(Clone, Copy, Debug)]
pub struct SendHwnd(usize);

impl SendHwnd {
    pub fn new(window: HWND) -> Self {
        Self(window as usize)
    }

    pub fn get(self) -> HWND {
        self.0 as HWND
    }
}

/// The title of `window`, empty when it has none.
pub(crate) fn title(window: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(window) };
    if length <= 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; length as usize + 1];
    let copied = unsafe { GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
}

/// The title of the window in front, when there is one with a title.
pub fn foreground_title() -> Option<String> {
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return None;
    }
    let title = title(window);
    (!title.trim().is_empty()).then_some(title)
}
