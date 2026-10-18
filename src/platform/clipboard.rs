//! The clipboard: text, images (as PNG) and files copied in Explorer. `has_*`, `files` and
//! `image_png` are copied from FastPad `src/platform/clipboard.rs`; reading text is FastCapture's.

use super::last_error;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::{GlobalFree, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};

/// Opens the clipboard, waiting briefly while another program holds it.
fn open(owner: HWND) -> bool {
    for _ in 0..10 {
        if unsafe { OpenClipboard(owner) } != 0 {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

/// The registered `PNG` format browsers and Snipping Tool put on the clipboard.
fn png_format() -> u32 {
    let name: Vec<u16> = "PNG\0".encode_utf16().collect();
    unsafe { RegisterClipboardFormatW(name.as_ptr()) }
}

fn available(format: u32) -> bool {
    format != 0 && unsafe { IsClipboardFormatAvailable(format) } != 0
}

/// Whether the clipboard holds text.
pub fn has_text() -> bool {
    available(u32::from(CF_UNICODETEXT))
}

/// Whether the clipboard holds an image a capture can attach.
pub fn has_image() -> bool {
    available(png_format()) || available(u32::from(CF_DIBV5)) || available(u32::from(CF_DIB))
}

/// Whether the clipboard holds files copied in Explorer.
pub fn has_files() -> bool {
    available(u32::from(CF_HDROP))
}

/// A copy of the clipboard's `format` block; the clipboard must be open.
fn read_global(format: u32) -> Option<Vec<u8>> {
    let handle = unsafe { GetClipboardData(format) };
    if handle.is_null() {
        return None;
    }
    let size = unsafe { GlobalSize(handle) };
    let data = unsafe { GlobalLock(handle) }.cast::<u8>();
    if data.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, size) }.to_vec();
    unsafe { GlobalUnlock(handle) };
    Some(bytes)
}

/// The clipboard's text, or `None` when it holds none.
pub fn text(owner: HWND) -> Option<String> {
    if !has_text() || !open(owner) {
        return None;
    }
    let bytes = read_global(u32::from(CF_UNICODETEXT));
    unsafe { CloseClipboard() };
    let units: Vec<u16> = bytes?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|unit| *unit != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// The files copied in Explorer (`CF_HDROP`), or none.
pub fn files(owner: HWND) -> Vec<PathBuf> {
    if !has_files() || !open(owner) {
        return Vec::new();
    }
    let handle = unsafe { GetClipboardData(u32::from(CF_HDROP)) };
    let paths = if handle.is_null() {
        Vec::new()
    } else {
        dropped_paths(handle)
    };
    unsafe { CloseClipboard() };
    paths
}

/// Copied from FastPad `src/platform/win32.rs`: the paths in a `DROPFILES` list. Does not free
/// the handle.
pub(crate) fn dropped_paths(drop: windows_sys::Win32::UI::Shell::HDROP) -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::Shell::DragQueryFileW;
    let count = unsafe { DragQueryFileW(drop, u32::MAX, std::ptr::null_mut(), 0) };
    let mut paths = Vec::new();
    for index in 0..count {
        let length = unsafe { DragQueryFileW(drop, index, std::ptr::null_mut(), 0) } as usize;
        if length == 0 {
            continue;
        }
        let mut buffer = vec![0_u16; length + 1];
        let copied =
            unsafe { DragQueryFileW(drop, index, buffer.as_mut_ptr(), buffer.len() as u32) }
                as usize;
        if copied == 0 {
            continue;
        }
        buffer.truncate(copied.min(length));
        paths.push(PathBuf::from(std::ffi::OsString::from_wide(&buffer)));
    }
    paths
}

/// An image read from the clipboard, before any encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClipboardImage {
    /// The registered `PNG` format, as it is (browsers, Snipping Tool).
    Png(Vec<u8>),
    /// A `CF_DIBV5` or `CF_DIB` block (the PrintScreen key, many apps), still to be encoded.
    Dib(Vec<u8>),
}

/// The clipboard's image as it is, or `None` when it holds none. Copying it out is quick;
/// encoding a bitmap is not, so that is left to `encode_dib`, off the UI thread.
pub fn image(owner: HWND) -> Option<ClipboardImage> {
    if !has_image() || !open(owner) {
        return None;
    }
    let png = read_global(png_format()).filter(|bytes| bytes.starts_with(b"\x89PNG"));
    let dib = png
        .is_none()
        .then(|| read_global(u32::from(CF_DIBV5)).or_else(|| read_global(u32::from(CF_DIB))))
        .flatten();
    unsafe { CloseClipboard() };
    png.map(ClipboardImage::Png)
        .or(dib.map(ClipboardImage::Dib))
}

