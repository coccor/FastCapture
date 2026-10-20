//! The notification-area icon: `Shell_NotifyIconW` with version 4, a tooltip,
//! balloons, and a popup menu. The icon is drawn from `crate::icon` at the size Windows asks
//! for, so it needs no resource and looks the same in light and dark taskbars.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIIF_NOSOUND,
    NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DestroyIcon, GetSystemMetrics, HICON, LR_DEFAULTCOLOR, SM_CXSMICON,
};

/// The icon's id within the window.
const ICON_ID: u32 = 1;

/// The tray icon's `NOTIFYICON_VERSION_4` events, in the callback's low word of `lParam`.
pub const NIN_SELECT: u32 = 0x0400;
pub const NIN_KEYSELECT: u32 = 0x0401;
pub const WM_CONTEXTMENU: u32 = 0x007B;

fn copy_into(target: &mut [u16], text: &str) {
    let units: Vec<u16> = text.encode_utf16().take(target.len() - 1).collect();
    target[..units.len()].copy_from_slice(&units);
    target[units.len()] = 0;
}

fn small_icon() -> HICON {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64) as u32;
    let mut bits = crate::icon::bitmap(size);
    unsafe {
        CreateIconFromResourceEx(
            bits.as_mut_ptr(),
            bits.len() as u32,
            1,
            0x0003_0000,
            size as i32,
            size as i32,
            LR_DEFAULTCOLOR,
        )
    }
}

pub struct Tray {
    window: HWND,
    callback: u32,
    icon: HICON,
    tip: String,
}

impl Tray {
    /// Adds the icon to `window`, which gets `callback` messages for it.
    pub fn add(window: HWND, callback: u32, tip: &str) -> Self {
        let tray = Self {
            window,
            callback,
            icon: small_icon(),
            tip: tip.to_owned(),
        };
        tray.show();
        tray
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.window;
        data.uID = ICON_ID;
        data
    }

    /// Adds the icon again, after Explorer restarted (`TaskbarCreated`) or the first time.
    pub fn show(&self) {
        let mut data = self.data();
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = self.callback;
        data.hIcon = self.icon;
        copy_into(&mut data.szTip, &self.tip);
        unsafe {
            Shell_NotifyIconW(NIM_ADD, &data);
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
    }

    pub fn set_tip(&mut self, tip: &str) {
        if self.tip == tip {
            return;
        }
        self.tip = tip.to_owned();
        let mut data = self.data();
        data.uFlags = NIF_TIP | NIF_SHOWTIP;
        copy_into(&mut data.szTip, tip);
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    /// A balloon (a toast on Windows 10 and 11) from the icon.
    pub fn notify(&self, title: &str, text: &str, error: bool) {
        let mut data = self.data();
        data.uFlags = NIF_INFO;
        copy_into(&mut data.szInfoTitle, title);
        copy_into(&mut data.szInfo, text);
        data.dwInfoFlags = (if error { NIIF_ERROR } else { NIIF_INFO }) | NIIF_NOSOUND;
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let data = self.data();
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &data);
            DestroyIcon(self.icon);
        }
    }
}
