//! Opens a window drawn with FastCapture's look, for checking it by eye: the frame, the painting
//! kit's shapes and text, and real standard controls painted in the theme.
//! `cargo run --example look -- [system|light|dark] [home|capture]`. It follows Windows when it
//! says System; `home` opens the home picker on its own instead, and `capture` the capture box.

#![windows_subsystem = "windows"]

use fastcapture::config::settings::ThemePreference;
use fastcapture::ui::controls::{self, Look, Segment, Surface};
use fastcapture::ui::frame::{self, Buttons, Frame};
use fastcapture::ui::paint::{self, Align, Corners, rect};
use fastcapture::ui::theme::metrics::{CONTROL_RADIUS, ROW, scale};
use fastcapture::ui::theme::ramp::{self, TextStyle};
use fastcapture::ui::theme::{self, Palette};
use std::cell::RefCell;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{DeleteObject, HGDIOBJ, InvalidateRect};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_SETCHECK, BS_AUTOCHECKBOX, BS_AUTORADIOBUTTON, BS_DEFPUSHBUTTON, BS_PUSHBUTTON,
    CB_ADDSTRING, CB_SETCURSEL, CBS_DROPDOWN, CBS_DROPDOWNLIST, CW_USEDEFAULT, CreateWindowExW,
    DefWindowProcW, DispatchMessageW, GetMessageW, IsDialogMessageW, IsZoomed, MSG,
    PostQuitMessage, RegisterClassW, SW_SHOW, SendMessageW, ShowWindow, TranslateMessage,
    WM_DESTROY, WM_ERASEBKGND, WM_NOTIFY, WM_PAINT, WM_SETFONT, WM_SETTINGCHANGE, WNDCLASSW,
    WS_CHILD, WS_CLIPCHILDREN, WS_EX_CLIENTEDGE, WS_GROUP, WS_OVERLAPPEDWINDOW, WS_TABSTOP,
    WS_VISIBLE, WS_VSCROLL,
};

thread_local! {
    static FRAME: RefCell<Option<Frame>> = const { RefCell::new(None) };
}

fn draw(
    dc: windows_sys::Win32::Graphics::Gdi::HDC,
    area: windows_sys::Win32::Foundation::RECT,
    palette: &Palette,
    dpi: u32,
) {
    let s = |value| scale(value, dpi);
    paint::fill(dc, area, palette.content_background);
    let body = ramp::font(TextStyle::Body, dpi);
    let heading = ramp::font(TextStyle::Heading, dpi);
    let title = ramp::font(TextStyle::Title, dpi);
    let left = s(20);
    let mut y = s(56);
    paint::text(
        dc,
        "Look",
        rect(left, y, area.right, y + s(28)),
        title,
        palette.content_foreground,
        Align::Left,
    );
    y += s(36);
    paint::text(
        dc,
        "Text and buttons",
        rect(left, y, area.right, y + s(22)),
        heading,
        palette.content_foreground,
        Align::Left,
    );
    y += s(28);
    paint::text(
        dc,
        "Body text, and muted text beside it.",
        rect(left, y, s(300), y + s(20)),
        body,
        palette.content_foreground,
        Align::Left,
    );
    paint::text(
        dc,
        "Muted",
        rect(s(300), y, area.right, y + s(20)),
        body,
        palette.muted_foreground,
        Align::Left,
    );
    y += s(32);
    let radius = paint::radius(palette, CONTROL_RADIUS, dpi);
    let primary = rect(left, y, left + s(88), y + s(26));
    paint::fill_rounded(
        dc,
        primary,
        radius,
        Corners::ALL,
        palette.accent,
        palette.content_background,
    );
    paint::text(dc, "Save", primary, body, palette.on_accent, Align::Center);
    let plain = rect(primary.right + s(8), y, primary.right + s(96), y + s(26));
    paint::fill_bordered(
        dc,
        plain,
        radius,
        palette.hover_background,
        palette.stroke,
        palette.content_background,
    );
    paint::text(
        dc,
        "Cancel",
        plain,
        body,
        palette.content_foreground,
        Align::Center,
    );
    let focused = rect(plain.right + s(8), y, plain.right + s(96), y + s(26));
    paint::fill_bordered(
        dc,
        focused,
        radius,
        palette.hover_background,
        palette.stroke,
        palette.content_background,
    );
    paint::text(
        dc,
        "Focused",
        focused,
        body,
        palette.content_foreground,
        Align::Center,
    );
    paint::focus_ring(
        dc,
        rect(
            focused.left - s(3),
            focused.top - s(3),
            focused.right + s(3),
            focused.bottom + s(3),
        ),
        palette,
        dpi,
    );
    y += s(44);
    paint::text(
        dc,
        "A list",
        rect(left, y, area.right, y + s(22)),
        heading,
        palette.content_foreground,
        Align::Left,
    );
    y += s(28);
    for (n, label) in ["A row", "The selected row", "A row under the pointer"]
        .iter()
        .enumerate()
    {
        let row = rect(left, y, s(420), y + s(ROW));
        let plate = rect(
            row.left + s(4),
            row.top + s(1),
            row.right - s(4),
            row.bottom - s(1),
        );
        let (fill, text) = match n {
            1 => (Some(palette.selection_background), palette.hover_foreground),
            2 => (Some(palette.hover_background), palette.hover_foreground),
            _ => (None, palette.content_foreground),
        };
        if let Some(fill) = fill {
            paint::fill_rounded(
                dc,
                plate,
                radius,
                Corners::ALL,
                fill,
                palette.content_background,
            );
        }
        if n == 1 && !palette.high_contrast {
            let bar = rect(
                plate.left,
                plate.top + s(4),
                plate.left + s(3),
                plate.bottom - s(4),
            );
            paint::fill_rounded(
                dc,
                bar,
                s(2),
                Corners::ALL,
                palette.accent,
                palette.selection_background,
            );
        }
        paint::text(
            dc,
            label,
            rect(plate.left + s(12), row.top, row.right, row.bottom),
            body,
            text,
            Align::Left,
        );
        y += s(ROW);
    }
    y += s(16);
    paint::text(
        dc,
        "Something went wrong: an error line.",
        rect(left, y, area.right, y + s(20)),
        body,
        palette.error_foreground,
        Align::Left,
    );
    let status = rect(0, area.bottom - s(22), area.right, area.bottom);
    paint::fill(dc, status, palette.strip_background);
    paint::text(
        dc,
        "Ctrl+Z undo · F5 refresh · Esc close",
        rect(s(16), status.top, status.right, status.bottom),
        body,
        palette.muted_foreground,
        Align::Left,
    );
    for font in [body, heading, title] {
        unsafe { DeleteObject(font as HGDIOBJ) };
    }
}

extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let handled = FRAME.with(|frame| {
        frame
            .borrow_mut()
            .as_mut()
            .and_then(|frame| frame.handle(window, message, wparam, lparam))
    });
    if let Some(answer) = handled {
        return answer;
    }
    if let Some(answer) = controls::colors(message, wparam, lparam, Surface::Content) {
        return answer;
    }
    if message == WM_NOTIFY
        && let Some(answer) = controls::list_rows(lparam, |_, _| None)
    {
        return answer;
    }
    match message {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let palette = theme::current();
            let dpi = unsafe { GetDpiForWindow(window) };
            let maximized = unsafe { IsZoomed(window) } != 0;
            paint::buffered(window, |dc, area| {
                draw(dc, area, &palette, dpi);
                FRAME.with(|frame| {
                    if let Some(frame) = frame.borrow().as_ref() {
                        frame.paint(dc, area.right, &palette, maximized);
                    }
                });
            });
            0
        }
        WM_SETTINGCHANGE => {
            if theme::refresh() {
                frame::apply_theme(window, &theme::current());
                controls::retheme(window);
                unsafe { InvalidateRect(window, std::ptr::null(), 0) };
            }
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// Real controls on the right, each painted by `controls`.
fn add_controls(window: HWND, dpi: u32) {
    let s = |value| scale(value, dpi);
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let font = ramp::font(TextStyle::Body, dpi);
    let make = |class: &str, text: &str, style: u32, extended: u32, x, y, width, height| {
        let class: Vec<u16> = class.encode_utf16().chain(std::iter::once(0)).collect();
        let text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let control = unsafe {
            CreateWindowExW(
                extended,
                class.as_ptr(),
                text.as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                s(x),
                s(y),
                s(width),
                s(height),
                window,
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        unsafe { SendMessageW(control, WM_SETFONT, font as usize, 0) };
        control
    };
    let local = make(
        "BUTTON",
        "A model on this PC",
        WS_TABSTOP | WS_GROUP | BS_AUTORADIOBUTTON as u32,
        0,
        460,
        60,
        190,
        26,
    );
    let cloud = make(
        "BUTTON",
        "A cloud API",
        BS_AUTORADIOBUTTON as u32,
        0,
        650,
        60,
        190,
        26,
    );
    controls::paint_as(local, Look::Segment(Segment::First), Surface::Content);
    controls::paint_as(cloud, Look::Segment(Segment::Last), Surface::Content);
    unsafe { SendMessageW(cloud, BM_SETCHECK, 1, 0) };
    let provider = make(
        "COMBOBOX",
        "",
        WS_TABSTOP | WS_GROUP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
        0,
        460,
        100,
        380,
        200,
    );
    for item in [
        "Claude API (Anthropic)",
        "OpenAI-compatible API",
        "GitHub Copilot (your Copilot plan)",
    ] {
        let item: Vec<u16> = item.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { SendMessageW(provider, CB_ADDSTRING, 0, item.as_ptr() as isize) };
    }
    unsafe { SendMessageW(provider, CB_SETCURSEL, 0, 0) };
    controls::paint_as(provider, Look::Combo, Surface::Content);
    let model = make(
        "COMBOBOX",
        "claude-sonnet-5-5",
        WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWN as u32,
        0,
        460,
        140,
        380,
        200,
    );
    let typed: Vec<u16> = "claude-sonnet-5-5\0".encode_utf16().collect();
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW(model, typed.as_ptr()) };
    controls::paint_as(model, Look::Combo, Surface::Content);
    let field = make(
        "EDIT",
        "C:\\Notes",
        WS_TABSTOP,
        WS_EX_CLIENTEDGE,
        460,
        180,
        380,
        26,
    );
    controls::paint_as(field, Look::Field, Surface::Content);
    let toggle = make(
        "BUTTON",
        "Start FastCapture when I sign in",
        WS_TABSTOP | BS_AUTOCHECKBOX as u32,
        0,
        460,
        220,
        380,
        28,
    );
    controls::paint_as(toggle, Look::Toggle, Surface::Content);
    unsafe { SendMessageW(toggle, BM_SETCHECK, 1, 0) };
    let off = make(
        "BUTTON",
        "Record the window I capture from",
        WS_TABSTOP | BS_AUTOCHECKBOX as u32,
        0,
        460,
        252,
        380,
        28,
    );
    controls::paint_as(off, Look::Toggle, Surface::Content);
    let test = make(
        "BUTTON",
        "Test",
        WS_TABSTOP | BS_PUSHBUTTON as u32,
        0,
        460,
        296,
        88,
        26,
    );
    controls::paint_as(test, Look::Button, Surface::Content);
    let save = make(
        "BUTTON",
        "Save",
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        656,
        296,
        88,
        26,
    );
    controls::paint_as(save, Look::Button, Surface::Content);
    let cancel = make(
        "BUTTON",
        "Cancel",
        WS_TABSTOP | BS_PUSHBUTTON as u32,
        0,
        752,
        296,
        88,
        26,
    );
    controls::paint_as(cancel, Look::Button, Surface::Content);
}

fn main() {
    let preference = match std::env::args().nth(1).as_deref() {
        Some("light") => ThemePreference::Light,
        Some("dark") => ThemePreference::Dark,
        _ => ThemePreference::System,
    };
    theme::set_preference(preference);
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let class: Vec<u16> = "FastCaptureLook\0".encode_utf16().collect();
    let title: Vec<u16> = "FastCapture look\0".encode_utf16().collect();
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let description = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    unsafe { RegisterClassW(&description) };
    let window = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            900,
            520,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    let dpi = unsafe { GetDpiForWindow(window) };
    FRAME.with(|frame| {
        *frame.borrow_mut() = Some(Frame::new(
            "FastCapture look",
            Buttons {
                minimize: true,
                maximize: true,
            },
            true,
            dpi,
        ))
    });
    frame::attach(window, &theme::current());
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetWindowPos(
            window,
            std::ptr::null_mut(),
            0,
            0,
            scale(900, dpi),
            scale(520, dpi),
            windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOZORDER,
        )
    };
    add_controls(window, dpi);
    if std::env::args().nth(2).as_deref() == Some("capture") {
        struct Nowhere;
        impl fastcapture::ui::capture_box::Sink for Nowhere {
            fn notebooks(&self) -> Vec<std::path::PathBuf> {
                vec![std::path::PathBuf::from(r"C:\Notes")]
            }
            fn file(
                &mut self,
                _: &fastcapture::capture::Capture,
                _: &std::path::Path,
            ) -> Result<(), String> {
                Ok(())
            }
            fn closed(&mut self) {
                unsafe { PostQuitMessage(0) };
            }
        }
        fastcapture::ui::capture_box::open(fastcapture::ui::capture_box::Open {
            sink: Box::new(Nowhere),
            notebook: std::path::PathBuf::from(r"C:\Notes"),
            draft_dir: None,
            source_window: Some("Grafana".to_owned()),
        });
    } else if std::env::args().nth(2).as_deref() == Some("home") {
        let option = |label: &str, detail: &str| fastcapture::file::home::HomeOption {
            label: label.to_owned(),
            detail: detail.to_owned(),
            target: fastcapture::file::Target::Folder(std::path::PathBuf::from(label)),
        };
        fastcapture::ui::home_picker::open(
            fastcapture::platform::window::SendHwnd::new(window),
            vec![
                option("Payments incidents", "topic in Payments/"),
                option("Payments/", "folder"),
                option("Ledger batch", "project in Payments/"),
                option("Learning/Rust/", "folder"),
            ],
            Box::new(|_| unsafe { PostQuitMessage(0) }),
        );
    } else {
        unsafe { ShowWindow(window, SW_SHOW) };
    }
    let mut message: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
        if fastcapture::ui::home_picker::dialog_message(&message)
            || unsafe { IsDialogMessageW(window, &message) } != 0
        {
            continue;
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
