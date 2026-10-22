//! The capture box, ported from FastPad `src/window/capture_box.rs`: a small
//! top-most window with a multi-line edit, a strip of attachments, and keys to save, close,
//! paste images, snip and pick a notebook. It files nothing itself: a `Sink` does. The first
//! half of the file is pure layout and key logic.
//!
//! Changes from FastPad's box: the title names the notebook, not a target, in a selector that
//! (like Ctrl+N) picks a notebook or any folder for this capture instead of cycling targets; the
//! footer names what was detected ("Screenshot + file"); there is no Ctrl+Shift+Enter (FastCapture has no editor
//! to open); files arrive through `WM_DROPFILES` rather than an OLE drop target; and the 20 MB
//! question is a message box.

use super::autocorrect::{self, Correction};
use super::controls;
use super::frame;
use super::theme::ramp::{self, TextStyle};
use super::theme::{self, Palette, scale};
use crate::capture::detect;
use crate::capture::file::LARGE_FILE_BYTES;
use crate::capture::{Attachment, Capture, draft};
use crate::config::settings::SettingsFile;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DT_CALCRECT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
    DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect,
    GetDC, GetMonitorInfoW, HBRUSH, HDC, HFONT, InvalidateRect, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromPoint, MonitorFromWindow, PAINTSTRUCT, ReleaseDC, ScreenToClient,
    SelectObject, SetBkColor, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, RemoveClipboardFormatListener,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    EM_GETSEL, EM_REPLACESEL, EM_SETMARGINS, EM_SETSEL, MARGINS,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, SetFocus, VK_BACK, VK_CONTROL, VK_DELETE, VK_ESCAPE, VK_LEFT, VK_MENU,
    VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB,
};
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, DragAcceptFiles, DragFinish, HDROP, RemoveWindowSubclass, SetWindowSubclass,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, EC_LEFTMARGIN, EC_RIGHTMARGIN, ES_AUTOVSCROLL,
    ES_MULTILINE, ES_WANTRETURN, FindWindowW, GWLP_USERDATA, GetClientRect, GetCursorPos,
    GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    HTCAPTION, HTCLIENT, IDC_ARROW, IsWindow, KillTimer, LoadCursorW, MINMAXINFO, RegisterClassW,
    SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetForegroundWindow, SetTimer,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, WM_APP, WM_CHAR,
    WM_CLIPBOARDUPDATE, WM_CLOSE, WM_CTLCOLOREDIT, WM_DESTROY, WM_DPICHANGED, WM_DROPFILES,
    WM_ENTERSIZEMOVE, WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_NCCALCSIZE, WM_NCDESTROY, WM_NCHITTEST, WM_PAINT, WM_PASTE, WM_SETFOCUS, WM_SETFONT,
    WM_SIZE, WM_TIMER, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_EX_ACCEPTFILES,
    WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

/// The box's size when it has none saved, and the smallest it can be made.
pub const DEFAULT_SIZE_AT_96_DPI: (i32, i32) = (720, 420);
pub const MIN_SIZE_AT_96_DPI: (i32, i32) = (420, 240);
const PAD_AT_96_DPI: i32 = 12;
const TITLE_AT_96_DPI: i32 = 34;
/// The notebook selector: its gap above and below in the title band, the room before its text
/// and the room its chevron takes.
const SELECTOR_INSET_AT_96_DPI: i32 = 4;
const SELECTOR_TEXT_AT_96_DPI: i32 = 8;
const SELECTOR_CHEVRON_AT_96_DPI: i32 = 26;
/// The edit never gets shorter than this, about two lines.
const EDIT_FLOOR_AT_96_DPI: i32 = 40;
/// Room above the edit's first line, and the edit's inner margins left and right.
const EDIT_TOP_GAP_AT_96_DPI: i32 = 6;
const EDIT_MARGIN_AT_96_DPI: i32 = 8;
const CHIP_HEIGHT_AT_96_DPI: i32 = 30;
const CHIP_GAP_AT_96_DPI: i32 = 8;
const CHIP_MAX_AT_96_DPI: i32 = 220;
const CHIP_REMOVE_AT_96_DPI: i32 = 24;
const ERROR_AT_96_DPI: i32 = 22;
const FOOTER_AT_96_DPI: i32 = 30;

/// Posted to the box when the capture hotkey arrives while it is open: it comes to the front,
/// or back from a snip without an image.
pub const WM_CAPTURE_AGAIN: u32 = WM_APP + 0x60;
/// A pasted bitmap finished encoding on its worker thread: a boxed `Result<Vec<u8>, String>` in
/// lparam.
const WM_IMAGE_READY: u32 = WM_APP + 0x61;
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
    /// Ctrl+Alt+V: the clipboard's plain text, even when it holds rich text.
    PastePlain,
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
        k if k == u16::from(b'V') && ctrl && alt && !shift => Some(Action::PastePlain),
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
    /// The notebook selector in the title band: click it (or Ctrl+N) for another notebook.
    pub notebook: RECT,
    pub close: RECT,
    pub edit: RECT,
    /// One per attachment, wrapped onto as many rows as they need.
    pub chips: Vec<RECT>,
    pub error: Option<RECT>,
    pub footer: RECT,
    pub snip: RECT,
}

/// Lays the box out in a client area `size` big, for chips `chip_widths` pixels wide (their
/// text), with an error line or not. `title_widths` are the title's "Capture →" and the
/// notebook's name, whose selector follows the one and is cut short before the close button.
/// The footer and error keep to the bottom, the chips sit above them, and the edit takes the
/// rest, down to its floor; chips that don't fit then are left empty (`RECT::default()`).
pub fn layout(
    dpi: u32,
    size: (i32, i32),
    chip_widths: &[i32],
    has_error: bool,
    snip_width: i32,
    title_widths: (i32, i32),
) -> Layout {
    let s = |v: i32| scale(v, dpi);
    let (width, height) = size;
    let pad = s(PAD_AT_96_DPI);
    let gap = s(CHIP_GAP_AT_96_DPI);
    let chip_height = s(CHIP_HEIGHT_AT_96_DPI);
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
    let (prefix, name) = title_widths;
    let selector_left = (pad + prefix + s(6)).min(close.left);
    let notebook = RECT {
        left: selector_left,
        top: s(SELECTOR_INSET_AT_96_DPI),
        right: (selector_left + s(SELECTOR_TEXT_AT_96_DPI) + name + s(SELECTOR_CHEVRON_AT_96_DPI))
            .min(close.left - s(4))
            .max(selector_left),
        bottom: title.bottom - s(SELECTOR_INSET_AT_96_DPI),
    };
    let footer = RECT {
        left: pad,
        top: height - s(4) - s(FOOTER_AT_96_DPI),
        right: width - pad,
        bottom: height - s(4),
    };
    let error = has_error.then(|| RECT {
        left: pad,
        top: footer.top - s(ERROR_AT_96_DPI),
        right: width - pad,
        bottom: footer.top,
    });
    let below = error.map_or(footer.top, |line| line.top);
    // The chips wrapped into rows, at x and a row number.
    let mut x = pad;
    let mut row = 0;
    let mut placed = Vec::new();
    for text in chip_widths {
        let chip = (text + s(CHIP_REMOVE_AT_96_DPI) + s(PAD_AT_96_DPI)).min(s(CHIP_MAX_AT_96_DPI));
        if x > pad && x + chip > width - pad {
            x = pad;
            row += 1;
        }
        placed.push((x, chip, row));
        x += chip + gap;
    }
    let rows = if placed.is_empty() { 0 } else { row + 1 };
    let strip = rows * (chip_height + gap);
    let edit_top = title.bottom + s(EDIT_TOP_GAP_AT_96_DPI);
    let edit_bottom = (below - strip - gap).max(edit_top + s(EDIT_FLOOR_AT_96_DPI));
    let edit = RECT {
        left: pad,
        top: edit_top,
        right: width - pad,
        bottom: edit_bottom,
    };
    let chips = placed
        .into_iter()
        .map(|(x, chip, row)| {
            let top = edit.bottom + gap + row * (chip_height + gap);
            if top + chip_height > below {
                RECT::default()
            } else {
                RECT {
                    left: x,
                    top,
                    right: x + chip,
                    bottom: top + chip_height,
                }
            }
        })
        .collect();
    let snip = RECT {
        left: pad,
        top: footer.top,
        right: pad + snip_width,
        bottom: footer.bottom,
    };
    Layout {
        width,
        height,
        title,
        notebook,
        close,
        edit,
        chips,
        error,
        footer,
        snip,
    }
}

