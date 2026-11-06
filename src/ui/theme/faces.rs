//! The font faces FastCapture's windows draw with, as FastPad's do: Segoe UI Variable and Segoe
//! Fluent Icons on Windows 11 (the icon face keeps Segoe MDL2 Assets' code points), Segoe UI and
//! Segoe MDL2 Assets before it. Chosen the first time a font is made, then kept.

use std::sync::OnceLock;
use windows_sys::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    DeleteObject, FW_NORMAL, GetDC, GetTextFaceW, HFONT, OUT_DEFAULT_PRECIS, ReleaseDC,
    SelectObject,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Faces {
    /// Every text style but the title.
    pub text: &'static str,
    /// Titles.
    pub display: &'static str,
    /// Glyphs.
    pub icons: &'static str,
}

pub const LEGACY: Faces = Faces {
    text: "Segoe UI",
    display: "Segoe UI",
    icons: "Segoe MDL2 Assets",
};
pub const WINDOWS_11: Faces = Faces {
    text: "Segoe UI Variable Text",
    display: "Segoe UI Variable Display",
    icons: "Segoe Fluent Icons",
};
/// The first Windows 11 build.
const WINDOWS_11_BUILD: u32 = 22000;

/// Windows 11's faces from build 22000 when `probe` finds the text face, the older ones
/// otherwise. `probe` is only called on Windows 11.
pub fn choose(build: Option<u32>, probe: impl Fn(&str) -> bool) -> Faces {
    match build {
        Some(build) if build >= WINDOWS_11_BUILD && probe(WINDOWS_11.text) => WINDOWS_11,
        _ => LEGACY,
    }
}

/// This PC's faces, found once.
pub fn current() -> Faces {
    static CURRENT: OnceLock<Faces> = OnceLock::new();
    *CURRENT.get_or_init(|| choose(crate::platform::system::windows_build(), is_installed))
}

/// A GDI font of `face` at `pixels` high, `weight`, italic or not. The caller deletes it.
pub fn create(pixels: i32, face: &str, weight: i32, italic: bool) -> HFONT {
    let face: Vec<u16> = face.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        CreateFontW(
            -pixels,
            0,
            0,
            0,
            weight,
            u32::from(italic),
            0,
            0,
            u32::from(DEFAULT_CHARSET),
            u32::from(OUT_DEFAULT_PRECIS),
            u32::from(CLIP_DEFAULT_PRECIS),
            u32::from(CLEARTYPE_QUALITY),
            u32::from(DEFAULT_PITCH),
            face.as_ptr(),
        )
    }
}

/// Whether GDI has `face` itself: a missing face is replaced by another, which reports its own
/// name.
fn is_installed(face: &str) -> bool {
    let font = create(13, face, FW_NORMAL as i32, false);
    if font.is_null() {
        return false;
    }
    unsafe {
        let dc = GetDC(std::ptr::null_mut());
        if dc.is_null() {
            DeleteObject(font);
            return false;
        }
        let previous = SelectObject(dc, font);
        let mut name = [0u16; 64];
        let written = GetTextFaceW(dc, name.len() as i32, name.as_mut_ptr());
        SelectObject(dc, previous);
        ReleaseDC(std::ptr::null_mut(), dc);
        DeleteObject(font);
        let length = usize::try_from(written).unwrap_or(0).saturating_sub(1);
        length > 0
            && String::from_utf16_lossy(&name[..length.min(name.len())]).eq_ignore_ascii_case(face)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_11_gets_segoe_ui_variable_when_it_is_there() {
        let never = |_: &str| -> bool { panic!("only Windows 11 is probed") };
        assert_eq!(choose(Some(19045), never), LEGACY);
        assert_eq!(choose(None, never), LEGACY);
        assert_eq!(choose(Some(22000), |_| true), WINDOWS_11);
        assert_eq!(
            choose(Some(26300), |_| false),
            LEGACY,
            "the face is missing"
        );
        assert!(!is_installed("No Such Face 1234"));
        let _ = current();
    }
}
