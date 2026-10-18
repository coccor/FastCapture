//! The capture box, ported from FastPad `src/window/capture_box.rs`: a small
//! top-most window with a multi-line edit, a strip of attachments, and keys to save, close,
//! paste images, snip and pick a notebook. It files nothing itself: a `Sink` does. The first
//! half of the file is pure layout and key logic.
//!
//! Changes from FastPad's box: the title names the notebook and the mode, not a target; the
//! footer names what was detected ("Screenshot + file"); Ctrl+N picks a notebook for this
//! capture instead of cycling targets; there is no Ctrl+Shift+Enter (FastCapture has no editor
//! to open); files arrive through `WM_DROPFILES` rather than an OLE drop target; and the 20 MB
//! question is a message box.

use super::theme::{self, Palette, scale};
use crate::capture::detect;
use crate::capture::file::LARGE_FILE_BYTES;
use crate::capture::{Attachment, Capture, draft};
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DT_CALCRECT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
    DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect,
    GetDC, GetMonitorInfoW, HBRUSH, HDC, HFONT, InvalidateRect, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromPoint, PAINTSTRUCT, ReleaseDC, ScreenToClient, SelectObject,
    SetBkColor, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, RemoveClipboardFormatListener,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{EM_SETSEL, MARGINS, SetWindowTheme};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, SetFocus, VK_BACK, VK_CONTROL, VK_DELETE, VK_ESCAPE, VK_LEFT, VK_MENU,
    VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB,
};
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, DragAcceptFiles, DragFinish, HDROP, RemoveWindowSubclass, SetWindowSubclass,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    ES_AUTOVSCROLL, ES_MULTILINE, ES_WANTRETURN, FindWindowW, GWLP_USERDATA, GetCursorPos,
    GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    HTCAPTION, HTCLIENT, IDC_ARROW, IsWindow, KillTimer, LoadCursorW, MF_CHECKED, MF_STRING,
    RegisterClassW, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow,
    TPM_RETURNCMD, TrackPopupMenuEx, WM_APP, WM_CHAR, WM_CLIPBOARDUPDATE, WM_CLOSE,
    WM_CTLCOLOREDIT, WM_DESTROY, WM_DROPFILES, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_NCCALCSIZE, WM_NCDESTROY, WM_NCHITTEST, WM_PAINT, WM_PASTE, WM_SETFOCUS, WM_SETFONT,
    WM_TIMER, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_EX_ACCEPTFILES, WS_EX_TOPMOST,
    WS_POPUP, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

const WIDTH_AT_96_DPI: i32 = 520;
const PAD_AT_96_DPI: i32 = 12;
const TITLE_AT_96_DPI: i32 = 34;
const EDIT_AT_96_DPI: i32 = 92;
const CHIP_HEIGHT_AT_96_DPI: i32 = 30;
const CHIP_GAP_AT_96_DPI: i32 = 8;
const CHIP_MAX_AT_96_DPI: i32 = 220;
const CHIP_REMOVE_AT_96_DPI: i32 = 24;
const ERROR_AT_96_DPI: i32 = 22;
const FOOTER_AT_96_DPI: i32 = 30;

/// Posted to the box when the capture hotkey arrives while it is open: it comes to the front,
/// or back from a snip without an image.
pub const WM_CAPTURE_AGAIN: u32 = WM_APP + 0x60;
const SNIP_TIMER: usize = 0x4350_534E;
const SNIP_WAIT_MS: u32 = 30_000;
const EDIT_HOOK: usize = 0x4350_4544;

/// What a key in the box's edit does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Submit,
    Close,
    /// Ctrl+Shift+V: the clipboard's image even when it has text too.
    AttachImage,
    Snip,
    /// Ctrl+N: another notebook for this capture.
    PickNotebook,
    /// Tab: to the attachment strip.
    ToStrip,
}

pub fn key_action(key: u16, ctrl: bool, shift: bool, alt: bool) -> Option<Action> {
    match key {
        VK_RETURN if ctrl && !alt => Some(Action::Submit),
        VK_ESCAPE => Some(Action::Close),
        k if k == u16::from(b'V') && ctrl && shift && !alt => Some(Action::AttachImage),
        k if k == u16::from(b'S') && ctrl && shift && !alt => Some(Action::Snip),
        k if k == u16::from(b'N') && ctrl && !shift && !alt => Some(Action::PickNotebook),
        VK_TAB if !ctrl && !alt => Some(Action::ToStrip),
        _ => None,
    }
}

/// What Ctrl+V does: files copied in Explorer are attached; an image with no text beside it is
/// attached; anything else is pasted as text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Paste {
    Files,
    Image,
    Text,
}

pub fn paste_kind(files: bool, image: bool, text: bool) -> Paste {
    if files {
        Paste::Files
    } else if image && !text {
        Paste::Image
    } else {
        Paste::Text
    }
}

/// The box's parts, in client pixels.
#[derive(Clone, Default)]
pub struct Layout {
    pub width: i32,
    pub height: i32,
    pub title: RECT,
    pub close: RECT,
    pub edit: RECT,
    /// One per attachment, wrapped onto as many rows as they need.
    pub chips: Vec<RECT>,
    pub error: Option<RECT>,
    pub footer: RECT,
    pub snip: RECT,
}

