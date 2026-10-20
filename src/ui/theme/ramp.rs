//! The text styles FastCapture's windows use, FastPad's type ramp: a height in pixels at 96 DPI,
//! a weight and an italic flag each. Sizes follow Windows' text-size setting (Accessibility ›
//! Text size, 100 to 225 %), read the first time it is needed and again when Windows says its
//! settings changed.

use super::faces;
use super::metrics::scale;
use std::sync::atomic::{AtomicU32, Ordering};
use windows_sys::Win32::Graphics::Gdi::{FW_BOLD, FW_NORMAL, FW_SEMIBOLD, HFONT};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextStyle {
    /// Text, labels, list rows, buttons, the title strip and the status line.
    Body,
    BodyItalic,
    BodyBold,
    /// A group's heading inside a window.
    Heading,
    /// A window's own title, when it shows one inside.
    Title,
}

impl TextStyle {
    /// Pixels at 96 DPI, the GDI weight, italic.
    pub const fn spec(self) -> (i32, i32, bool) {
        let (pixels, weight, italic) = match self {
            Self::Body => (13, FW_NORMAL, false),
            Self::BodyItalic => (13, FW_NORMAL, true),
            Self::BodyBold => (13, FW_BOLD, false),
            Self::Heading => (14, FW_SEMIBOLD, false),
            Self::Title => (18, FW_SEMIBOLD, false),
        };
        (pixels, weight as i32, italic)
    }
}

/// A GDI font for `style` at `dpi` and the text size. The caller deletes it.
pub fn font(style: TextStyle, dpi: u32) -> HFONT {
    let (pixels, weight, italic) = style.spec();
    let faces = faces::current();
    let face = if style == TextStyle::Title {
        faces.display
    } else {
        faces.text
    };
    faces::create(scale_text(pixels, dpi), face, weight, italic)
}

/// A glyph font (Segoe Fluent Icons or Segoe MDL2 Assets) `pixels` high at 96 DPI. The caller
/// deletes it.
pub fn icon_font(pixels: i32, dpi: u32) -> HFONT {
    faces::create(
        scale(pixels, dpi),
        faces::current().icons,
        FW_NORMAL as i32,
        false,
    )
}

const MIN: u32 = 100;
const MAX: u32 = 225;
/// 0 until it has been read.
static FACTOR: AtomicU32 = AtomicU32::new(0);

/// The percentage for a registry value: 100 when there is none, else kept within 100 to 225.
pub fn parse_factor(raw: Option<u32>) -> u32 {
    raw.map_or(MIN, |value| value.clamp(MIN, MAX))
}

/// Tests never read the machine's setting: they would depend on it.
#[cfg(test)]
fn read_factor() -> u32 {
    MIN
}

#[cfg(not(test))]
fn read_factor() -> u32 {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain(std::iter::once(0)).collect() };
    let key = wide(r"Software\Microsoft\Accessibility");
    let name = wide("TextScaleFactor");
    let mut value: u32 = 0;
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
    parse_factor((status == 0).then_some(value))
}

/// The text size, 100 to 225 %.
pub fn factor() -> u32 {
    match FACTOR.load(Ordering::Relaxed) {
        0 => {
            let value = read_factor();
            FACTOR.store(value, Ordering::Relaxed);
            value
        }
        value => value,
    }
}

/// Reads the text size again; true when it changed.
pub fn refresh() -> bool {
    let before = factor();
    let after = read_factor();
    FACTOR.store(after, Ordering::Relaxed);
    before != after
}

/// `value` (pixels at 96 DPI) at `dpi`, then at the text size, rounded half up.
pub fn scale_text(value: i32, dpi: u32) -> i32 {
    let scaled = i64::from(scale(value, dpi));
    ((scaled * i64::from(factor()) + 50) / 100) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ramp_is_fastpads() {
        assert_eq!(TextStyle::Body.spec(), (13, FW_NORMAL as i32, false));
        assert_eq!(TextStyle::Heading.spec(), (14, FW_SEMIBOLD as i32, false));
        assert_eq!(TextStyle::Title.spec(), (18, FW_SEMIBOLD as i32, false));
        for style in [
            TextStyle::Body,
            TextStyle::BodyItalic,
            TextStyle::BodyBold,
            TextStyle::Heading,
            TextStyle::Title,
        ] {
            let font = font(style, 144);
            assert!(!font.is_null(), "{style:?}");
            unsafe { windows_sys::Win32::Graphics::Gdi::DeleteObject(font) };
        }
        let icons = icon_font(10, 96);
        assert!(!icons.is_null());
        unsafe { windows_sys::Win32::Graphics::Gdi::DeleteObject(icons) };
    }

    #[test]
    fn the_text_size_is_kept_in_range_and_scales_after_the_dpi() {
        assert_eq!(parse_factor(None), 100);
        assert_eq!(parse_factor(Some(50)), 100);
        assert_eq!(parse_factor(Some(150)), 150);
        assert_eq!(parse_factor(Some(400)), 225);
        assert_eq!(factor(), 100, "tests don't read the machine's setting");
        assert_eq!(scale_text(13, 96), 13);
        assert_eq!(scale_text(13, 144), 20);
        FACTOR.store(150, Ordering::Relaxed);
        assert_eq!(scale_text(13, 96), 20);
        assert_eq!(scale_text(13, 144), 30);
        assert!(refresh(), "back to 100");
        assert_eq!(factor(), 100);
    }
}