/// The title band says where the capture goes: this, then the notebook selector.
pub const TITLE_PREFIX: &str = "Capture \u{2192}";

/// The notebook menu's last entry: any folder, through Windows' folder dialog.
const OTHER_FOLDER_LABEL: &str = "&Other folder\u{2026}";

/// What the notebook menu offers: the recent notebooks, with the current one first when it isn't
/// among them.
pub fn notebook_choices(current: &Path, recent: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut notebooks = recent;
    if !notebooks.iter().any(|known| known == current) {
        notebooks.insert(0, current.to_path_buf());
    }
    notebooks
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
    /// The page the last rich text pasted came from.
    source_url: Option<String>,
    draft_dir: Option<PathBuf>,
    settings: Option<PathBuf>,
    /// The box's size when a move or resize began: only a new size is saved.
    size_before_drag: Option<(i32, i32)>,
    autocorrect: bool,
    /// The typo autocorrect fixed last, for Ctrl+Z, until another key, click or paste.
    last_fix: Option<Correction>,
    /// The WM_CHAR of a Ctrl+Z that undid a fix must not undo in the edit too.
    swallow_undo_char: bool,
    error: Option<String>,
    layout: Layout,
    snipping: bool,
    /// The WM_CHAR of a Ctrl+Shift+V that attached an image must not paste text too.
    swallow_paste_char: bool,
    /// Pasted bitmaps still being encoded to PNG on worker threads.
    preparing: usize,
    /// Ctrl+Enter came while images were being prepared: file once they are ready.
    submit_when_ready: bool,
}

const SNIP_LABEL: &str = "+ Snip (Ctrl+Shift+S)";
const PREPARING_LABEL: &str = "Preparing image\u{2026}";

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
    /// `fastcapture.ini`, where the box's size is kept; `None` keeps no size.
    pub settings: Option<PathBuf>,
    /// Fix common English typos as they are typed.
    pub autocorrect: bool,
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
    let body_font = ramp::font(TextStyle::Body, dpi);
    let title_font = ramp::font(TextStyle::Heading, dpi);
    unsafe {
        SendMessageW(edit, WM_SETFONT, body_font as usize, 0);
        set_margins(edit, dpi);
        SetWindowSubclass(edit, Some(edit_proc), EDIT_HOOK, hwnd as usize);
        DragAcceptFiles(hwnd, 1);
    }
    controls::theme_scrollbars(edit, &colors);
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
        edit_brush: unsafe { CreateSolidBrush(colors.content_background) },
        title_font,
        body_font,
        items: restored.attachments,
        selected: 0,
        notebook: options.notebook,
        source_window: options.source_window,
        source_url: restored.source_url,
        draft_dir: options.draft_dir,
        settings: options.settings,
        size_before_drag: None,
        autocorrect: options.autocorrect,
        last_fix: None,
        swallow_undo_char: false,
        error: None,
        layout: Layout::default(),
        snipping: false,
        swallow_paste_char: false,
        preparing: 0,
        submit_when_ready: false,
    });
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize) };
    set_edit_text(edit, &restored.text);
    place(hwnd, cursor);
    relayout(hwnd);
    // Rounded and dark as the theme says; the 1 px frame extension keeps DWM's shadow on a
    // window without a border.
    frame::apply_theme(hwnd, &colors);
    unsafe {
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

/// Sizes the box and centers it on the work area of the monitor at `point`, a third of the way
/// down.
fn place(hwnd: HWND, point: POINT) {
    let Some(capture) = state(hwnd) else {
        return;
    };
    let mut monitor: MONITORINFO = unsafe { std::mem::zeroed() };
    monitor.cbSize = size_of::<MONITORINFO>() as u32;
    unsafe {
        GetMonitorInfoW(
            MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        )
    };
    let work = monitor.rcWork;
    let (width, height) = initial_size(saved_size(capture.settings.as_deref()), capture.dpi, work);
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

/// The size the box opens at: the saved one when it is at least the smallest the box can be,
/// else the default, and never bigger than the work area `work`.
pub fn initial_size(saved: Option<(i32, i32)>, dpi: u32, work: RECT) -> (i32, i32) {
    let s = |v: i32| scale(v, dpi);
    let (width, height) = saved
        .filter(|&(width, height)| {
            width >= s(MIN_SIZE_AT_96_DPI.0) && height >= s(MIN_SIZE_AT_96_DPI.1)
        })
        .unwrap_or((s(DEFAULT_SIZE_AT_96_DPI.0), s(DEFAULT_SIZE_AT_96_DPI.1)));
    (
        width.min(work.right - work.left),
        height.min(work.bottom - work.top),
    )
}

/// The box's last size, from `fastcapture.ini`, when both numbers are there.
fn saved_size(settings: Option<&Path>) -> Option<(i32, i32)> {
    let file = SettingsFile::load(settings?);
    let number = |key: &str| file.value(key)?.parse::<i32>().ok();
    Some((number("capture_width")?, number("capture_height")?))
}

fn window_size(hwnd: HWND) -> (i32, i32) {
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(hwnd, &mut rect) };
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// Keeps the box's size (not its place: it opens where you are) in `fastcapture.ini`.
fn save_size(hwnd: HWND, settings: Option<&Path>) {
    let Some(path) = settings else {
        return;
    };
    let (width, height) = window_size(hwnd);
    let mut file = SettingsFile::load(path);
    file.set("capture_width", &width.to_string());
    file.set("capture_height", &height.to_string());
    if let Err(error) = file.save(path) {
        crate::log::line(&format!("capture box: {error}"));
    }
}

/// Follows the box to a monitor with another scale: the size Windows suggests, the fonts and
/// the margins at the new DPI.
fn change_dpi(hwnd: HWND, dpi: u32, suggested: &RECT) {
    let Some(capture) = state(hwnd) else {
        return;
    };
    capture.dpi = dpi;
    let title_font = ramp::font(TextStyle::Heading, dpi);
    let body_font = ramp::font(TextStyle::Body, dpi);
    unsafe {
        DeleteObject(std::mem::replace(&mut capture.title_font, title_font));
        DeleteObject(std::mem::replace(&mut capture.body_font, body_font));
        SendMessageW(capture.edit, WM_SETFONT, body_font as usize, 0);
    }
    set_margins(capture.edit, dpi);
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            suggested.left,
            suggested.top,
            suggested.right - suggested.left,
            suggested.bottom - suggested.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
    relayout(hwnd);
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

/// Lays the box out again for its size, attachments and error.
fn relayout(hwnd: HWND) {
    let Some(capture) = state(hwnd) else {
        return;
    };
    let widths: Vec<i32> = capture
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| text_width(hwnd, capture.body_font, &chip_label(item, index)))
        .chain(
            std::iter::repeat_n(PREPARING_LABEL, capture.preparing)
                .map(|label| text_width(hwnd, capture.body_font, label)),
        )
        .collect();
    let snip = text_width(hwnd, capture.body_font, SNIP_LABEL) + scale(8, capture.dpi);
    let name = notebook_name(&capture.notebook);
    let title = (
        text_width(hwnd, capture.title_font, TITLE_PREFIX),
        text_width(hwnd, capture.title_font, &name),
    );
    let mut client: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut client) };
    capture.layout = layout(
        capture.dpi,
        (client.right - client.left, client.bottom - client.top),
        &widths,
        capture.error.is_some(),
        snip,
        title,
    );
    capture.selected = capture.selected.min(capture.items.len());
    let edit = capture.layout.edit;
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
        SetWindowTextW(hwnd, wide(&accessible_name(&name)).as_ptr());
        InvalidateRect(hwnd, std::ptr::null(), 1);
    }
}