/// Lays the box out for chips `chip_widths` pixels wide (their text), with an error line or not.
pub fn layout(dpi: u32, chip_widths: &[i32], has_error: bool, snip_width: i32) -> Layout {
    let s = |v: i32| scale(v, dpi);
    let width = s(WIDTH_AT_96_DPI);
    let pad = s(PAD_AT_96_DPI);
    let title = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: s(TITLE_AT_96_DPI),
    };
    let close = RECT {
        left: width - s(TITLE_AT_96_DPI),
        ..title
    };
    let edit = RECT {
        left: pad,
        top: title.bottom,
        right: width - pad,
        bottom: title.bottom + s(EDIT_AT_96_DPI),
    };
    let mut y = edit.bottom + s(CHIP_GAP_AT_96_DPI);
    let mut x = pad;
    let mut chips = Vec::new();
    for text in chip_widths {
        let chip = (text + s(CHIP_REMOVE_AT_96_DPI) + s(PAD_AT_96_DPI)).min(s(CHIP_MAX_AT_96_DPI));
        if x > pad && x + chip > width - pad {
            x = pad;
            y += s(CHIP_HEIGHT_AT_96_DPI) + s(CHIP_GAP_AT_96_DPI);
        }
        chips.push(RECT {
            left: x,
            top: y,
            right: x + chip,
            bottom: y + s(CHIP_HEIGHT_AT_96_DPI),
        });
        x += chip + s(CHIP_GAP_AT_96_DPI);
    }
    if !chips.is_empty() {
        y += s(CHIP_HEIGHT_AT_96_DPI) + s(CHIP_GAP_AT_96_DPI);
    }
    let error = has_error.then(|| {
        let line = RECT {
            left: pad,
            top: y,
            right: width - pad,
            bottom: y + s(ERROR_AT_96_DPI),
        };
        y = line.bottom;
        line
    });
    let footer = RECT {
        left: pad,
        top: y,
        right: width - pad,
        bottom: y + s(FOOTER_AT_96_DPI),
    };
    let snip = RECT {
        left: pad,
        top: footer.top,
        right: pad + snip_width,
        bottom: footer.bottom,
    };
    Layout {
        width,
        height: footer.bottom + s(4),
        title,
        close,
        edit,
        chips,
        error,
        footer,
        snip,
    }
}

/// The title band: where the capture goes, and which mode (M2 adds the mode).
pub fn title_text(notebook: &str) -> String {
    format!("Capture \u{2192} {notebook}")
}

/// The window's accessible name.
pub fn accessible_name(notebook: &str) -> String {
    format!("Capture to {notebook}")
}

/// The footer: what the capture holds, then the keys.
pub fn footer_text(capture: &Capture) -> String {
    format!(
        "{} \u{00B7} Ctrl+Enter save \u{00B7} Esc close",
        detect::label(&detect::kinds(capture))
    )
}

/// An attachment's label in the strip.
pub fn chip_label(attachment: &Attachment, index: usize) -> String {
    match attachment {
        Attachment::Png(_) => format!("Image {}", index + 1),
        Attachment::File(path) => path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    }
}

/// A notebook's name in the title and the picker.
pub fn notebook_name(notebook: &Path) -> String {
    notebook
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| notebook.display().to_string())
}

/// Whoever files the box's captures.
pub trait Sink {
    /// The notebooks the picker offers, most recent first.
    fn notebooks(&self) -> Vec<PathBuf>;
    /// Files the capture into `notebook`; the error is shown in the box, which stays open.
    fn file(&mut self, capture: &Capture, notebook: &Path) -> Result<(), String>;
    /// The box closed (saved or not).
    fn closed(&mut self) {}
}

// ---------------------------------------------------------------------------------------------
// The window.

struct CaptureBox {
    sink: Box<dyn Sink>,
    colors: Palette,
    dpi: u32,
    edit: HWND,
    edit_brush: HBRUSH,
    title_font: HFONT,
    body_font: HFONT,
    items: Vec<Attachment>,
    /// The strip's keyboard selection: an attachment, or `items.len()` for the Snip button.
    selected: usize,
    notebook: PathBuf,
    source_window: Option<String>,
    draft_dir: Option<PathBuf>,
    error: Option<String>,
    layout: Layout,
    snipping: bool,
    /// The WM_CHAR of a Ctrl+Shift+V that attached an image must not paste text too.
    swallow_paste_char: bool,
}

const SNIP_LABEL: &str = "+ Snip (Ctrl+Shift+S)";

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn class_name() -> &'static [u16] {
    static NAME: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| wide("FastCaptureBox"))
}

fn register_class() -> bool {
    static REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *REGISTERED.get_or_init(|| {
        let class = WNDCLASSW {
            lpfnWndProc: Some(box_proc),
            hInstance: unsafe { GetModuleHandleW(std::ptr::null()) },
            hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
            lpszClassName: class_name().as_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        unsafe { RegisterClassW(&class) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS }
    })
}

/// The open capture box, if any.
pub fn find() -> Option<HWND> {
    let hwnd = unsafe { FindWindowW(class_name().as_ptr(), std::ptr::null()) };
    (!hwnd.is_null()).then_some(hwnd)
}

fn state(hwnd: HWND) -> Option<&'static mut CaptureBox> {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut CaptureBox;
    (!pointer.is_null()).then(|| unsafe { &mut *pointer })
}

/// What the box opens with.
pub struct Open {
    pub sink: Box<dyn Sink>,
    pub notebook: PathBuf,
    /// Where the draft lives; `None` keeps no draft.
    pub draft_dir: Option<PathBuf>,
    /// The window that was in front when the hotkey was pressed.
    pub source_window: Option<String>,
}

