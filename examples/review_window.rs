//! Opens the review window over a queue folder, for looking at it by hand:
//! `cargo run --example review_window -- <queue folder> [<notebook>]`. Esc closes it.

#![windows_subsystem = "windows"]

use fastcapture::ui::review;
use std::path::PathBuf;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, TranslateMessage,
};

fn main() {
    let mut args = std::env::args_os().skip(1);
    let queue = PathBuf::from(args.next().expect("a queue folder"));
    let notebook = args.next().map(PathBuf::from);
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
