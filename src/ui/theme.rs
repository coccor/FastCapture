//! Colors and fonts for FastCapture's own windows: light or dark as Windows' app setting says,
//! and Segoe UI at the window's DPI. A small stand-in for FastPad's palette and type ramp.

use windows_sys::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CreateFontW, DEFAULT_CHARSET, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, HFONT,
    OUT_DEFAULT_PRECIS,
};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

/// A `COLORREF` from RGB.
pub const fn rgb(red: u8, green: u8, blue: u8) -> u32 {
    red as u32 | (green as u32) << 8 | (blue as u32) << 16
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// The window behind everything.
    pub background: u32,
    /// The edit control and chips.
    pub surface: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub stroke: u32,
    pub selection: u32,
    pub error: u32,
}

pub const LIGHT: Palette = Palette {
    dark: false,
    background: rgb(243, 243, 243),
    surface: rgb(255, 255, 255),
    text: rgb(26, 26, 26),
    muted: rgb(96, 96, 96),
    accent: rgb(37, 99, 235),
    stroke: rgb(210, 210, 210),
    selection: rgb(219, 234, 254),
    error: rgb(196, 43, 28),
};

pub const DARK: Palette = Palette {
    dark: true,
    background: rgb(32, 32, 32),
    surface: rgb(43, 43, 43),
    text: rgb(240, 240, 240),
    muted: rgb(160, 160, 160),
    accent: rgb(96, 165, 250),
    stroke: rgb(70, 70, 70),
    selection: rgb(30, 58, 95),
    error: rgb(255, 153, 164),
};

/// Light or dark, from `AppsUseLightTheme` (light when it can't be read).
pub fn current() -> Palette {
    let key: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\0"
        .encode_utf16()
        .collect();
    let name: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();
    let mut value: u32 = 1;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut value).cast(),
            &mut size,
        )
    };
    if status == 0 && value == 0 {
        DARK
    } else {
        LIGHT
    }
}

/// `value` device-independent pixels at `dpi`.
pub fn scale(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

/// Segoe UI at `points` and `dpi`, semibold or not. The caller deletes it.
pub fn font(points: i32, semibold: bool, dpi: u32) -> HFONT {
    let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
    let height = -((points * dpi as i32 + 36) / 72);
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            if semibold { FW_SEMIBOLD } else { FW_NORMAL } as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            0,
            CLEARTYPE_QUALITY as u32,
            FF_DONTCARE as u32,
            face.as_ptr(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_and_sizes() {
        assert_eq!(rgb(0x12, 0x34, 0x56), 0x0056_3412);
        assert_eq!(scale(12, 96), 12);
        assert_eq!(scale(12, 144), 18);
        assert_eq!(scale(520, 192), 1040);
        assert_ne!(LIGHT.background, DARK.background);
        let _ = current();
    }
}