/// Opens the box centered on the monitor under the mouse, with the last draft. Returns the
/// window.
pub fn open(options: Open) -> Option<HWND> {
    if !register_class() {
        return None;
    }
    let mut cursor = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut cursor) };
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_ACCEPTFILES,
            class_name().as_ptr(),
            std::ptr::null(),
            // WS_CAPTION gives the DWM shadow; WM_NCCALCSIZE hides the frame, as FastPad does.
            WS_POPUP | WS_CAPTION | WS_CLIPCHILDREN,
            cursor.x,
            cursor.y,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        return None;
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let edit = unsafe {
        CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            std::ptr::null(),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WS_VSCROLL
                | (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32,
            0,
            0,
            0,
            0,
            hwnd,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    let colors = theme::current();
    let body_font = theme::font(10, false, dpi);
    let title_font = theme::font(11, true, dpi);
    unsafe {
        SendMessageW(edit, WM_SETFONT, body_font as usize, 0);
        SetWindowSubclass(edit, Some(edit_proc), EDIT_HOOK, hwnd as usize);
        if colors.dark {
            SetWindowTheme(edit, wide("DarkMode_Explorer").as_ptr(), std::ptr::null());
        }
        DragAcceptFiles(hwnd, 1);
    }
    let restored = options
        .draft_dir
        .as_deref()
        .and_then(draft::load)
        .unwrap_or_default();
    let state = Box::new(CaptureBox {
        sink: options.sink,
        colors,
        dpi,
        edit,
        edit_brush: unsafe { CreateSolidBrush(colors.surface) },
        title_font,
        body_font,
        items: restored.attachments,
        selected: 0,
        notebook: options.notebook,
        source_window: options.source_window,
        draft_dir: options.draft_dir,
        error: None,
        layout: Layout::default(),
        snipping: false,
        swallow_paste_char: false,
    });
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize) };
    set_edit_text(edit, &restored.text);
    relayout(hwnd);
    place(hwnd, cursor);
    unsafe {
        let corners = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&raw const corners).cast(),
            size_of_val(&corners) as u32,
        );
        let dark: i32 = colors.dark.into();
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&raw const dark).cast(),
            size_of_val(&dark) as u32,
        );
        let margins = MARGINS {
            cxLeftWidth: 1,
            cxRightWidth: 1,
            cyTopHeight: 1,
            cyBottomHeight: 1,
        };
        DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
    show(hwnd);
    Some(hwnd)
}

fn show(hwnd: HWND) {
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
    if let Some(capture) = state(hwnd) {
        let edit = capture.edit;
        unsafe {
            SetFocus(edit);
            // The caret goes to the end of a restored draft.
            SendMessageW(edit, EM_SETSEL, usize::MAX, -1);
        }
    }
}

/// Centers the box on the work area of the monitor at `point`, a third of the way down.
fn place(hwnd: HWND, point: POINT) {
    let Some(capture) = state(hwnd) else {
        return;
    };
    let (width, height) = (capture.layout.width, capture.layout.height);
    let mut monitor: MONITORINFO = unsafe { std::mem::zeroed() };
    monitor.cbSize = size_of::<MONITORINFO>() as u32;
    unsafe {
        GetMonitorInfoW(
            MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        )
    };
    let work = monitor.rcWork;
    let left = work.left + ((work.right - work.left) - width).max(0) / 2;
    let top = work.top + ((work.bottom - work.top) - height).max(0) / 3;
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            left,
            top,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

fn text_width(hwnd: HWND, font: HFONT, text: &str) -> i32 {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe {
        let dc = GetDC(hwnd);
        let old = SelectObject(dc, font);
        DrawTextW(
            dc,
            wide.as_ptr(),
            wide.len() as i32,
            &mut rect,
            DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(dc, old);
        ReleaseDC(hwnd, dc);
    }
    rect.right - rect.left
}

/// Lays the box out again for its attachments and error, keeping its top-left corner.
fn relayout(hwnd: HWND) {
    let Some(capture) = state(hwnd) else {
        return;
    };
    let widths: Vec<i32> = capture
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| text_width(hwnd, capture.body_font, &chip_label(item, index)))
        .collect();
    let snip = text_width(hwnd, capture.body_font, SNIP_LABEL) + scale(8, capture.dpi);
    capture.layout = layout(capture.dpi, &widths, capture.error.is_some(), snip);
    capture.selected = capture.selected.min(capture.items.len());
    let edit = capture.layout.edit;
    let (width, height) = (capture.layout.width, capture.layout.height);
    let name = notebook_name(&capture.notebook);
    unsafe {
        SetWindowPos(
            capture.edit,
            std::ptr::null_mut(),
            edit.left,
            edit.top,
            edit.right - edit.left,
            edit.bottom - edit.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let mut frame: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut frame);
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            frame.left,
            frame.top,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        SetWindowTextW(hwnd, wide(&accessible_name(&name)).as_ptr());
        InvalidateRect(hwnd, std::ptr::null(), 1);
    }
}

fn set_edit_text(edit: HWND, text: &str) {
    unsafe { SetWindowTextW(edit, wide(text).as_ptr()) };
}

fn edit_text(edit: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(edit) }.max(0) as usize;
    let mut buffer = vec![0_u16; length + 1];
    let copied = unsafe { GetWindowTextW(edit, buffer.as_mut_ptr(), buffer.len() as i32) };
    // The edit control's line breaks are CRLF; captures use LF.
    String::from_utf16_lossy(&buffer[..copied.max(0) as usize]).replace("\r\n", "\n")
}