/// The edit's inner margins, so text keeps off its edges.
fn set_margins(edit: HWND, dpi: u32) {
    let margin = scale(EDIT_MARGIN_AT_96_DPI, dpi) as usize;
    unsafe {
        SendMessageW(
            edit,
            EM_SETMARGINS,
            (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
            (margin | (margin << 16)) as isize,
        )
    };
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

/// The edit's text as it holds it: UTF-16, with CRLF line breaks.
fn edit_units(edit: HWND) -> Vec<u16> {
    let length = unsafe { GetWindowTextLengthW(edit) }.max(0) as usize;
    let mut buffer = vec![0_u16; length + 1];
    let copied = unsafe { GetWindowTextW(edit, buffer.as_mut_ptr(), buffer.len() as i32) };
    buffer.truncate(copied.max(0) as usize);
    buffer
}

/// The edit's selection, as UTF-16 offsets.
fn selection(edit: HWND) -> (usize, usize) {
    let (mut start, mut end) = (0_u32, 0_u32);
    unsafe {
        SendMessageW(
            edit,
            EM_GETSEL,
            &raw mut start as usize,
            &raw mut end as LPARAM,
        )
    };
    (start as usize, end as usize)
}

/// Replaces `range` of the edit's text with `text`, out of the edit's own undo: the box's Ctrl+Z
/// takes a fix back.
fn replace_range(edit: HWND, range: std::ops::Range<usize>, text: &str) {
    unsafe {
        SendMessageW(edit, EM_SETSEL, range.start, range.end as LPARAM);
        SendMessageW(edit, EM_REPLACESEL, 0, wide(text).as_ptr() as LPARAM);
    }
}

/// A word boundary is about to be typed: fixes the word before the caret when Windows' English
/// checker is sure of the fix.
fn autocorrect_before(hwnd: HWND) {
    let Some(capture) = state(hwnd) else {
        return;
    };
    let edit = capture.edit;
    let (start, end) = selection(edit);
    if start != end {
        return;
    }
    let units = edit_units(edit);
    let before = String::from_utf16_lossy(&units[..end.min(units.len())]);
    let Some(fix) = autocorrect::correction(&before, |word| {
        crate::platform::spell::with_english(|speller| speller.replacement(word)).flatten()
    }) else {
        return;
    };
    replace_range(edit, fix.start..end, &fix.replacement);
    capture.last_fix = Some(fix);
}

/// Ctrl+Z right after a fix: puts the typed word back. False when there is no such fix, and the
/// edit undoes as usual.
fn undo_fix(hwnd: HWND) -> bool {
    let Some(capture) = state(hwnd) else {
        return false;
    };
    let Some(fix) = capture.last_fix.take() else {
        return false;
    };
    let edit = capture.edit;
    let replacement: Vec<u16> = fix.replacement.encode_utf16().collect();
    let end = fix.start + replacement.len();
    let units = edit_units(edit);
    // The boundary typed after the fix: one character, or the CRLF Enter puts in.
    let boundary = if units.get(end..end + 2) == Some(&[0x0D, 0x0A][..]) {
        2
    } else {
        1
    };
    // The fix, then that boundary, with the caret right after it.
    let intact = units.get(fix.start..end) == Some(&replacement[..])
        && units.len() >= end + boundary
        && selection(edit) == (end + boundary, end + boundary);
    if !intact {
        return false;
    }
    replace_range(edit, fix.start..end, &fix.original);
    let caret = fix.start + fix.original.encode_utf16().count() + boundary;
    unsafe { SendMessageW(edit, EM_SETSEL, caret, caret as LPARAM) };
    true
}

/// What the box holds now.
fn current_capture(capture: &CaptureBox) -> Capture {
    Capture {
        text: edit_text(capture.edit),
        attachments: capture.items.clone(),
        source_window: capture.source_window.clone(),
        source_url: capture.source_url.clone(),
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
        let question = super::popup::Question {
            title: "Copy a large file?",
            text: &message,
            buttons: &["Copy it", "Don't copy"],
            default: 0,
            cancel: 1,
        };
        super::popup::ask(hwnd, &question) == 0
    }
}

/// Puts `text` in place of the edit's selection, as typing would (it can be undone).
fn insert_text(hwnd: HWND, text: &str) {
    if let Some(capture) = state(hwnd) {
        let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
        unsafe {
            SendMessageW(
                capture.edit,
                EM_REPLACESEL,
                1,
                wide(&text).as_ptr() as LPARAM,
            )
        };
    }
    invalidate(hwnd);
}

/// Remembers the page the clipboard's rich text came from, and returns that rich text.
fn clipboard_html(hwnd: HWND) -> Option<crate::platform::clipboard::Html> {
    let html = crate::platform::clipboard::html(hwnd)?;
    if let Some(url) = &html.source_url
        && let Some(capture) = state(hwnd)
    {
        capture.source_url = Some(url.clone());
    }
    Some(html)
}

/// Ctrl+Alt+V: the clipboard's text as it is.
fn paste_plain(hwnd: HWND) {
    let _ = clipboard_html(hwnd);
    if let Some(text) = crate::platform::clipboard::text(hwnd) {
        insert_text(hwnd, &text);
    }
}

/// Ctrl+V and the edit's Paste: files and lone images are attached, and rich text with
/// structure goes in as Markdown. Returns whether the paste was taken, so the edit doesn't
/// paste the plain text too.
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
        Paste::Text => {
            let Some(html) = clipboard_html(hwnd) else {
                return false;
            };
            match crate::extract::markdown::rich_text(&html.fragment, html.source_url.as_deref()) {
                Some(markdown) => {
                    insert_text(hwnd, &markdown);
                    true
                }
                None => false,
            }
        }
        Paste::Files => {
            for path in clipboard::files(hwnd) {
                attach(hwnd, Attachment::File(path));
            }
            true
        }
        Paste::Image => {
            match clipboard::image(hwnd) {
                Some(clipboard::ClipboardImage::Png(png)) => attach(hwnd, Attachment::Png(png)),
                Some(clipboard::ClipboardImage::Dib(dib)) => prepare_image(hwnd, dib),
                None => {}
            }
            true
        }
    }
}

