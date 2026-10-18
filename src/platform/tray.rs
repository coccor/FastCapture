//! The notification-area icon (spec §5.1, §10.1): `Shell_NotifyIconW` with version 4, a tooltip,
//! balloons, and a popup menu. The icon is drawn from `crate::icon` at the size Windows asks
//! for, so it needs no resource and looks the same in light and dark taskbars.

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIIF_NOSOUND,
    NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, DestroyIcon, DestroyMenu,
    GetSystemMetrics, HICON, LR_DEFAULTCOLOR, MF_GRAYED, MF_SEPARATOR, MF_STRING, PostMessageW,
    SM_CXSMICON, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenuEx, WM_NULL,
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

/// One line of the tray menu.
pub enum MenuItem<'a> {
    Command {
        id: u32,
        text: &'a str,
        enabled: bool,
    },
    Separator,
}

/// Shows a popup menu at `at` and returns the chosen command, or 0. The window must be the
/// tray icon's: it is brought to the front so the menu closes when you click elsewhere.
pub(crate) fn menu(window: HWND, at: POINT, items: &[MenuItem<'_>]) -> u32 {
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return 0;
    }
    for item in items {
        match item {
            MenuItem::Command { id, text, enabled } => {
                let text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
                let flags = if *enabled {
                    MF_STRING
                } else {
                    MF_STRING | MF_GRAYED
                };
                unsafe { AppendMenuW(menu, flags, *id as usize, text.as_ptr()) };
            }
            MenuItem::Separator => unsafe {
                AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            },
        }
    }
    let chosen = unsafe {
        SetForegroundWindow(window);
        let chosen = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            at.x,
            at.y,
            window,
            std::ptr::null(),
        );
        PostMessageW(window, WM_NULL, 0, 0);
        DestroyMenu(menu);
        chosen
    };
    chosen as u32
}