/// What the box holds now.
fn current_capture(capture: &CaptureBox) -> Capture {
    Capture {
        text: edit_text(capture.edit),
        attachments: capture.items.clone(),
        source_window: capture.source_window.clone(),
    }
}

/// Shows `error` in the box (or clears it).
fn set_error(hwnd: HWND, error: Option<String>) {
    if let Some(capture) = state(hwnd) {
        capture.error = error;
    }
    relayout(hwnd);
}

/// Attaches `attachment`; a folder is refused and a file over 20 MB asks first.
fn attach(hwnd: HWND, attachment: Attachment) {
    if let Attachment::File(path) = &attachment {
        if path.is_dir() {
            set_error(
                hwnd,
                Some(format!(
                    "{} is a folder. Only files can be attached.",
                    path.display()
                )),
            );
            return;
        }
        let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
        if size > LARGE_FILE_BYTES && !confirm_large(hwnd, path, size) {
            return;
        }
    }
    if let Some(capture) = state(hwnd) {
        capture.items.push(attachment);
        capture.error = None;
    }
    relayout(hwnd);
}

#[cfg(test)]
thread_local! {
    /// What tests answer to the 20 MB question.
    static ALLOW_LARGE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// How many snips tests started.
    static SNIPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn confirm_large(hwnd: HWND, path: &Path, size: u64) -> bool {
    #[cfg(test)]
    {
        let _ = (hwnd, path, size);
        ALLOW_LARGE.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        let message = format!(
            "{} is {} MB. Copy it into the notebook anyway?",
            path.file_name().unwrap_or_default().to_string_lossy(),
            size.div_ceil(1024 * 1024)
        );
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            IDNO, MB_ICONQUESTION, MB_YESNO, MessageBoxW,
        };
        unsafe {
            MessageBoxW(
                hwnd,
                wide(&message).as_ptr(),
                wide("FastCapture").as_ptr(),
                MB_YESNO | MB_ICONQUESTION,
            ) != IDNO
        }
    }
}

/// Ctrl+V and the edit's Paste: files and lone images are attached. Returns whether the paste
/// was taken, so the edit doesn't paste text too.
fn paste(hwnd: HWND, force_image: bool) -> bool {
    use crate::platform::clipboard;
    let kind = if force_image {
        if clipboard::has_image() {
            Paste::Image
        } else {
            return false;
        }
    } else {
        paste_kind(
            clipboard::has_files(),
            clipboard::has_image(),
            clipboard::has_text(),
        )
    };
    match kind {
        Paste::Text => false,
        Paste::Files => {
            for path in clipboard::files(hwnd) {
                attach(hwnd, Attachment::File(path));
            }
            true
        }
        Paste::Image => {
            match clipboard::image_png(hwnd) {
                Some(Ok(png)) => attach(hwnd, Attachment::Png(png)),
                Some(Err(error)) => set_error(hwnd, Some(error)),
                None => {}
            }
            true
        }
    }
}

fn remove_item(hwnd: HWND, index: usize) {
    if let Some(capture) = state(hwnd)
        && index < capture.items.len()
    {
        capture.items.remove(index);
    }
    relayout(hwnd);
}

/// Ctrl+N: a menu of the recent notebooks, the current one checked, for this capture only.
fn pick_notebook(hwnd: HWND) {
    let Some(capture) = state(hwnd) else {
        return;
    };
    let mut notebooks = capture.sink.notebooks();
    if !notebooks.contains(&capture.notebook) {
        notebooks.insert(0, capture.notebook.clone());
    }
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return;
    }
    for (index, notebook) in notebooks.iter().enumerate() {
        let flags = if *notebook == capture.notebook {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        let label = wide(&format!("&{} {}", index + 1, notebook_name(notebook)));
        unsafe { AppendMenuW(menu, flags, index + 1, label.as_ptr()) };
    }
    let title = capture.layout.title;
    let mut at = POINT {
        x: scale(PAD_AT_96_DPI, capture.dpi),
        y: title.bottom,
    };
    unsafe { windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut at) };
    let chosen = unsafe {
        let chosen = TrackPopupMenuEx(menu, TPM_RETURNCMD, at.x, at.y, hwnd, std::ptr::null());
        DestroyMenu(menu);
        chosen
    };
    if chosen > 0
        && let Some(notebook) = notebooks.get(chosen as usize - 1)
        && let Some(capture) = state(hwnd)
    {
        capture.notebook = notebook.clone();
        relayout(hwnd);
    }
    if let Some(capture) = state(hwnd) {
        unsafe { SetFocus(capture.edit) };
    }
}

/// Snip: the box hides, Windows' own snipping overlay starts, and the next image on the
/// clipboard is attached when the box comes back. 30 seconds without one brings it back.
fn start_snip(hwnd: HWND) {
    if let Some(capture) = state(hwnd) {
        capture.snipping = true;
    }
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
        AddClipboardFormatListener(hwnd);
        SetTimer(hwnd, SNIP_TIMER, SNIP_WAIT_MS, None);
    }
    #[cfg(test)]
    SNIPS.with(|snips| snips.set(snips.get() + 1));
    #[cfg(not(test))]
    if let Err(error) = crate::platform::shell::open(Path::new("ms-screenclip:")) {
        crate::log::line(&format!("snip: {error}"));
    }
}

fn end_snip(hwnd: HWND) {
    let was = state(hwnd).is_some_and(|capture| std::mem::take(&mut capture.snipping));
    if !was {
        return;
    }
    unsafe {
        KillTimer(hwnd, SNIP_TIMER);
        RemoveClipboardFormatListener(hwnd);
    }
    show(hwnd);
}