/// A clipboard bitmap as PNG bytes, encoded with WIC.
pub fn encode_dib(dib: &[u8]) -> Result<Vec<u8>, String> {
    match super::png::dib_to_pixels(dib) {
        Some(pixels) => super::png::encode_png(&pixels),
        None => Err("FastCapture can't read this kind of image from the clipboard.".to_owned()),
    }
}

/// The clipboard's image as PNG bytes: the `PNG` format as it is, else a bitmap encoded here.
/// `None` when it holds no image.
pub fn image_png(owner: HWND) -> Option<Result<Vec<u8>, String>> {
    Some(match image(owner)? {
        ClipboardImage::Png(png) => Ok(png),
        ClipboardImage::Dib(dib) => encode_dib(&dib),
    })
}

/// Replaces the clipboard's contents with `format` holding `bytes`. For tests and, later, Copy
/// commands.
pub fn set(owner: HWND, format: u32, bytes: &[u8]) -> crate::Result<()> {
    if !open(owner) {
        return Err(last_error("OpenClipboard"));
    }
    let result = (|| unsafe {
        EmptyClipboard();
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
        if memory.is_null() {
            return Err(last_error("GlobalAlloc"));
        }
        let target = GlobalLock(memory).cast::<u8>();
        if target.is_null() {
            let error = last_error("GlobalLock");
            GlobalFree(memory);
            return Err(error);
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
        GlobalUnlock(memory);
        // The clipboard owns the memory once SetClipboardData succeeds.
        if SetClipboardData(format, memory).is_null() {
            let error = last_error("SetClipboardData");
            GlobalFree(memory);
            return Err(error);
        }
        Ok(())
    })();
    unsafe { CloseClipboard() };
    result
}

/// `text` as the clipboard's `CF_UNICODETEXT`.
pub fn set_text(owner: HWND, text: &str) -> crate::Result<()> {
    let bytes: Vec<u8> = text
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    set(owner, u32::from(CF_UNICODETEXT), &bytes)
}

#[cfg(test)]
pub(crate) mod tests {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, OpenClipboard,
        SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
    };

    fn open() {
        for _ in 0..50 {
            if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the clipboard stayed open in another program");
    }

    /// Copied from FastPad: the user's clipboard, put back on drop (every format held in global
    /// memory; Windows synthesizes the GDI ones).
    pub(crate) struct Backup(Vec<(u32, Vec<u8>)>);

    impl Backup {
        pub(crate) fn take() -> Self {
            const GDI_HANDLES: [u32; 7] = [2, 3, 9, 14, 0x80, 0x82, 0x8e];
            let mut formats = Vec::new();
            open();
            unsafe {
                let mut format = EnumClipboardFormats(0);
                while format != 0 {
                    let private_gdi = (0x300..=0x3ff).contains(&format);
                    if !GDI_HANDLES.contains(&format) && !private_gdi {
                        let data = GetClipboardData(format);
                        let size = if data.is_null() { 0 } else { GlobalSize(data) };
                        let bytes = if size == 0 {
                            std::ptr::null()
                        } else {
                            GlobalLock(data) as *const u8
                        };
                        if !bytes.is_null() {
                            formats
                                .push((format, std::slice::from_raw_parts(bytes, size).to_vec()));
                            GlobalUnlock(data);
                        }
                    }
                    format = EnumClipboardFormats(format);
                }
                CloseClipboard();
            }
            Self(formats)
        }
    }

    impl Drop for Backup {
        fn drop(&mut self) {
            open();
            unsafe {
                EmptyClipboard();
                for (format, bytes) in &self.0 {
                    let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
                    if memory.is_null() {
                        continue;
                    }
                    let target = GlobalLock(memory) as *mut u8;
                    if !target.is_null() {
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
                        GlobalUnlock(memory);
                        SetClipboardData(*format, memory);
                    }
                }
                CloseClipboard();
            }
        }
    }

    #[test]
    fn text_and_images_read_back_from_the_clipboard() {
        let _backup = Backup::take();
        super::set_text(std::ptr::null_mut(), "zăpadă 🦀").unwrap();
        assert!(super::has_text());
        assert!(!super::has_image());
        assert_eq!(
            super::text(std::ptr::null_mut()).as_deref(),
            Some("zăpadă 🦀")
        );
        assert_eq!(super::image_png(std::ptr::null_mut()), None);

        super::set(
            std::ptr::null_mut(),
            super::png_format(),
            b"\x89PNG\r\n\x1a\nrest",
        )
        .unwrap();
        assert_eq!(
            super::image_png(std::ptr::null_mut()),
            Some(Ok(b"\x89PNG\r\n\x1a\nrest".to_vec()))
        );
        assert!(super::files(std::ptr::null_mut()).is_empty());
    }
}