/// Encodes a pasted bitmap on a worker thread, so a 4K screenshot doesn't freeze the box; a
/// "Preparing image…" chip stands in for it until `WM_IMAGE_READY`.
fn prepare_image(hwnd: HWND, dib: Vec<u8>) {
    if let Some(capture) = state(hwnd) {
        capture.preparing += 1;
    }
    relayout(hwnd);
    let window = hwnd as usize;
    std::thread::spawn(move || {
        let result = Box::new(crate::platform::clipboard::encode_dib(&dib));
        let payload = Box::into_raw(result);
        let posted = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                window as HWND,
                WM_IMAGE_READY,
                0,
                payload as isize,
            )
        };
        // The box closed meanwhile: the image goes with it.
        if posted == 0 {
            drop(unsafe { Box::from_raw(payload) });
        }
    });
}

fn image_ready(hwnd: HWND, result: Result<Vec<u8>, String>) {
    if let Some(capture) = state(hwnd) {
        capture.preparing = capture.preparing.saturating_sub(1);
    }
    match result {
        Ok(png) => attach(hwnd, Attachment::Png(png)),
        Err(error) => set_error(hwnd, Some(error)),
    }
    let file_now =
        state(hwnd).is_some_and(|capture| capture.preparing == 0 && capture.submit_when_ready);
    if file_now {
        submit(hwnd);
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

#[cfg(test)]
thread_local! {
    /// What tests choose in the notebook menu (0 closes it), and then in the folder dialog.
    static MENU_ANSWER: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static FOLDER_ANSWER: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

fn ask_menu(hwnd: HWND, anchor: super::popup::Anchor, entries: &[super::popup::Entry<'_>]) -> u32 {
    #[cfg(test)]
    {
        let _ = (hwnd, anchor, entries);
        MENU_ANSWER.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        super::popup::menu(hwnd, anchor, entries)
    }
}

fn ask_folder(hwnd: HWND) -> Option<PathBuf> {
    #[cfg(test)]
    {
        let _ = hwnd;
        FOLDER_ANSWER.with(|answer| answer.borrow_mut().take())
    }
    #[cfg(not(test))]
    {
        crate::platform::shell::pick_folder(hwnd, "Choose a notebook for this capture")
    }
}

/// The notebook selector or Ctrl+N: a menu of the recent notebooks, the current one checked,
/// then any other folder. The choice is for this capture only.
fn pick_notebook(hwnd: HWND) {
    let Some(capture) = state(hwnd) else {
        return;
    };
    let notebooks = notebook_choices(&capture.notebook, capture.sink.notebooks());
    let labels: Vec<String> = notebooks
        .iter()
        .enumerate()
        .map(|(index, notebook)| match index {
            // Access keys 1 to 9; the rest are reached with the arrows.
            0..9 => format!("&{} {}", index + 1, notebook_name(notebook)),
            _ => notebook_name(notebook),
        })
        .collect();
    let other = notebooks.len() as u32 + 1;
    let mut entries: Vec<super::popup::Entry<'_>> = notebooks
        .iter()
        .zip(&labels)
        .enumerate()
        .map(|(index, (notebook, label))| super::popup::Entry::Item {
            id: index as u32 + 1,
            text: label,
            enabled: true,
            checked: *notebook == capture.notebook,
        })
        .collect();
    entries.push(super::popup::Entry::Separator);
    entries.push(super::popup::Entry::Item {
        id: other,
        text: OTHER_FOLDER_LABEL,
        enabled: true,
        checked: false,
    });
    let mut at = POINT {
        x: capture.layout.notebook.left,
        y: capture.layout.title.bottom,
    };
    unsafe { windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut at) };
    let chosen = ask_menu(hwnd, super::popup::Anchor::Below(at), &entries);
    let notebook = if chosen == other {
        ask_folder(hwnd)
    } else {
        (chosen as usize)
            .checked_sub(1)
            .and_then(|index| notebooks.get(index).cloned())
    };
    if let Some(notebook) = notebook
        && let Some(capture) = state(hwnd)
    {
        capture.notebook = notebook;
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
    if capture.preparing > 0 {
        capture.submit_when_ready = true;
        set_error(
            hwnd,
            Some("Saving as soon as the image is ready\u{2026}".to_owned()),
        );
        return;
    }
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
        Action::PastePlain => paste_plain(hwnd),
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
            let key = wparam as u16;
            if key == u16::from(b'Z') && ctrl && !shift && !alt {
                if undo_fix(hwnd)
                    && let Some(capture) = state(hwnd)
                {
                    capture.swallow_undo_char = true;
                    invalidate(hwnd);
                    return 0;
                }
            } else if !matches!(key, VK_CONTROL | VK_SHIFT | VK_MENU)
                && let Some(capture) = state(hwnd)
            {
                capture.last_fix = None;
            }
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
            if ch == 0x1A
                && let Some(capture) = state(hwnd)
                && std::mem::take(&mut capture.swallow_undo_char)
            {
                return 0;
            }
            if autocorrect::is_boundary(ch)
                && state(hwnd).is_some_and(|capture| capture.autocorrect)
            {
                autocorrect_before(hwnd);
            }
            // Typing changes what the footer says the capture holds.
            invalidate(hwnd);
        }
        WM_LBUTTONDOWN => {
            if let Some(capture) = state(hwnd) {
                capture.last_fix = None;
            }
        }
        WM_PASTE => {
            if let Some(capture) = state(hwnd) {
                capture.last_fix = None;
            }
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

/// Takes the theme now in force: colors, the edit's brush and scroll bars, the frame.
fn retheme(hwnd: HWND) {
    let Some(capture) = state(hwnd) else {
        return;
    };
    let colors = theme::current();
    if colors == capture.colors {
        return;
    }
    capture.colors = colors;
    unsafe {
        DeleteObject(capture.edit_brush);
        capture.edit_brush = CreateSolidBrush(colors.content_background);
    }
    controls::theme_scrollbars(capture.edit, &colors);
    frame::apply_theme(hwnd, &colors);
    unsafe {
        InvalidateRect(capture.edit, std::ptr::null(), 1);
    }
    invalidate(hwnd);
}

fn paint(hwnd: HWND) {
    let Some(capture) = state(hwnd) else {
        return;
    };
    let mut ps: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    let screen = unsafe { BeginPaint(hwnd, &mut ps) };
    let whole = RECT {
        left: 0,
        top: 0,
        right: capture.layout.width,
        bottom: capture.layout.height,
    };
    // Drawn in memory first, so nothing flickers as the box is typed in or chips change.
    super::paint::in_memory(screen, whole, |dc, _| draw(capture, dc, hwnd));
    unsafe { EndPaint(hwnd, &ps) };
}

fn draw(capture: &CaptureBox, dc: HDC, hwnd: HWND) {
    let colors = capture.colors;
    let layout = capture.layout.clone();
    let client = RECT {
        left: 0,
        top: 0,
        right: layout.width,
        bottom: layout.height,
    };
    let strip_focused = unsafe { GetFocus() } == hwnd;
    fill(dc, &client, colors.strip_background);
    unsafe { SetBkMode(dc, TRANSPARENT as i32) };
    let pad = scale(PAD_AT_96_DPI, capture.dpi);
    let old = unsafe { SelectObject(dc, capture.title_font) };
    unsafe { SetTextColor(dc, colors.content_foreground) };
    draw_text(
        dc,
        TITLE_PREFIX,
        &RECT {
            left: pad,
            right: layout.notebook.left,
            ..layout.title
        },
        DT_LEFT | DT_END_ELLIPSIS,
    );
    draw_selector(capture, dc, &layout.notebook);
    unsafe { SelectObject(dc, capture.body_font) };
    unsafe { SetTextColor(dc, colors.muted_foreground) };
    draw_text(dc, "\u{2715}", &layout.close, DT_CENTER);
    for (index, chip) in layout.chips.iter().enumerate() {
        if chip.right <= chip.left {
            // Clipped: no room for it in a box this small.
            continue;
        }
        let selected = strip_focused && capture.selected == index;
        fill(
            dc,
            chip,
            if selected {
                colors.selection_background
            } else {
                colors.content_background
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
        unsafe { SetTextColor(dc, colors.content_foreground) };
        let Some(item) = capture.items.get(index) else {
            // An image still being prepared: no remove button yet.
            unsafe { SetTextColor(dc, colors.muted_foreground) };
            draw_text(
                dc,
                PREPARING_LABEL,
                &RECT {
                    left: chip.left + pad / 2,
                    ..*chip
                },
                DT_LEFT | DT_END_ELLIPSIS,
            );
            continue;
        };
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
        unsafe { SetTextColor(dc, colors.muted_foreground) };
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
        unsafe { SetTextColor(dc, colors.error_foreground) };
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
    unsafe { SetTextColor(dc, colors.muted_foreground) };
    draw_text(
        dc,
        &footer_text(&current_capture(capture)),
        &RECT {
            left: layout.snip.right + pad,
            ..layout.footer
        },
        DT_RIGHT | DT_END_ELLIPSIS,
    );
    unsafe { SelectObject(dc, old) };
}

/// The notebook selector: a field like the settings' drop-downs, the notebook's name in it and a
/// chevron at its end. Drawn with the title font selected.
fn draw_selector(capture: &CaptureBox, dc: HDC, area: &RECT) {
    if area.right <= area.left {
        return;
    }
    let colors = capture.colors;
    let dpi = capture.dpi;
    let radius = super::paint::radius(&colors, theme::metrics::CONTROL_RADIUS, dpi);
    super::paint::fill_bordered(
        dc,
        *area,
        radius,
        colors.content_background,
        colors.stroke,
        colors.strip_background,
    );
    let chevron = RECT {
        left: (area.right - scale(SELECTOR_CHEVRON_AT_96_DPI, dpi)).max(area.left),
        ..*area
    };
    unsafe { SetTextColor(dc, colors.content_foreground) };
    draw_text(
        dc,
        &notebook_name(&capture.notebook),
        &RECT {
            left: area.left + scale(SELECTOR_TEXT_AT_96_DPI, dpi),
            right: chevron.left,
            ..*area
        },
        DT_LEFT | DT_END_ELLIPSIS,
    );
    let glyphs = ramp::icon_font(10, dpi);
    super::paint::text(
        dc,
        super::paint::GLYPH_CHEVRON_DOWN,
        chevron,
        glyphs,
        colors.muted_foreground,
        super::paint::Align::Center,
    );
    unsafe { DeleteObject(glyphs) };
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
    if contains(&layout.notebook, x, y) {
        pick_notebook(hwnd);
        return;
    }
    if contains(&layout.snip, x, y) {
        start_snip(hwnd);
        return;
    }
    let remove = scale(CHIP_REMOVE_AT_96_DPI, capture.dpi);
    if let Some(index) = layout.chips.iter().position(|chip| {
        chip.right > chip.left
            && contains(
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
        _ if message == theme::changed_message() => {
            retheme(hwnd);
            0
        }
        WM_NCCALCSIZE => 0,
        WM_NCHITTEST => {
            let mut point = POINT {
                x: (lparam & 0xFFFF) as i16 as i32,
                y: ((lparam >> 16) & 0xFFFF) as i16 as i32,
            };
            unsafe { ScreenToClient(hwnd, &mut point) };
            if let Some(capture) = state(hwnd) {
                // The close button keeps its target to the corner, as the frame's buttons do.
                if contains(&capture.layout.close, point.x, point.y) {
                    return HTCLIENT as LRESULT;
                }
                let (width, height) = (capture.layout.width, capture.layout.height);
                let border = frame::resize_border(capture.dpi);
                if let Some(edge) = frame::edge(point.x, point.y, width, height, border) {
                    return edge.code() as LRESULT;
                }
            }
            let drags = state(hwnd).is_some_and(|capture| {
                contains(&capture.layout.title, point.x, point.y)
                    && !contains(&capture.layout.close, point.x, point.y)
                    && !contains(&capture.layout.notebook, point.x, point.y)
            });
            if drags {
                HTCAPTION as LRESULT
            } else {
                HTCLIENT as LRESULT
            }
        }
        WM_GETMINMAXINFO => {
            let Some(capture) = state(hwnd) else {
                return 0;
            };
            let info = unsafe { &mut *(lparam as *mut MINMAXINFO) };
            info.ptMinTrackSize.x = scale(MIN_SIZE_AT_96_DPI.0, capture.dpi);
            info.ptMinTrackSize.y = scale(MIN_SIZE_AT_96_DPI.1, capture.dpi);
            let mut monitor: MONITORINFO = unsafe { std::mem::zeroed() };
            monitor.cbSize = size_of::<MONITORINFO>() as u32;
            let found = unsafe {
                GetMonitorInfoW(
                    MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                    &mut monitor,
                )
            };
            if found != 0 {
                let work = monitor.rcWork;
                info.ptMaxTrackSize.x = work.right - work.left;
                info.ptMaxTrackSize.y = work.bottom - work.top;
            }
            0
        }
        WM_SIZE => {
            relayout(hwnd);
            0
        }
        WM_ENTERSIZEMOVE => {
            if let Some(capture) = state(hwnd) {
                capture.size_before_drag = Some(window_size(hwnd));
            }
            0
        }
        WM_EXITSIZEMOVE => {
            if let Some(capture) = state(hwnd)
                && capture
                    .size_before_drag
                    .take()
                    .is_some_and(|size| size != window_size(hwnd))
            {
                save_size(hwnd, capture.settings.as_deref());
            }
            0
        }
        WM_DPICHANGED => {
            if lparam != 0 {
                let suggested = unsafe { *(lparam as *const RECT) };
                change_dpi(hwnd, ((wparam >> 16) & 0xFFFF) as u32, &suggested);
            }
            0
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
                SetTextColor(dc, capture.colors.content_foreground);
                SetBkColor(dc, capture.colors.content_background);
            }
            capture.edit_brush as LRESULT
        }
        WM_CAPTURE_AGAIN => {
            if state(hwnd).is_some_and(|capture| capture.snipping) {
                // The hotkey during a snip brings the box back without an image.
                end_snip(hwnd);
            } else if unsafe { GetForegroundWindow() } != hwnd {
                retheme(hwnd);
                show(hwnd);
            }
            0
        }
        WM_DROPFILES => {
            dropped(hwnd, wparam as HDROP);
            0
        }
        WM_IMAGE_READY => {
            if lparam != 0 {
                let result = unsafe { Box::from_raw(lparam as *mut Result<Vec<u8>, String>) };
                image_ready(hwnd, *result);
            }
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

/// Types `text` key by key into the box's edit, as WM_CHARs through its subclass.
#[cfg(test)]
pub(crate) fn type_for_test(hwnd: HWND, text: &str) {
    let Some(edit) = state(hwnd).map(|capture| capture.edit) else {
        return;
    };
    for unit in text.encode_utf16() {
        unsafe { SendMessageW(edit, WM_CHAR, usize::from(unit), 0) };
    }
}

/// Puts `text` in the box, for tests elsewhere.
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
            key_action(u16::from(b'V'), true, false, true),
            Some(Action::PastePlain)
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
    fn the_edit_fills_what_the_strip_error_and_footer_leave() {
        let plain = layout(96, (720, 420), &[], false, 160, (80, 60));
        assert_eq!((plain.width, plain.height), (720, 420));
        assert_eq!(plain.edit.top, 34 + 6);
        assert_eq!(plain.footer.bottom, 420 - 4);
        assert_eq!(plain.edit.bottom + 8, plain.footer.top);
        let with = layout(96, (520, 420), &[150, 150, 150], true, 160, (80, 60));
        assert_eq!(with.chips[0].top, with.chips[1].top);
        assert!(
            with.chips[2].top > with.chips[1].top,
            "the third wraps at 520"
        );
        assert!(
            with.edit.bottom < plain.edit.bottom,
            "the edit gave the chips room"
        );
        assert!(with.error.unwrap().top >= with.chips[2].bottom);
        assert_eq!(
            layout(192, (1440, 840), &[], false, 320, (160, 120))
                .edit
                .top,
            80
        );
    }

    #[test]
    fn chips_that_do_not_fit_are_clipped_not_overlapping() {
        let tight = layout(96, (420, 240), &[200; 12], true, 160, (80, 60));
        assert_eq!(tight.chips.len(), 12);
        assert_eq!(
            tight.edit.bottom - tight.edit.top,
            40,
            "the edit keeps its floor"
        );
        let error = tight.error.unwrap();
        for chip in tight.chips.iter().filter(|c| c.right > c.left) {
            assert!(chip.bottom <= error.top, "{} > {}", chip.bottom, error.top);
        }
        assert!(
            tight.chips.iter().any(|c| c.right == c.left),
            "some were clipped"
        );
    }

    #[test]
    fn the_notebook_selector_follows_the_title_and_stops_before_the_close_button() {
        let plain = layout(96, (720, 420), &[], false, 160, (80, 60));
        assert_eq!(plain.notebook.left, 12 + 80 + 6);
        assert_eq!(plain.notebook.right, plain.notebook.left + 8 + 60 + 26);
        assert_eq!((plain.notebook.top, plain.notebook.bottom), (4, 30));
        let long = layout(96, (420, 240), &[], false, 160, (80, 900));
        assert_eq!(
            long.notebook.right,
            long.close.left - 4,
            "a long name is cut short"
        );
    }

    #[test]
    fn the_notebook_menu_offers_the_current_notebook_once() {
        let recent = vec![PathBuf::from(r"D:\Notes"), PathBuf::from(r"E:\Work")];
        assert_eq!(
            notebook_choices(Path::new(r"E:\Work"), recent.clone()),
            recent
        );
        assert_eq!(
            notebook_choices(Path::new(r"F:\New"), recent),
            [
                PathBuf::from(r"F:\New"),
                PathBuf::from(r"D:\Notes"),
                PathBuf::from(r"E:\Work")
            ]
        );
    }

    /// Clicks the box's notebook selector, the menu answering `entry` and the folder dialog
    /// `folder`.
    fn click_selector(hwnd: HWND, entry: u32, folder: Option<&str>) {
        MENU_ANSWER.with(|answer| answer.set(entry));
        FOLDER_ANSWER.with(|answer| *answer.borrow_mut() = folder.map(PathBuf::from));
        let selector = state(hwnd).unwrap().layout.notebook;
        let (x, y) = (
            (selector.left + selector.right) / 2,
            (selector.top + selector.bottom) / 2,
        );
        unsafe {
            SendMessageW(
                hwnd,
                WM_LBUTTONDOWN,
                0,
                (((y as u32) << 16) | (x as u32 & 0xFFFF)) as isize,
            )
        };
        MENU_ANSWER.with(|answer| answer.set(0));
    }

    #[test]
    fn the_notebook_selector_changes_where_this_capture_goes() {
        let drafts = temp("selector");
        let (hwnd, filed) = open_box(&drafts, None);
        let selector = state(hwnd).unwrap().layout.notebook;
        let mut point = POINT {
            x: selector.left + 1,
            y: (selector.top + selector.bottom) / 2,
        };
        unsafe { windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut point) };
        let hit = unsafe {
            SendMessageW(
                hwnd,
                WM_NCHITTEST,
                0,
                (((point.y as u32) << 16) | (point.x as u32 & 0xFFFF)) as isize,
            )
        };
        assert_eq!(
            hit, HTCLIENT as isize,
            "the selector clicks, it doesn't drag"
        );
        click_selector(hwnd, 0, None);
        assert_eq!(
            state(hwnd).unwrap().notebook,
            Path::new(r"D:\Notes"),
            "closed"
        );
        click_selector(hwnd, 2, None);
        assert_eq!(state(hwnd).unwrap().notebook, Path::new(r"E:\Work"));
        set_edit_text(state(hwnd).unwrap().edit, "to work");
        submit(hwnd);
        assert_eq!(filed.borrow()[0].1, PathBuf::from(r"E:\Work"));
        draft::clear(&drafts);
    }

    #[test]
    fn any_folder_can_be_picked_from_the_notebook_menu() {
        let drafts = temp("other");
        let (hwnd, _) = open_box(&drafts, None);
        // The two recent notebooks, then "Other folder…".
        click_selector(hwnd, 3, None);
        assert_eq!(
            state(hwnd).unwrap().notebook,
            Path::new(r"D:\Notes"),
            "cancelled"
        );
        click_selector(hwnd, 3, Some(r"F:\Elsewhere"));
        assert_eq!(state(hwnd).unwrap().notebook, Path::new(r"F:\Elsewhere"));
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
    }

    #[test]
    fn the_box_resizes_from_its_edges_and_keeps_a_minimum() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            HTBOTTOMRIGHT, MINMAXINFO, SWP_NOMOVE, WM_GETMINMAXINFO,
        };
        let drafts = temp("resize");
        let (hwnd, _) = open_box(&drafts, None);
        let mut frame: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(hwnd, &mut frame) };
        let at = |x: i32, y: i32| unsafe {
            SendMessageW(
                hwnd,
                WM_NCHITTEST,
                0,
                (((y as u32) << 16) | (x as u32 & 0xFFFF)) as isize,
            )
        };
        assert_eq!(
            at(frame.right - 1, frame.bottom - 1),
            HTBOTTOMRIGHT as isize
        );
        let mut info: MINMAXINFO = unsafe { std::mem::zeroed() };
        unsafe { SendMessageW(hwnd, WM_GETMINMAXINFO, 0, &raw mut info as isize) };
        let dpi = state(hwnd).unwrap().dpi;
        assert_eq!(info.ptMinTrackSize.x, scale(420, dpi));
        unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                scale(900, dpi),
                scale(600, dpi),
                SWP_NOMOVE | SWP_NOZORDER,
            )
        };
        let capture = state(hwnd).unwrap();
        assert_eq!(
            capture.layout.width,
            scale(900, dpi),
            "WM_SIZE laid it out again"
        );
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
    }

    #[test]
    fn the_title_footer_and_chips_say_what_and_where() {
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
            source_url: None,
        };
        assert_eq!(
            footer_text(&capture),
            "Text + screenshot + file \u{00B7} Ctrl+Enter save \u{00B7} Esc close"
        );
        assert!(footer_text(&Capture::default()).starts_with("Empty"));
    }

    /// What a test sink filed, and into which notebook.
    pub(super) type Filed = Rc<RefCell<Vec<(Capture, PathBuf)>>>;

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

    /// The autocorrect tests need Windows' English checker with its autocorrect list, which
    /// Windows Server (CI's runners) lacks.
    fn no_speller() -> bool {
        let missing = crate::platform::spell::with_english(|s| s.replacement("teh"))
            .flatten()
            .is_none();
        if missing {
            eprintln!("skipped: no en-US autocorrect list");
        }
        missing
    }

    /// A key pressed in the edit: its WM_KEYDOWN, as the keyboard sends it.
    fn key_down(edit: HWND, key: u16) {
        unsafe { SendMessageW(edit, WM_KEYDOWN, usize::from(key), 0) };
    }

    /// Ctrl+Z in the edit: WM_KEYDOWN with Ctrl held, then its WM_CHAR (0x1A).
    fn press_ctrl_z(edit: HWND) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardState, SetKeyboardState};
        let mut keys = [0u8; 256];
        unsafe { GetKeyboardState(keys.as_mut_ptr()) };
        let saved = keys;
        keys[usize::from(VK_CONTROL)] = 0x80;
        unsafe {
            SetKeyboardState(keys.as_ptr());
            SendMessageW(edit, WM_KEYDOWN, usize::from(b'Z'), 0);
            SendMessageW(edit, WM_CHAR, 0x1A, 0);
            SetKeyboardState(saved.as_ptr());
        }
    }

    fn open_box_off(draft_dir: &Path) -> (HWND, Filed) {
        open_box_full(draft_dir, None, None, false)
    }

    #[test]
    fn a_typed_typo_is_fixed_and_ctrl_z_brings_it_back() {
        if no_speller() {
            return;
        }
        let drafts = temp("fix");
        let (hwnd, _) = open_box(&drafts, None);
        type_for_test(hwnd, "I think teh ");
        let edit = state(hwnd).unwrap().edit;
        assert_eq!(edit_text(edit), "I think the ");
        press_ctrl_z(edit);
        assert_eq!(edit_text(edit), "I think teh ");
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
    }

    #[test]
    fn ctrl_z_after_a_fix_made_by_enter_keeps_the_line_break() {
        if no_speller() {
            return;
        }
        let drafts = temp("enter");
        let (hwnd, _) = open_box(&drafts, None);
        let edit = state(hwnd).unwrap().edit;
        type_for_test(hwnd, "teh\r");
        assert_eq!(edit_text(edit), "the\n");
        press_ctrl_z(edit);
        assert_eq!(edit_text(edit), "teh\n");
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
    }

    #[test]
    fn all_capitals_are_left_alone_as_windows_does() {
        if no_speller() {
            return;
        }
        let drafts = temp("caps");
        let (hwnd, _) = open_box(&drafts, None);
        type_for_test(hwnd, "CNA ADN Teh ");
        assert_eq!(edit_text(state(hwnd).unwrap().edit), "CNA ADN The ");
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
    }

    #[test]
    fn the_close_button_keeps_its_corner() {
        let drafts = temp("corner");
        let (hwnd, _) = open_box(&drafts, None);
        let mut frame: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(hwnd, &mut frame) };
        let (x, y) = (frame.right - 1, frame.top + 1);
        let hit = unsafe {
            SendMessageW(
                hwnd,
                WM_NCHITTEST,
                0,
                (((y as u32) << 16) | (x as u32 & 0xFFFF)) as isize,
            )
        };
        assert_eq!(hit, HTCLIENT as isize);
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
    }

    #[test]
    fn moving_the_box_does_not_rewrite_its_size() {
        use windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE;
        let drafts = temp("move");
        std::fs::create_dir_all(&drafts).unwrap();
        let ini = drafts.join("fastcapture.ini");
        let (hwnd, _) = open_box_with(&drafts, Some(&ini));
        unsafe {
            SendMessageW(hwnd, WM_ENTERSIZEMOVE, 0, 0);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                10,
                10,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER,
            );
            SendMessageW(hwnd, WM_EXITSIZEMOVE, 0, 0);
            DestroyWindow(hwnd);
        }
        assert!(!ini.exists(), "a move writes nothing");
        draft::clear(&drafts);
    }

    #[test]
    fn a_word_in_the_middle_is_fixed_in_place() {
        if no_speller() {
            return;
        }
        let drafts = temp("middle");
        let (hwnd, _) = open_box(&drafts, None);
        let edit = state(hwnd).unwrap().edit;
        set_edit_text(edit, "say teh world");
        unsafe { SendMessageW(edit, EM_SETSEL, 7, 7) };
        type_for_test(hwnd, ",");
        assert_eq!(edit_text(edit), "say the, world");
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
    }

    #[test]
    fn fixes_land_after_non_ascii_text() {
        if no_speller() {
            return;
        }
        let drafts = temp("unicode");
        let (hwnd, _) = open_box(&drafts, None);
        type_for_test(hwnd, "café 😀 teh.");
        assert_eq!(edit_text(state(hwnd).unwrap().edit), "café 😀 the.");
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
    }

    #[test]
    fn ctrl_z_after_more_typing_does_not_bring_the_typo_back() {
        if no_speller() {
            return;
        }
        let drafts = temp("later");
        let (hwnd, _) = open_box(&drafts, None);
        let edit = state(hwnd).unwrap().edit;
        type_for_test(hwnd, "teh ");
        key_down(edit, u16::from(b'X'));
        type_for_test(hwnd, "x");
        press_ctrl_z(edit);
        assert!(!edit_text(edit).contains("teh"), "{}", edit_text(edit));
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
    }

    #[test]
    fn pasted_restored_and_switched_off_text_is_left_alone() {
        if no_speller() {
            return;
        }
        let drafts = temp("alone");
        let (hwnd, _) = open_box(&drafts, None);
        insert_text(hwnd, "teh ");
        assert_eq!(edit_text(state(hwnd).unwrap().edit), "teh ");
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
        let (hwnd, _) = open_box_off(&drafts);
        type_for_test(hwnd, "teh ");
        assert_eq!(edit_text(state(hwnd).unwrap().edit), "teh ");
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
    }

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fastcapture-box-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    pub(super) fn open_box(draft_dir: &Path, fail: Option<&str>) -> (HWND, Filed) {
        open_box_full(draft_dir, fail, None, true)
    }

    /// A box that keeps its size in `settings`.
    fn open_box_with(draft_dir: &Path, settings: Option<&Path>) -> (HWND, Filed) {
        open_box_full(draft_dir, None, settings, true)
    }

    fn open_box_full(
        draft_dir: &Path,
        fail: Option<&str>,
        settings: Option<&Path>,
        autocorrect: bool,
    ) -> (HWND, Filed) {
        let filed = Rc::new(RefCell::new(Vec::new()));
        let hwnd = open(Open {
            sink: Box::new(Recorder {
                filed: filed.clone(),
                fail: fail.map(str::to_owned),
                notebooks: vec![PathBuf::from(r"D:\Notes"), PathBuf::from(r"E:\Work")],
            }),
            notebook: PathBuf::from(r"D:\Notes"),
            draft_dir: Some(draft_dir.to_path_buf()),
            source_window: Some("Grafana".to_owned()),
            settings: settings.map(Path::to_path_buf),
            autocorrect,
        })
        .unwrap();
        (hwnd, filed)
    }

    #[test]
    fn the_saved_size_is_used_when_it_is_big_enough() {
        let work = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        assert_eq!(initial_size(None, 96, work), (720, 420));
        assert_eq!(initial_size(None, 144, work), (1080, 630));
        assert_eq!(initial_size(Some((900, 500)), 96, work), (900, 500));
        assert_eq!(
            initial_size(Some((300, 500)), 96, work),
            (720, 420),
            "too narrow"
        );
    }

    #[test]
    fn a_saved_size_is_clamped_to_the_work_area() {
        let small = RECT {
            left: 1920,
            top: 0,
            right: 3200,
            bottom: 760,
        };
        assert_eq!(initial_size(Some((1800, 1000)), 96, small), (1280, 760));
    }

    #[test]
    fn a_resized_box_reopens_at_its_size() {
        use windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE;
        let drafts = temp("size");
        std::fs::create_dir_all(&drafts).unwrap();
        let ini = drafts.join("fastcapture.ini");
        let (hwnd, _) = open_box_with(&drafts, Some(&ini));
        let dpi = state(hwnd).unwrap().dpi;
        unsafe {
            SendMessageW(hwnd, WM_ENTERSIZEMOVE, 0, 0);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                scale(800, dpi),
                scale(500, dpi),
                SWP_NOMOVE | SWP_NOZORDER,
            );
            SendMessageW(hwnd, WM_EXITSIZEMOVE, 0, 0);
            DestroyWindow(hwnd);
        }
        let file = SettingsFile::load(&ini);
        assert_eq!(
            file.value("capture_width"),
            Some(scale(800, dpi).to_string())
        );
        let (hwnd, _) = open_box_with(&drafts, Some(&ini));
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(hwnd, &mut rect) };
        assert_eq!(
            (rect.right - rect.left, rect.bottom - rect.top),
            (scale(800, dpi), scale(500, dpi))
        );
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
    }

    #[test]
    fn a_dpi_change_rescales_the_box() {
        let drafts = temp("dpi");
        let (hwnd, _) = open_box(&drafts, None);
        let suggested = RECT {
            left: 0,
            top: 0,
            right: 1080,
            bottom: 630,
        };
        unsafe {
            SendMessageW(
                hwnd,
                WM_DPICHANGED,
                (144 << 16) | 144,
                &raw const suggested as isize,
            )
        };
        let capture = state(hwnd).unwrap();
        assert_eq!(capture.dpi, 144);
        assert_eq!(capture.layout.edit.top, scale(34 + 6, 144));
        unsafe { DestroyWindow(hwnd) };
        draft::clear(&drafts);
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
    fn an_open_box_follows_a_theme_change() {
        use crate::ui::theme::{DARK, LIGHT, ThemePreference};
        theme::set_system_for_test(theme::System::default());
        theme::set_preference(ThemePreference::Light);
        let drafts = temp("theme");
        let (hwnd, _) = open_box(&drafts, None);
        assert_eq!(state(hwnd).unwrap().colors, LIGHT);
        let brush = state(hwnd).unwrap().edit_brush;
        theme::set_preference(ThemePreference::Dark);
        unsafe { SendMessageW(hwnd, theme::changed_message(), 0, 0) };
        let capture = state(hwnd).unwrap();
        assert_eq!(capture.colors, DARK);
        assert_ne!(
            capture.edit_brush, brush,
            "the edit's brush is the dark one"
        );
        theme::set_preference(ThemePreference::System);
        unsafe { DestroyWindow(hwnd) };
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

#[cfg(test)]
mod preparing_tests {
    use super::tests::open_box;
    use super::*;
    use crate::platform::clipboard::{self, tests::Backup};

    /// A 2 × 1 32-bit bottom-up DIB, as the PrintScreen key puts on the clipboard.
    fn dib() -> Vec<u8> {
        let mut bytes = Vec::new();
        for value in [40u32, 2, 1] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(&[0; 24]);
        bytes.extend_from_slice(&[255, 0, 0, 255, 0, 255, 0, 255]);
        bytes
    }

    /// Runs this thread's messages until `done` or two seconds pass.
    fn pump_until(done: impl Fn() -> bool) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{PM_REMOVE, PeekMessageW};
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !done() && std::time::Instant::now() < until {
            let mut message: MSG = unsafe { std::mem::zeroed() };
            while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0
            {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&message);
                    windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&message);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    use windows_sys::Win32::UI::WindowsAndMessaging::MSG;

    #[test]
    fn a_pasted_bitmap_is_encoded_off_the_ui_thread_and_ctrl_enter_waits_for_it() {
        let _backup = Backup::take();
        let format = u32::from(windows_sys::Win32::System::Ole::CF_DIB);
        clipboard::set(std::ptr::null_mut(), format, &dib()).unwrap();
        let drafts =
            std::env::temp_dir().join(format!("fastcapture-box-preparing-{}", std::process::id()));
        let (hwnd, filed) = open_box(&drafts, None);
        assert!(
            paste(hwnd, false),
            "a lone image is attached, not pasted as text"
        );
        assert_eq!(state(hwnd).unwrap().preparing, 1);
        assert!(state(hwnd).unwrap().items.is_empty());
        submit(hwnd);
        assert!(filed.borrow().is_empty(), "waits for the image");
        pump_until(|| !filed.borrow().is_empty());
        let filed = filed.borrow();
        assert_eq!(filed.len(), 1);
        match &filed[0].0.attachments[..] {
            [Attachment::Png(png)] => assert!(png.starts_with(b"\x89PNG")),
            other => panic!("{other:?}"),
        }
        draft::clear(&drafts);
    }

    #[test]
    fn rich_text_pastes_as_markdown_and_ctrl_alt_v_as_plain_text() {
        let _backup = Backup::take();
        let drafts =
            std::env::temp_dir().join(format!("fastcapture-box-rich-{}", std::process::id()));
        let (hwnd, filed) = open_box(&drafts, None);
        let edit = state(hwnd).unwrap().edit;
        clipboard::set_html(
            std::ptr::null_mut(),
            "<h2>Lag</h2><ul><li>one</li><li><a href='/b'>two</a></li></ul>",
            "Lag\r\none\r\ntwo",
            Some("https://example.com/a"),
        );
        paste_for_test(hwnd);
        assert_eq!(
            edit_text(edit),
            "## Lag\n\n- one\n- [two](https://example.com/b)"
        );
        run(hwnd, Action::PastePlain);
        assert_eq!(
            edit_text(edit),
            "## Lag\n\n- one\n- [two](https://example.com/b)Lag\none\ntwo"
        );

        // Plain paragraphs paste as the plain text beside them; the page is still kept.
        set_edit_text(edit, "");
        clipboard::set_html(
            std::ptr::null_mut(),
            "<p>Just <b>a</b> line</p>",
            "Just a line",
            Some("https://example.com/c"),
        );
        paste_for_test(hwnd);
        assert_eq!(edit_text(edit), "Just a line");
        submit(hwnd);
        let filed = filed.borrow();
        assert_eq!(
            filed[0].0.source_url.as_deref(),
            Some("https://example.com/c")
        );
        draft::clear(&drafts);
    }
}