/// Files the capture; on success the draft goes and the box closes.
fn submit(hwnd: HWND) {
    let Some(capture) = state(hwnd) else {
        return;
    };
    let submitted = current_capture(capture);
    if submitted.is_empty() {
        close(hwnd);
        return;
    }
    let notebook = capture.notebook.clone();
    match capture.sink.file(&submitted, &notebook) {
        Ok(()) => {
            if let Some(capture) = state(hwnd) {
                if let Some(dir) = &capture.draft_dir {
                    draft::clear(dir);
                }
                capture.items.clear();
                set_edit_text(capture.edit, "");
            }
            unsafe { DestroyWindow(hwnd) };
        }
        Err(error) => {
            if let Some(dir) = &capture.draft_dir {
                let _ = draft::save(dir, &submitted);
            }
            set_error(hwnd, Some(error));
        }
    }
}

/// Esc: closes the box, keeping what it holds as a draft.
fn close(hwnd: HWND) {
    if let Some(capture) = state(hwnd)
        && let Some(dir) = &capture.draft_dir
        && let Err(error) = draft::save(dir, &current_capture(capture))
    {
        crate::log::line(&format!("draft: {error}"));
    }
    unsafe { DestroyWindow(hwnd) };
}

fn modifiers() -> (bool, bool, bool) {
    let down = |key: u16| unsafe { GetKeyState(i32::from(key)) } < 0;
    (down(VK_CONTROL), down(VK_SHIFT), down(VK_MENU))
}

fn run(hwnd: HWND, action: Action) {
    match action {
        Action::Submit => submit(hwnd),
        Action::Close => close(hwnd),
        Action::AttachImage => {
            if paste(hwnd, true)
                && let Some(capture) = state(hwnd)
            {
                capture.swallow_paste_char = true;
            }
        }
        Action::Snip => start_snip(hwnd),
        Action::PickNotebook => pick_notebook(hwnd),
        Action::ToStrip => {
            if let Some(capture) = state(hwnd) {
                capture.selected = 0;
            }
            unsafe { SetFocus(hwnd) };
            invalidate(hwnd);
        }
    }
    // The footer follows what the box holds.
    if unsafe { IsWindow(hwnd) } != 0 {
        invalidate(hwnd);
    }
}

fn invalidate(hwnd: HWND) {
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
}

unsafe extern "system" fn edit_proc(
    edit: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    owner: usize,
) -> LRESULT {
    let hwnd = owner as HWND;
    match message {
        WM_KEYDOWN => {
            let (ctrl, shift, alt) = modifiers();
            if let Some(action) = key_action(wparam as u16, ctrl, shift, alt) {
                run(hwnd, action);
                return 0;
            }
        }
        WM_CHAR => {
            let ch = wparam as u32;
            // Ctrl+Enter's line feed, Esc, Tab and Ctrl+N never reach the text.
            if ch == 0x0A || ch == 0x1B || ch == 0x09 || ch == 0x0E {
                return 0;
            }
            if ch == 0x16
                && let Some(capture) = state(hwnd)
                && std::mem::take(&mut capture.swallow_paste_char)
            {
                return 0;
            }
            // Typing changes what the footer says the capture holds.
            invalidate(hwnd);
        }
        WM_PASTE => {
            if paste(hwnd, false) {
                return 0;
            }
            invalidate(hwnd);
        }
        WM_NCDESTROY => unsafe {
            RemoveWindowSubclass(edit, Some(edit_proc), EDIT_HOOK);
        },
        _ => {}
    }
    unsafe { DefSubclassProc(edit, message, wparam, lparam) }
}

fn draw_text(dc: HDC, text: &str, rect: &RECT, format: u32) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut rect = *rect;
    unsafe {
        DrawTextW(
            dc,
            wide.as_ptr(),
            wide.len() as i32,
            &mut rect,
            format | DT_NOPREFIX | DT_SINGLELINE | DT_VCENTER,
        )
    };
}

fn fill(dc: HDC, rect: &RECT, color: u32) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(dc, rect, brush);
        DeleteObject(brush);
    }
}

fn frame(dc: HDC, rect: &RECT, color: u32) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FrameRect(dc, rect, brush);
        DeleteObject(brush);
    }
}

