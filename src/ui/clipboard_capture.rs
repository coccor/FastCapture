//! Capture clipboard: the clipboard as it is, with no box. Files copied in Explorer
//! come first; then an image when there is no text (as in FastPad, copying a paragraph from Word
//! also puts a picture of it on the clipboard, and that must not win over its text); then rich
//! text as Markdown, or plain text.

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
    // Rich text with structure is kept as Markdown, as Ctrl+V in the box does.
    let html = clipboard::html(owner);
    let source_url = html.as_ref().and_then(|html| html.source_url.clone());
    let rich = html.and_then(|html| {
        crate::extract::markdown::rich_text(&html.fragment, html.source_url.as_deref())
    });
    match rich.or_else(|| clipboard::text(owner)) {
        Some(text) if !text.trim().is_empty() => Ok(Capture {
            text,
            source_url,
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

    #[test]
    fn rich_text_is_captured_as_markdown_with_its_page() {
        let _backup = Backup::take();
        clipboard::set_html(
            std::ptr::null_mut(),
            "<ol><li>first</li><li>second</li></ol>",
            "first\r\nsecond",
            Some("https://example.com/steps"),
        );
        assert_eq!(
            from_clipboard(std::ptr::null_mut()),
            Ok(Capture {
                text: "1. first\n2. second".to_owned(),
                source_url: Some("https://example.com/steps".to_owned()),
                ..Capture::default()
            })
        );
        clipboard::set_html(std::ptr::null_mut(), "<p>plain</p>", "plain", None);
        assert_eq!(
            from_clipboard(std::ptr::null_mut()),
            Ok(Capture {
                text: "plain".to_owned(),
                ..Capture::default()
            })
        );
    }
}
