//! Opens the review window over a queue folder, for looking at it by hand:
//! `cargo run --example review_window -- <queue folder> [<notebook>]`, or
//! `-- --sample <empty folder> [light|dark]` for a scratch notebook with a few captures in it.
//! Esc closes it.

#![windows_subsystem = "windows"]

use fastcapture::ui::review;
use std::path::PathBuf;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, TranslateMessage,
};

/// A scratch notebook with three captures queued, and its queue.
fn sample(folder: &std::path::Path) -> (PathBuf, Option<PathBuf>) {
    use fastcapture::capture::{Capture, submit::submit};
    let notebook = folder.join("Sample notebook");
    let queue = folder.join("queue");
    std::fs::create_dir_all(&notebook).unwrap();
    for text in [
        "lag spikes during ledger batch?? check batch size",
        "Kafka consumer groups: read later",
        "call Ana about the Q4 payments roadmap",
    ] {
        let capture = Capture {
            text: text.to_owned(),
            attachments: Vec::new(),
            source_window: None,
            source_url: None,
        };
        submit(&notebook, &capture, Some(&queue), true).unwrap();
    }
    (queue, Some(notebook))
}

fn main() {
    // As the app's manifest makes it: drawn at the monitor's own DPI.
    unsafe {
        windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
    };
    let mut args = std::env::args_os().skip(1);
    let first = PathBuf::from(args.next().expect("a queue folder"));
    let (queue, notebook) = if first.as_os_str() == "--sample" {
        let folder = PathBuf::from(args.next().expect("an empty folder"));
        let theme = match args.next().as_deref().and_then(|theme| theme.to_str()) {
            Some("light") => fastcapture::ui::theme::ThemePreference::Light,
            Some("dark") => fastcapture::ui::theme::ThemePreference::Dark,
            _ => fastcapture::ui::theme::ThemePreference::System,
        };
        fastcapture::ui::theme::set_preference(theme);
        sample(&folder)
    } else {
        (first, args.next().map(PathBuf::from))
    };
    if review::open(review::Open {
        queue,
        settings: None,
        notebook,
        indexes: Some(fastcapture::index::service::Indexes::new(None)),
        actions: Box::new(review::ShellActions {
            on_change: Box::new(|| {}),
        }),
    })
    .is_none()
    {
        return;
    }
    let mut message: MSG = unsafe { std::mem::zeroed() };
    while review::find().is_some()
        && unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0
    {
        if fastcapture::ui::home_picker::dialog_message(&message)
            || review::dialog_message(&message)
        {
            continue;
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