fn paint(hwnd: HWND) {
    let Some(capture) = state(hwnd) else {
        return;
    };
    let mut ps: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    let dc = unsafe { BeginPaint(hwnd, &mut ps) };
    let colors = capture.colors;
    let layout = capture.layout.clone();
    let client = RECT {
        left: 0,
        top: 0,
        right: layout.width,
        bottom: layout.height,
    };
    let strip_focused = unsafe { GetFocus() } == hwnd;
    fill(dc, &client, colors.background);
    unsafe { SetBkMode(dc, TRANSPARENT as i32) };
    let pad = scale(PAD_AT_96_DPI, capture.dpi);
    let old = unsafe { SelectObject(dc, capture.title_font) };
    unsafe { SetTextColor(dc, colors.text) };
    draw_text(
        dc,
        &title_text(&notebook_name(&capture.notebook)),
        &RECT {
            left: pad,
            right: layout.close.left,
            ..layout.title
        },
        DT_LEFT | DT_END_ELLIPSIS,
    );
    unsafe { SelectObject(dc, capture.body_font) };
    unsafe { SetTextColor(dc, colors.muted) };
    draw_text(dc, "\u{2715}", &layout.close, DT_CENTER);
    for (index, chip) in layout.chips.iter().enumerate() {
        let selected = strip_focused && capture.selected == index;
        fill(
            dc,
            chip,
            if selected {
                colors.selection
            } else {
                colors.surface
            },
        );
        frame(
            dc,
            chip,
            if selected {
                colors.accent
            } else {
                colors.stroke
            },
        );
        let remove = scale(CHIP_REMOVE_AT_96_DPI, capture.dpi);
        unsafe { SetTextColor(dc, colors.text) };
        if let Some(item) = capture.items.get(index) {
            draw_text(
                dc,
                &chip_label(item, index),
                &RECT {
                    left: chip.left + pad / 2,
                    right: chip.right - remove,
                    ..*chip
                },
                DT_LEFT | DT_END_ELLIPSIS,
            );
        }
        unsafe { SetTextColor(dc, colors.muted) };
        draw_text(
            dc,
            "\u{2715}",
            &RECT {
                left: chip.right - remove,
                ..*chip
            },
            DT_CENTER,
        );
    }
    if let (Some(rect), Some(error)) = (layout.error, capture.error.as_deref()) {
        unsafe { SetTextColor(dc, colors.error) };
        draw_text(dc, error, &rect, DT_LEFT | DT_END_ELLIPSIS);
    }
    if strip_focused && capture.selected == capture.items.len() {
        frame(dc, &layout.snip, colors.accent);
    }
    unsafe { SetTextColor(dc, colors.accent) };
    draw_text(
        dc,
        SNIP_LABEL,
        &RECT {
            left: layout.snip.left + scale(4, capture.dpi),
            ..layout.snip
        },
        DT_LEFT,
    );
    unsafe { SetTextColor(dc, colors.muted) };
    draw_text(
        dc,
        &footer_text(&current_capture(capture)),
        &RECT {
            left: layout.snip.right + pad,
            ..layout.footer
        },
        DT_RIGHT | DT_END_ELLIPSIS,
    );
    unsafe {
        SelectObject(dc, old);
        EndPaint(hwnd, &ps);
    }
}

fn contains(rect: &RECT, x: i32, y: i32) -> bool {
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}

fn clicked(hwnd: HWND, x: i32, y: i32) {
    let Some(capture) = state(hwnd) else {
        return;
    };
    let layout = capture.layout.clone();
    if contains(&layout.close, x, y) {
        close(hwnd);
        return;
    }
    if contains(&layout.snip, x, y) {
        start_snip(hwnd);
        return;
    }
    let remove = scale(CHIP_REMOVE_AT_96_DPI, capture.dpi);
    if let Some(index) = layout.chips.iter().position(|chip| {
        contains(
            &RECT {
                left: chip.right - remove,
                ..*chip
            },
            x,
            y,
        )
    }) {
        remove_item(hwnd, index);
    }
}

/// Keys while the strip (the box itself) has the focus.
fn strip_key(hwnd: HWND, key: u16) {
    let Some(capture) = state(hwnd) else {
        return;
    };
    let count = capture.items.len();
    let (ctrl, _, _) = modifiers();
    match key {
        VK_TAB => unsafe {
            SetFocus(capture.edit);
        },
        VK_LEFT => capture.selected = capture.selected.saturating_sub(1),
        VK_RIGHT => capture.selected = (capture.selected + 1).min(count),
        VK_DELETE | VK_BACK if capture.selected < count => remove_item(hwnd, capture.selected),
        VK_RETURN if ctrl => submit(hwnd),
        VK_RETURN | VK_SPACE if capture.selected == count => start_snip(hwnd),
        VK_ESCAPE => close(hwnd),
        _ => {}
    }
    if unsafe { IsWindow(hwnd) } != 0 {
        invalidate(hwnd);
    }
}

fn dropped(hwnd: HWND, drop: HDROP) {
    let paths = crate::platform::clipboard::dropped_paths(drop);
    unsafe { DragFinish(drop) };
    for path in paths {
        attach(hwnd, Attachment::File(path));
    }
}

