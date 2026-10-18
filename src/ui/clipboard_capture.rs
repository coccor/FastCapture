//! Capture clipboard: the clipboard as it is, with no box. Files copied in Explorer
//! come first; then an image when there is no text (as in FastPad, copying a paragraph from Word
//! also puts a picture of it on the clipboard, and that must not win over its text); then text.

use crate::capture::{Attachment, Capture};
use crate::platform::clipboard;
use windows_sys::Win32::Foundation::HWND;

/// What the clipboard holds, as a capture, or why there's nothing to capture.
pub fn from_clipboard(owner: HWND) -> Result<Capture, String> {
    let files = clipboard::files(owner);
    if !files.is_empty() {
        return Ok(Capture {
            attachments: files.into_iter().map(Attachment::File).collect(),
            ..Capture::default()
        });
    }
    if !clipboard::has_text()
        && let Some(png) = clipboard::image_png(owner)
    {
        return png.map(|png| Capture {
            attachments: vec![Attachment::Png(png)],
            ..Capture::default()
        });
    }
    match clipboard::text(owner) {
        Some(text) if !text.trim().is_empty() => Ok(Capture {
            text,
            ..Capture::default()
        }),
        _ => Err("The clipboard has nothing FastCapture can capture.".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::clipboard::tests::Backup;

    #[test]
    fn text_images_and_nothing() {
        let _backup = Backup::take();
        clipboard::set_text(std::ptr::null_mut(), "a thought").unwrap();
        assert_eq!(
            from_clipboard(std::ptr::null_mut()),
            Ok(Capture {
                text: "a thought".to_owned(),
                ..Capture::default()
            })
        );
        let png_format: Vec<u16> = "PNG\0".encode_utf16().collect();
        let format = unsafe {
            windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW(png_format.as_ptr())
        };
        clipboard::set(std::ptr::null_mut(), format, b"\x89PNG\r\n\x1a\nx").unwrap();
        assert_eq!(
            from_clipboard(std::ptr::null_mut()),
            Ok(Capture {
                attachments: vec![Attachment::Png(b"\x89PNG\r\n\x1a\nx".to_vec())],
                ..Capture::default()
            })
        );
        clipboard::set_text(std::ptr::null_mut(), "  \r\n").unwrap();
        assert!(from_clipboard(std::ptr::null_mut()).is_err());
    }
}