unsafe extern "system" fn box_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } == 0 {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    match message {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_NCCALCSIZE => 0,
        WM_NCHITTEST => {
            let mut point = POINT {
                x: (lparam & 0xFFFF) as i16 as i32,
                y: ((lparam >> 16) & 0xFFFF) as i16 as i32,
            };
            unsafe { ScreenToClient(hwnd, &mut point) };
            let drags = state(hwnd).is_some_and(|capture| {
                contains(&capture.layout.title, point.x, point.y)
                    && !contains(&capture.layout.close, point.x, point.y)
            });
            if drags {
                HTCAPTION as LRESULT
            } else {
                HTCLIENT as LRESULT
            }
        }
        WM_LBUTTONDOWN => {
            clicked(
                hwnd,
                (lparam & 0xFFFF) as i16 as i32,
                ((lparam >> 16) & 0xFFFF) as i16 as i32,
            );
            0
        }
        WM_KEYDOWN => {
            strip_key(hwnd, wparam as u16);
            0
        }
        WM_SETFOCUS => {
            invalidate(hwnd);
            0
        }
        WM_CTLCOLOREDIT => {
            let Some(capture) = state(hwnd) else {
                return 0;
            };
            let dc = wparam as HDC;
            unsafe {
                SetTextColor(dc, capture.colors.text);
                SetBkColor(dc, capture.colors.surface);
            }
            capture.edit_brush as LRESULT
        }
        WM_CAPTURE_AGAIN => {
            if state(hwnd).is_some_and(|capture| capture.snipping) {
                // The hotkey during a snip brings the box back without an image.
                end_snip(hwnd);
            } else if unsafe { GetForegroundWindow() } != hwnd {
                show(hwnd);
            }
            0
        }
        WM_DROPFILES => {
            dropped(hwnd, wparam as HDROP);
            0
        }
        WM_CLIPBOARDUPDATE => {
            if state(hwnd).is_some_and(|capture| capture.snipping)
                && crate::platform::clipboard::has_image()
            {
                end_snip(hwnd);
                let _ = paste(hwnd, true);
            }
            0
        }
        WM_TIMER if wparam == SNIP_TIMER => {
            end_snip(hwnd);
            0
        }
        WM_CLOSE => {
            close(hwnd);
            0
        }
        WM_DESTROY => {
            unsafe {
                KillTimer(hwnd, SNIP_TIMER);
                RemoveClipboardFormatListener(hwnd);
            }
            if let Some(capture) = state(hwnd) {
                capture.sink.closed();
            }
            0
        }
        WM_NCDESTROY => {
            let pointer = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut CaptureBox;
            if !pointer.is_null() {
                let capture = unsafe { Box::from_raw(pointer) };
                unsafe {
                    DeleteObject(capture.edit_brush);
                    DeleteObject(capture.title_font);
                    DeleteObject(capture.body_font);
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

// ---------------------------------------------------------------------------------------------
// Tests.

/// Types `text` into the box, for tests elsewhere.
#[cfg(test)]
pub(crate) fn set_text_for_test(hwnd: HWND, text: &str) {
    if let Some(capture) = state(hwnd) {
        set_edit_text(capture.edit, text);
    }
}

/// Runs a key's action, for tests elsewhere.
#[cfg(test)]
pub(crate) fn run_for_test(hwnd: HWND, action: Action) {
    run(hwnd, action);
}

/// Sends `WM_PASTE` to the box's edit, as Ctrl+V does.
#[cfg(test)]
pub(crate) fn paste_for_test(hwnd: HWND) {
    if let Some(capture) = state(hwnd) {
        unsafe { SendMessageW(capture.edit, WM_PASTE, 0, 0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn keys_map_to_the_box_actions() {
        assert_eq!(
            key_action(VK_RETURN, true, false, false),
            Some(Action::Submit)
        );
        assert_eq!(
            key_action(VK_RETURN, false, false, false),
            None,
            "Enter is a new line"
        );
        assert_eq!(
            key_action(VK_ESCAPE, false, false, false),
            Some(Action::Close)
        );
        assert_eq!(
            key_action(u16::from(b'V'), true, true, false),
            Some(Action::AttachImage)
        );
        assert_eq!(
            key_action(u16::from(b'V'), true, false, false),
            None,
            "the edit pastes"
        );
        assert_eq!(
            key_action(u16::from(b'S'), true, true, false),
            Some(Action::Snip)
        );
        assert_eq!(
            key_action(u16::from(b'N'), true, false, false),
            Some(Action::PickNotebook)
        );
        assert_eq!(
            key_action(u16::from(b'N'), true, false, true),
            None,
            "Ctrl+Alt+N is FastPad's capture key"
        );
        assert_eq!(
            key_action(VK_TAB, false, false, false),
            Some(Action::ToStrip)
        );
    }

    #[test]
    fn ctrl_v_attaches_files_and_lone_images_and_pastes_text_otherwise() {
        assert_eq!(paste_kind(true, false, true), Paste::Files);
        assert_eq!(paste_kind(false, true, false), Paste::Image);
        assert_eq!(
            paste_kind(false, true, true),
            Paste::Text,
            "Word puts a picture beside copied text"
        );
        assert_eq!(paste_kind(false, false, true), Paste::Text);
    }

    #[test]
    fn the_strip_appears_only_with_attachments_and_wraps() {
        let plain = layout(96, &[], false, 120);
        assert_eq!(plain.width, 520);
        assert!(plain.chips.is_empty());
        assert_eq!(plain.footer.top, plain.edit.bottom + 8);

        let with = layout(96, &[200, 200, 200], true, 120);
        assert_eq!(with.chips.len(), 3);
        assert_eq!(with.chips[0].top, with.chips[1].top);
        assert!(with.chips[2].top > with.chips[1].top, "the third wraps");
        assert!(with.chips.iter().all(|chip| chip.right <= 520 - 12));
        let error = with.error.unwrap();
        assert!(error.top >= with.chips[2].bottom);
        assert!(with.height > plain.height);

        assert_eq!(layout(192, &[], false, 240).width, 1040);
    }

    #[test]
    fn the_title_footer_and_chips_say_what_and_where() {
        assert_eq!(title_text("Work notes"), "Capture \u{2192} Work notes");
        assert_eq!(accessible_name("Work notes"), "Capture to Work notes");
        assert_eq!(notebook_name(Path::new(r"D:\Work notes")), "Work notes");
        assert_eq!(chip_label(&Attachment::Png(Vec::new()), 1), "Image 2");
        assert_eq!(
            chip_label(&Attachment::File(PathBuf::from(r"C:\logs\ledger.log")), 0),
            "ledger.log"
        );
        let capture = Capture {
            text: "lag".to_owned(),
            attachments: vec![
                Attachment::Png(Vec::new()),
                Attachment::File(PathBuf::from("a.log")),
            ],
            source_window: None,
        };
        assert_eq!(
            footer_text(&capture),
            "Text + screenshot + file \u{00B7} Ctrl+Enter save \u{00B7} Esc close"
        );
        assert!(footer_text(&Capture::default()).starts_with("Empty"));
    }

    /// What a test sink filed, and into which notebook.
    type Filed = Rc<RefCell<Vec<(Capture, PathBuf)>>>;

    /// Files into a list, or fails with `fail` when set.
    struct Recorder {
        filed: Filed,
        fail: Option<String>,
        notebooks: Vec<PathBuf>,
    }

    impl Sink for Recorder {
        fn notebooks(&self) -> Vec<PathBuf> {
            self.notebooks.clone()
        }

        fn file(&mut self, capture: &Capture, notebook: &Path) -> Result<(), String> {
            if let Some(error) = &self.fail {
                return Err(error.clone());
            }
            self.filed
                .borrow_mut()
                .push((capture.clone(), notebook.to_path_buf()));
            Ok(())
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fastcapture-box-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn open_box(draft_dir: &Path, fail: Option<&str>) -> (HWND, Filed) {
        let filed = Rc::new(RefCell::new(Vec::new()));
        let hwnd = open(Open {
            sink: Box::new(Recorder {
                filed: filed.clone(),
                fail: fail.map(str::to_owned),
                notebooks: vec![PathBuf::from(r"D:\Notes")],
            }),
            notebook: PathBuf::from(r"D:\Notes"),
            draft_dir: Some(draft_dir.to_path_buf()),
            source_window: Some("Grafana".to_owned()),
        })
        .unwrap();
        (hwnd, filed)
    }

    #[test]
    fn ctrl_enter_files_what_the_box_holds_and_closes_it() {
        let drafts = temp("submit");
        let (hwnd, filed) = open_box(&drafts, None);
        set_edit_text(state(hwnd).unwrap().edit, "line one\r\nline two");
        attach(hwnd, Attachment::Png(b"png".to_vec()));
        run(hwnd, Action::Submit);
        assert_eq!(unsafe { IsWindow(hwnd) }, 0, "the box closed");
        let filed = filed.borrow();
        assert_eq!(filed.len(), 1);
        assert_eq!(filed[0].0.text, "line one\nline two");
        assert_eq!(filed[0].0.attachments, [Attachment::Png(b"png".to_vec())]);
        assert_eq!(filed[0].0.source_window.as_deref(), Some("Grafana"));
        assert_eq!(filed[0].1, PathBuf::from(r"D:\Notes"));
        assert_eq!(draft::load(&drafts), None);
    }

    #[test]
    fn esc_keeps_a_draft_that_the_next_box_brings_back() {
        let drafts = temp("draft");
        let (hwnd, filed) = open_box(&drafts, None);
        set_edit_text(state(hwnd).unwrap().edit, "half a thought");
        run(hwnd, Action::Close);
        assert!(filed.borrow().is_empty());
        let (hwnd, _) = open_box(&drafts, None);
        assert_eq!(edit_text(state(hwnd).unwrap().edit), "half a thought");
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
    }

    #[test]
    fn a_failure_stays_in_the_box_with_its_error() {
        let drafts = temp("fail");
        let (hwnd, _) = open_box(&drafts, Some("The notebook is read-only."));
        set_edit_text(state(hwnd).unwrap().edit, "keep me");
        run(hwnd, Action::Submit);
        assert_ne!(unsafe { IsWindow(hwnd) }, 0, "still open");
        assert_eq!(
            state(hwnd).unwrap().error.as_deref(),
            Some("The notebook is read-only.")
        );
        assert_eq!(draft::load(&drafts).unwrap().text, "keep me");
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
    }

    #[test]
    fn folders_are_refused_and_large_files_ask_first() {
        let drafts = temp("attach");
        std::fs::create_dir_all(&drafts).unwrap();
        let big = drafts.join("big.bin");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(LARGE_FILE_BYTES + 1)
            .unwrap();
        let (hwnd, _) = open_box(&drafts.join("draft"), None);
        attach(hwnd, Attachment::File(drafts.clone()));
        assert!(state(hwnd).unwrap().error.is_some());
        assert!(state(hwnd).unwrap().items.is_empty());
        ALLOW_LARGE.with(|allow| allow.set(false));
        attach(hwnd, Attachment::File(big.clone()));
        assert!(state(hwnd).unwrap().items.is_empty(), "declined");
        ALLOW_LARGE.with(|allow| allow.set(true));
        attach(hwnd, Attachment::File(big.clone()));
        assert_eq!(state(hwnd).unwrap().items.len(), 1);
        assert_eq!(
            state(hwnd).unwrap().error,
            None,
            "a good attach clears the error"
        );
        remove_item(hwnd, 0);
        assert!(state(hwnd).unwrap().items.is_empty());
        unsafe { DestroyWindow(hwnd) };
        let _ = std::fs::remove_dir_all(&drafts);
    }

    #[test]
    fn a_snip_hides_the_box_and_the_hotkey_brings_it_back() {
        let drafts = temp("snip");
        let (hwnd, _) = open_box(&drafts, None);
        let before = SNIPS.with(std::cell::Cell::get);
        run(hwnd, Action::Snip);
        assert_eq!(SNIPS.with(std::cell::Cell::get), before + 1);
        assert!(state(hwnd).unwrap().snipping);
        unsafe { SendMessageW(hwnd, WM_CAPTURE_AGAIN, 0, 0) };
        assert!(!state(hwnd).unwrap().snipping);
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
    }
}
