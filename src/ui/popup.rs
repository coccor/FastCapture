//! Menus and questions in the theme, in place of Windows' popup menus and message boxes, which
//! stay light in a dark theme.
//!
//! A menu is an owner-drawn list box in a borderless popup: screen readers read it as a list of
//! named items, and it is driven from the keyboard (Up and Down skip separators and disabled
//! items, Enter or a click chooses, an item's `&` letter chooses it, Esc or a click elsewhere
//! closes). A question is a framed window with its text and painted buttons, modal over its
//! owner: Enter takes the default button, Esc the cancel one, Tab moves between them.

// Window handles are only passed on to Windows, which checks them.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use super::controls::{self, Look, Surface};
use super::frame::{self, Buttons, Frame};
use super::paint::{self, Align, Corners, GLYPH_CHECK, rect};
use super::theme::metrics::{
    CONTROL_RADIUS, DIALOG_PADDING, FOOTER, ROW_INSET_X, ROW_INSET_Y, scale,
};
use super::theme::ramp::{self, TextStyle, scale_text};
use super::theme::{self, Palette};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUNDSMALL, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    DT_CALCRECT, DT_NOPREFIX, DT_WORDBREAK, DeleteObject, DrawTextW, GetDC, GetMonitorInfoW, HDC,
    HFONT, HGDIOBJ, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    ReleaseDC, SelectObject,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_SELECTED};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, SetFocus, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_RETURN, VK_UP,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, ES_MULTILINE, ES_READONLY, GWLP_USERDATA, GetCursorPos, GetMessageW,
    GetWindowLongPtrW, GetWindowRect, IDC_ARROW, IDCANCEL, IsDialogMessageW, IsWindow,
    IsWindowVisible, LB_ADDSTRING, LB_GETCURSEL, LB_ITEMFROMPOINT, LB_SETCURSEL, LBN_SELCHANGE,
    LBS_HASSTRINGS, LBS_NOINTEGRALHEIGHT, LBS_NOTIFY, LBS_OWNERDRAWVARIABLE, LoadCursorW, MSG,
    PostMessageW, PostQuitMessage, RegisterClassW, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER,
    SendMessageW, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    TranslateMessage, WA_INACTIVE, WM_ACTIVATE, WM_CHAR, WM_CLOSE, WM_COMMAND, WM_DPICHANGED,
    WM_DRAWITEM, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONUP, WM_MEASUREITEM, WM_MOUSEMOVE,
    WM_NCDESTROY, WM_NULL, WM_PAINT, WM_SETFONT, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_GROUP, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
    WS_VSCROLL,
};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// One line of a menu.
#[derive(Clone, Copy, Debug)]
pub enum Entry<'a> {
    /// `text` may hold an `&` before its access key, and a tab before a shortcut shown on the
    /// right (`Capture…\tCtrl+Alt+C`).
    Item {
        id: u32,
        text: &'a str,
        enabled: bool,
        checked: bool,
    },
    Separator,
}

/// Where a menu opens.
#[derive(Clone, Copy)]
pub enum Anchor {
    /// Its top-left corner at the point (above it when there is no room below).
    Below(POINT),
    /// Its bottom-left corner at the point, as the tray's menu opens above the taskbar.
    Above(POINT),
}

/// A menu's line, owned.
struct Line {
    id: u32,
    label: String,
    shortcut: String,
    key: Option<char>,
    enabled: bool,
    checked: bool,
    separator: bool,
}

impl Line {
    fn from(entry: &Entry<'_>) -> Self {
        match *entry {
            Entry::Separator => Self {
                id: 0,
                label: String::new(),
                shortcut: String::new(),
                key: None,
                enabled: false,
                checked: false,
                separator: true,
            },
            Entry::Item {
                id,
                text,
                enabled,
                checked,
            } => {
                let (text, shortcut) = text.split_once('\t').unwrap_or((text, ""));
                let key = text
                    .find('&')
                    .and_then(|at| text[at + 1..].chars().next())
                    .map(|key| key.to_ascii_lowercase());
                Self {
                    id,
                    label: text.replacen('&', "", 1),
                    shortcut: shortcut.to_owned(),
                    key,
                    enabled,
                    checked,
                    separator: false,
                }
            }
        }
    }

    fn choosable(&self) -> bool {
        self.enabled && !self.separator
    }
}

struct MenuState {
    lines: Vec<Line>,
    list: HWND,
    font: HFONT,
    glyphs: HFONT,
    dpi: u32,
    /// The last selection, to know which way the keyboard moved.
    selected: usize,
    done: bool,
    chosen: u32,
}

const MENU_CLASS: &str = "FastCaptureMenu";
const QUESTION_CLASS: &str = "FastCaptureQuestion";
const LIST_ID: usize = 1;
const LIST_HOOK: usize = 0x4643_4D4E;
/// A menu item's height, a separator's, the check column and the edges.
const ITEM: i32 = 28;
const SEPARATOR: i32 = 9;
const CHECK_COLUMN: i32 = 32;
const SHORTCUT_GAP: i32 = 32;
const EDGE: i32 = 4;

fn register(class: &str, proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT) {
    let name = wide(class);
    let description = WNDCLASSW {
        lpfnWndProc: Some(proc),
        hInstance: unsafe { GetModuleHandleW(std::ptr::null()) },
        hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
        lpszClassName: name.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    // Registering twice fails harmlessly.
    unsafe { RegisterClassW(&description) };
}

fn menu_state(window: HWND) -> Option<&'static mut MenuState> {
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *mut MenuState;
    (!pointer.is_null()).then(|| unsafe { &mut *pointer })
}

/// The work area of the monitor at `point`.
fn work_area(point: POINT) -> RECT {
    let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    unsafe { GetMonitorInfoW(monitor, &mut info) };
    info.rcWork
}

/// Where a `width` × `height` popup goes for `anchor`, kept on the monitor's work area.
pub fn place(anchor: Anchor, width: i32, height: i32, work: RECT) -> POINT {
    let (point, below) = match anchor {
        Anchor::Below(point) => (point, true),
        Anchor::Above(point) => (point, false),
    };
    let top = if below {
        if point.y + height <= work.bottom {
            point.y
        } else {
            point.y - height
        }
    } else if point.y - height >= work.top {
        point.y - height
    } else {
        point.y
    };
    let left = point.x.min(work.right - width).max(work.left);
    POINT {
        x: left,
        y: top.min(work.bottom - height).max(work.top),
    }
}

/// Opens a menu of `entries` at `anchor` without waiting for it; `run_menu` waits.
pub(crate) fn open_menu(owner: HWND, anchor: Anchor, entries: &[Entry<'_>]) -> Option<HWND> {
    register(MENU_CLASS, menu_proc);
    let point = match anchor {
        Anchor::Below(point) | Anchor::Above(point) => point,
    };
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let popup = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            wide(MENU_CLASS).as_ptr(),
            wide("Menu").as_ptr(),
            WS_POPUP | WS_CLIPCHILDREN,
            point.x,
            point.y,
            1,
            1,
            owner,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if popup.is_null() {
        return None;
    }
    let dpi = unsafe { GetDpiForWindow(popup) };
    let font = ramp::font(TextStyle::Body, dpi);
    let state = Box::new(MenuState {
        lines: entries.iter().map(Line::from).collect(),
        list: std::ptr::null_mut(),
        font,
        glyphs: ramp::icon_font(12, dpi),
        dpi,
        selected: 0,
        done: false,
        chosen: 0,
    });
    unsafe { SetWindowLongPtrW(popup, GWLP_USERDATA, Box::into_raw(state) as isize) };
    let state = menu_state(popup)?;
    let list = unsafe {
        CreateWindowExW(
            0,
            wide("LISTBOX").as_ptr(),
            std::ptr::null(),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | (LBS_NOTIFY | LBS_OWNERDRAWVARIABLE | LBS_HASSTRINGS | LBS_NOINTEGRALHEIGHT)
                    as u32,
            0,
            0,
            1,
            1,
            popup,
            LIST_ID as _,
            instance,
            std::ptr::null(),
        )
    };
    state.list = list;
    unsafe {
        SendMessageW(list, WM_SETFONT, font as usize, 0);
        SetWindowSubclass(list, Some(list_proc), LIST_HOOK, popup as usize);
    }
    // The items' text is what screen readers read.
    for line in &state.lines {
        let text = if line.separator {
            String::new()
        } else if line.shortcut.is_empty() {
            line.label.clone()
        } else {
            format!("{}, {}", line.label, line.shortcut)
        };
        unsafe { SendMessageW(list, LB_ADDSTRING, 0, wide(&text).as_ptr() as LPARAM) };
    }
    let (width, height) = menu_size(popup, state);
    let at = place(anchor, width, height, work_area(point));
    unsafe {
        SetWindowPos(
            popup,
            std::ptr::null_mut(),
            at.x,
            at.y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let edge = scale(EDGE, dpi);
        SetWindowPos(
            list,
            std::ptr::null_mut(),
            edge,
            edge,
            width - 2 * edge,
            height - 2 * edge,
            SWP_NOZORDER,
        );
        let corners = if theme::current().high_contrast {
            DWMWCP_DONOTROUND
        } else {
            DWMWCP_ROUNDSMALL
        };
        DwmSetWindowAttribute(
            popup,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&raw const corners).cast(),
            size_of_val(&corners) as u32,
        );
    }
    if let Some(first) = state.lines.iter().position(Line::choosable) {
        state.selected = first;
        unsafe { SendMessageW(list, LB_SETCURSEL, first, 0) };
    }
    unsafe {
        ShowWindow(popup, SW_SHOW);
        SetForegroundWindow(popup);
        SetFocus(list);
    }
    Some(popup)
}

/// The menu's size: the widest label and shortcut, and every line.
fn menu_size(popup: HWND, state: &MenuState) -> (i32, i32) {
    let s = |value| scale(value, state.dpi);
    let dc = unsafe { GetDC(popup) };
    let widest = |text: fn(&Line) -> &str| {
        state
            .lines
            .iter()
            .map(|line| paint::text_width(dc, text(line), state.font))
            .max()
            .unwrap_or(0)
    };
    let label = widest(|line| &line.label);
    let shortcut = widest(|line| &line.shortcut);
    unsafe { ReleaseDC(popup, dc) };
    let gap = if shortcut > 0 { s(SHORTCUT_GAP) } else { 0 };
    let width = (s(CHECK_COLUMN) + label + gap + shortcut + s(24) + 2 * s(EDGE)).max(s(200));
    let height = state
        .lines
        .iter()
        .map(|line| line_height(line, state.dpi))
        .sum::<i32>()
        + 2 * s(EDGE);
    (width, height)
}

fn line_height(line: &Line, dpi: u32) -> i32 {
    if line.separator {
        scale(SEPARATOR, dpi)
    } else {
        scale_text(ITEM, dpi)
    }
}

/// Waits for the menu opened by `open_menu` to close and returns the chosen id, or 0.
pub(crate) fn run_menu(popup: HWND, owner: HWND) -> u32 {
    let mut message: MSG = unsafe { std::mem::zeroed() };
    while menu_state(popup).is_some_and(|state| !state.done) {
        match unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } {
            0 => {
                // Quitting: the outer loop must see it too.
                unsafe { PostQuitMessage(message.wParam as i32) };
                break;
            }
            -1 => break,
            _ => unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            },
        }
    }
    let chosen = menu_state(popup).map_or(0, |state| state.chosen);
    unsafe {
        DestroyWindow(popup);
        if !owner.is_null() {
            PostMessageW(owner, WM_NULL, 0, 0);
        }
    }
    chosen
}

/// Shows a menu of `entries` at `anchor`, over `owner`, and returns the chosen id, or 0 when
/// none was chosen.
pub fn menu(owner: HWND, anchor: Anchor, entries: &[Entry<'_>]) -> u32 {
    match open_menu(owner, anchor, entries) {
        Some(popup) => run_menu(popup, owner),
        None => 0,
    }
}

/// The result of a menu that has closed: `Some(id)`, 0 for none.
#[cfg(test)]
fn menu_result(popup: HWND) -> Option<u32> {
    menu_state(popup).and_then(|state| state.done.then_some(state.chosen))
}

fn finish(popup: HWND, chosen: u32) {
    if let Some(state) = menu_state(popup)
        && !state.done
    {
        state.done = true;
        state.chosen = chosen;
        unsafe { ShowWindow(popup, 0) };
    }
}

/// Chooses the selected line, if it can be.
fn choose_selected(popup: HWND) {
    let Some(state) = menu_state(popup) else {
        return;
    };
    let index = unsafe { SendMessageW(state.list, LB_GETCURSEL, 0, 0) };
    if let Some(line) = usize::try_from(index)
        .ok()
        .and_then(|index| state.lines.get(index))
        && line.choosable()
    {
        let id = line.id;
        finish(popup, id);
    }
}

/// Moves the selection to the next line that can be chosen from `from`, `forward` or back.
fn select_next(state: &mut MenuState, from: usize, forward: bool) {
    let count = state.lines.len();
    let mut at = from;
    for _ in 0..count {
        at = if forward {
            (at + 1).min(count.saturating_sub(1))
        } else {
            at.saturating_sub(1)
        };
        if state.lines[at].choosable() {
            break;
        }
    }
    if !state.lines.get(at).is_some_and(Line::choosable) {
        at = state.selected;
    }
    state.selected = at;
    unsafe { SendMessageW(state.list, LB_SETCURSEL, at, 0) };
}

unsafe extern "system" fn list_proc(
    list: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    popup: usize,
) -> LRESULT {
    let popup = popup as HWND;
    match message {
        WM_NCDESTROY => unsafe {
            RemoveWindowSubclass(list, Some(list_proc), LIST_HOOK);
        },
        WM_KEYDOWN => {
            let key = wparam as u16;
            if key == VK_RETURN {
                choose_selected(popup);
                return 0;
            }
            if key == VK_ESCAPE {
                finish(popup, 0);
                return 0;
            }
            if let Some(state) = menu_state(popup)
                && matches!(key, VK_UP | VK_DOWN | VK_HOME | VK_END)
            {
                let count = state.lines.len();
                let (from, forward) = match key {
                    VK_UP => (state.selected, false),
                    VK_DOWN => (state.selected, true),
                    VK_HOME => (usize::MAX, true),
                    _ => (count, false),
                };
                let from = if from == usize::MAX {
                    // Home: the first line that can be chosen.
                    match state.lines.iter().position(Line::choosable) {
                        Some(first) => {
                            state.selected = first;
                            unsafe { SendMessageW(state.list, LB_SETCURSEL, first, 0) };
                            return 0;
                        }
                        None => return 0,
                    }
                } else {
                    from
                };
                select_next(state, from.min(count), forward);
                return 0;
            }
        }
        WM_CHAR => {
            if let Some(state) = menu_state(popup)
                && let Some(typed) = char::from_u32(wparam as u32).map(|c| c.to_ascii_lowercase())
                && let Some(index) = state
                    .lines
                    .iter()
                    .position(|line| line.choosable() && line.key == Some(typed))
            {
                state.selected = index;
                unsafe { SendMessageW(state.list, LB_SETCURSEL, index, 0) };
                choose_selected(popup);
            }
            return 0;
        }
        WM_MOUSEMOVE | WM_LBUTTONUP => {
            let found = unsafe { SendMessageW(list, LB_ITEMFROMPOINT, 0, lparam) } as u32;
            let index = (found & 0xFFFF) as usize;
            let outside = found >> 16 != 0;
            if let Some(state) = menu_state(popup)
                && !outside
                && state.lines.get(index).is_some_and(Line::choosable)
            {
                if state.selected != index {
                    state.selected = index;
                    unsafe { SendMessageW(list, LB_SETCURSEL, index, 0) };
                }
                if message == WM_LBUTTONUP {
                    choose_selected(popup);
                }
            }
            if message == WM_LBUTTONUP {
                return 0;
            }
        }
        _ => {}
    }
    unsafe { DefSubclassProc(list, message, wparam, lparam) }
}

fn draw_line(item: &DRAWITEMSTRUCT, state: &MenuState, palette: &Palette) {
    let Some(line) = state.lines.get(item.itemID as usize) else {
        return;
    };
    let dc = item.hDC;
    let area = item.rcItem;
    let s = |value| scale(value, state.dpi);
    paint::fill(dc, area, palette.content_background);
    if line.separator {
        let middle = (area.top + area.bottom) / 2;
        paint::fill(
            dc,
            rect(area.left + s(8), middle, area.right - s(8), middle + 1),
            palette.stroke,
        );
        return;
    }
    let selected = item.itemState & ODS_SELECTED != 0 && line.enabled;
    let ink = if !line.enabled {
        palette.disabled_foreground
    } else if selected {
        palette.hover_foreground
    } else {
        palette.content_foreground
    };
    if selected {
        let plate = rect(
            area.left + s(ROW_INSET_X) - s(EDGE),
            area.top + s(ROW_INSET_Y),
            area.right - s(ROW_INSET_X) + s(EDGE),
            area.bottom - s(ROW_INSET_Y),
        );
        let radius = paint::radius(palette, CONTROL_RADIUS, state.dpi);
        paint::fill_rounded(
            dc,
            plate,
            radius,
            Corners::ALL,
            palette.hover_background,
            palette.content_background,
        );
    }
    if line.checked {
        let check = rect(
            area.left,
            area.top,
            area.left + s(CHECK_COLUMN),
            area.bottom,
        );
        paint::text(dc, GLYPH_CHECK, check, state.glyphs, ink, Align::Center);
    }
    let label = rect(
        area.left + s(CHECK_COLUMN),
        area.top,
        area.right - s(12),
        area.bottom,
    );
    paint::text(dc, &line.label, label, state.font, ink, Align::Left);
    if !line.shortcut.is_empty() {
        let width = paint::text_width(dc, &line.shortcut, state.font);
        let right = rect(
            area.right - s(12) - width,
            area.top,
            area.right - s(12),
            area.bottom,
        );
        let muted = if line.enabled {
            palette.muted_foreground
        } else {
            palette.disabled_foreground
        };
        paint::text(dc, &line.shortcut, right, state.font, muted, Align::Left);
    }
}

unsafe extern "system" fn menu_proc(
    popup: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_MEASUREITEM => {
            let measure = unsafe { &mut *(lparam as *mut MEASUREITEMSTRUCT) };
            if let Some(state) = menu_state(popup)
                && let Some(line) = state.lines.get(measure.itemID as usize)
            {
                measure.itemHeight = line_height(line, state.dpi) as u32;
            }
            return 1;
        }
        WM_DRAWITEM => {
            let item = unsafe { &*(lparam as *const DRAWITEMSTRUCT) };
            if let Some(state) = menu_state(popup) {
                draw_line(item, state, &theme::current());
            }
            return 1;
        }
        WM_COMMAND if (wparam >> 16) as u32 == LBN_SELCHANGE => {
            // A selection the keyboard moved onto a line that can't be chosen moves on.
            if let Some(state) = menu_state(popup) {
                let index = unsafe { SendMessageW(state.list, LB_GETCURSEL, 0, 0) };
                if let Ok(index) = usize::try_from(index) {
                    if state.lines.get(index).is_some_and(Line::choosable) {
                        state.selected = index;
                    } else {
                        let forward = index > state.selected;
                        select_next(state, index, forward);
                    }
                }
            }
            return 0;
        }
        WM_ERASEBKGND => return 1,
        WM_PAINT => {
            let palette = theme::current();
            paint::buffered(popup, |dc, area| {
                paint::fill(dc, area, palette.content_background);
                paint::stroke_ring(dc, area, 0, 1, palette.stroke);
            });
            return 0;
        }
        WM_ACTIVATE if (wparam & 0xFFFF) as u32 == WA_INACTIVE => {
            // A click elsewhere.
            finish(popup, 0);
        }
        WM_NCDESTROY => {
            let pointer = unsafe { SetWindowLongPtrW(popup, GWLP_USERDATA, 0) } as *mut MenuState;
            if !pointer.is_null() {
                let state = unsafe { Box::from_raw(pointer) };
                unsafe {
                    DeleteObject(state.font as HGDIOBJ);
                    DeleteObject(state.glyphs as HGDIOBJ);
                }
            }
        }
        _ => {}
    }
    if let Some(answer) = controls::colors(message, wparam, lparam, Surface::Content) {
        return answer;
    }
    unsafe { DefWindowProcW(popup, message, wparam, lparam) }
}

// ---------------------------------------------------------------------------------------------
// Questions.

/// A question and its answers.
pub struct Question<'a> {
    pub title: &'a str,
    pub text: &'a str,
    /// The buttons, left to right.
    pub buttons: &'a [&'a str],
    /// The button Enter takes.
    pub default: usize,
    /// The answer for Esc and the close button.
    pub cancel: usize,
}

struct QuestionState {
    frame: Frame,
    font: HFONT,
    /// The question, to wrap again at another DPI.
    text: String,
    field: HWND,
    buttons: Vec<HWND>,
    cancel: usize,
    answer: Option<usize>,
}

const BUTTON_ID: usize = 100;
const WIDTH: i32 = 460;
/// The tallest the text gets before it scrolls.
const MOST_TEXT: i32 = 320;

fn question_state(window: HWND) -> Option<&'static mut QuestionState> {
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *mut QuestionState;
    (!pointer.is_null()).then(|| unsafe { &mut *pointer })
}

/// How tall `text` is in `font` when wrapped to `width`.
fn text_height(window: HWND, text: &str, font: HFONT, width: i32) -> i32 {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut area = rect(0, 0, width, 0);
    let dc = unsafe { GetDC(window) };
    unsafe {
        let previous = SelectObject(dc, font as HGDIOBJ);
        DrawTextW(
            dc,
            wide.as_ptr(),
            wide.len() as i32,
            &mut area,
            DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
        );
        SelectObject(dc, previous);
        ReleaseDC(window, dc);
    }
    area.bottom
}

/// The width of the question's text.
fn question_inner(dpi: u32) -> i32 {
    scale(WIDTH, dpi) - 2 * scale(DIALOG_PADDING, dpi)
}

/// Places the text and the buttons at `dpi` in the state's font, and returns the window's size.
fn lay_out_question(window: HWND, state: &QuestionState, dpi: u32) -> (i32, i32) {
    let s = |value| scale(value, dpi);
    let width = s(WIDTH);
    let pad = s(DIALOG_PADDING);
    let inner = question_inner(dpi);
    let wrapped = text_height(window, &state.text, state.font, inner - s(4));
    let text_height = wrapped.min(s(MOST_TEXT)) + s(4);
    let top = state.frame.height() + s(8);
    let height = top + text_height + s(16) + s(FOOTER);
    unsafe {
        SendMessageW(state.field, WM_SETFONT, state.font as usize, 0);
        SetWindowPos(
            state.field,
            std::ptr::null_mut(),
            pad,
            top,
            inner,
            text_height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    let button_width = s(96);
    let button_height = s(26);
    let gap = s(8);
    let count = state.buttons.len() as i32;
    let mut left = width - pad - count * button_width - (count - 1).max(0) * gap;
    for &button in &state.buttons {
        unsafe {
            SendMessageW(button, WM_SETFONT, state.font as usize, 0);
            SetWindowPos(
                button,
                std::ptr::null_mut(),
                left,
                height - s(FOOTER) + (s(FOOTER) - button_height) / 2 - s(4),
                button_width,
                button_height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        left += button_width + gap;
    }
    (width, height)
}

/// Follows the question to a monitor with another scale: the fonts, the strip and the layout at
/// the new DPI, where Windows suggests.
fn change_question_dpi(window: HWND, dpi: u32, suggested: &RECT) {
    let Some(state) = question_state(window) else {
        return;
    };
    state.frame.set_dpi(dpi);
    let font = ramp::font(TextStyle::Body, dpi);
    let previous = std::mem::replace(&mut state.font, font);
    let (width, height) = lay_out_question(window, state, dpi);
    unsafe {
        DeleteObject(previous as HGDIOBJ);
        SetWindowPos(
            window,
            std::ptr::null_mut(),
            suggested.left,
            suggested.top,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        InvalidateRect(window, std::ptr::null(), 1);
    }
}

/// Opens `question` over `owner` without waiting; `run_question` waits.
pub(crate) fn open_question(owner: HWND, question: &Question<'_>) -> Option<HWND> {
    register(QUESTION_CLASS, question_proc);
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let window = unsafe {
        CreateWindowExW(
            0,
            wide(QUESTION_CLASS).as_ptr(),
            wide(question.title).as_ptr(),
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
            0,
            0,
            1,
            1,
            owner,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if window.is_null() {
        return None;
    }
    let dpi = unsafe { GetDpiForWindow(window) };
    let font = ramp::font(TextStyle::Body, dpi);
    let state = Box::new(QuestionState {
        frame: Frame::new(question.title, Buttons::default(), false, dpi),
        font,
        text: question.text.to_owned(),
        field: std::ptr::null_mut(),
        buttons: Vec::new(),
        cancel: question.cancel,
        answer: None,
    });
    unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, Box::into_raw(state) as isize) };
    let state = question_state(window)?;
    frame::attach(window, &theme::current());
    let scrolls = text_height(
        window,
        question.text,
        font,
        question_inner(dpi) - scale(4, dpi),
    ) > scale(MOST_TEXT, dpi);
    let text = question.text.replace("\r\n", "\n").replace('\n', "\r\n");
    state.field = unsafe {
        CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            wide(&text).as_ptr(),
            WS_CHILD
                | WS_VISIBLE
                | (ES_MULTILINE | ES_READONLY) as u32
                | if scrolls { WS_VSCROLL } else { 0 },
            0,
            0,
            0,
            0,
            window,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    controls::theme_scrollbars(state.field, &theme::current());
    for (index, label) in question.buttons.iter().enumerate() {
        let kind = if index == question.default {
            BS_DEFPUSHBUTTON
        } else {
            BS_PUSHBUTTON
        };
        let flags = if index == 0 { WS_GROUP } else { 0 };
        let button = unsafe {
            CreateWindowExW(
                0,
                wide("BUTTON").as_ptr(),
                wide(label).as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | flags | kind as u32,
                0,
                0,
                0,
                0,
                window,
                (BUTTON_ID + index) as _,
                instance,
                std::ptr::null(),
            )
        };
        controls::paint_as(button, Look::Button, Surface::Content);
        state.buttons.push(button);
    }
    let (width, height) = lay_out_question(window, state, dpi);
    // Centered over the owner when it is showing, else on the monitor under the mouse.
    let mut over = rect(0, 0, 0, 0);
    let showing = !owner.is_null() && unsafe { IsWindowVisible(owner) } != 0;
    if showing {
        unsafe { GetWindowRect(owner, &mut over) };
    } else {
        let mut cursor = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut cursor) };
        over = work_area(cursor);
    }
    let x = over.left + (over.right - over.left - width) / 2;
    let y = over.top + (over.bottom - over.top - height) / 3;
    unsafe {
        SetWindowPos(
            window,
            std::ptr::null_mut(),
            x,
            y,
            width,
            height,
            SWP_NOZORDER,
        );
        ShowWindow(window, SW_SHOW);
        SetForegroundWindow(window);
        if let Some(button) = state.buttons.get(question.default) {
            SetFocus(*button);
        }
    }
    Some(window)
}

/// Waits for the question opened by `open_question` to be answered; its owner can't be used
/// meanwhile.
pub(crate) fn run_question(window: HWND, owner: HWND) -> usize {
    let cancel = question_state(window).map_or(0, |state| state.cancel);
    let owned = !owner.is_null() && unsafe { IsWindow(owner) } != 0;
    if owned {
        unsafe { EnableWindow(owner, 0) };
    }
    let mut message: MSG = unsafe { std::mem::zeroed() };
    while question_state(window).is_some_and(|state| state.answer.is_none()) {
        match unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } {
            0 => {
                unsafe { PostQuitMessage(message.wParam as i32) };
                break;
            }
            -1 => break,
            _ => unsafe {
                if IsDialogMessageW(window, &message) == 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            },
        }
    }
    let answer = question_state(window)
        .and_then(|state| state.answer)
        .unwrap_or(cancel);
    unsafe {
        if owned {
            EnableWindow(owner, 1);
            SetForegroundWindow(owner);
        }
        DestroyWindow(window);
    }
    answer
}

/// Asks `question` over `owner` and returns the button chosen.
pub fn ask(owner: HWND, question: &Question<'_>) -> usize {
    match open_question(owner, question) {
        Some(window) => run_question(window, owner),
        None => question.cancel,
    }
}

#[cfg(test)]
fn answer_of(window: HWND) -> Option<usize> {
    question_state(window).and_then(|state| state.answer)
}

fn answer(window: HWND, index: usize) {
    if let Some(state) = question_state(window)
        && state.answer.is_none()
    {
        state.answer = Some(index);
        unsafe { ShowWindow(window, 0) };
    }
}

unsafe extern "system" fn question_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(state) = question_state(window)
        && let Some(result) = state.frame.handle(window, message, wparam, lparam)
    {
        return result;
    }
    if let Some(result) = controls::colors(message, wparam, lparam, Surface::Content) {
        return result;
    }
    match message {
        WM_COMMAND => {
            let id = wparam & 0xFFFF;
            if let Some(state) = question_state(window) {
                if id == IDCANCEL as usize {
                    let cancel = state.cancel;
                    answer(window, cancel);
                } else if (BUTTON_ID..BUTTON_ID + state.buttons.len()).contains(&id) {
                    answer(window, id - BUTTON_ID);
                }
            }
            0
        }
        WM_CLOSE => {
            if let Some(state) = question_state(window) {
                let cancel = state.cancel;
                answer(window, cancel);
            }
            0
        }
        WM_DPICHANGED => {
            let dpi = u32::from((wparam & 0xFFFF) as u16).max(96);
            change_question_dpi(window, dpi, unsafe { &*(lparam as *const RECT) });
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let palette = theme::current();
            if let Some(state) = question_state(window) {
                paint::buffered(window, |dc: HDC, area| {
                    paint::fill(dc, area, palette.content_background);
                    let footer = rect(
                        0,
                        area.bottom - scale(FOOTER, unsafe { GetDpiForWindow(window) }),
                        area.right,
                        area.bottom,
                    );
                    paint::fill(dc, footer, palette.strip_background);
                    state.frame.paint(dc, area.right, &palette, false);
                });
            }
            0
        }
        _ if message == theme::changed_message() => {
            frame::apply_theme(window, &theme::current());
            controls::retheme(window);
            0
        }
        WM_NCDESTROY => {
            let pointer =
                unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, 0) } as *mut QuestionState;
            if !pointer.is_null() {
                let state = unsafe { Box::from_raw(pointer) };
                unsafe { DeleteObject(state.font as HGDIOBJ) };
            }
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::ThemePreference;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

    fn entries() -> [Entry<'static>; 5] {
        [
            Entry::Item {
                id: 1,
                text: "&Capture…\tCtrl+Alt+C",
                enabled: true,
                checked: false,
            },
            Entry::Item {
                id: 2,
                text: "Review (3)",
                enabled: false,
                checked: false,
            },
            Entry::Separator,
            Entry::Item {
                id: 3,
                text: "&Settings…",
                enabled: true,
                checked: true,
            },
            Entry::Item {
                id: 4,
                text: "&Quit",
                enabled: true,
                checked: false,
            },
        ]
    }

    fn list_of(popup: HWND) -> HWND {
        menu_state(popup).unwrap().list
    }

    fn selected(popup: HWND) -> usize {
        unsafe { SendMessageW(list_of(popup), LB_GETCURSEL, 0, 0) as usize }
    }

    #[test]
    fn a_line_keeps_its_access_key_and_shortcut_apart() {
        let line = Line::from(&entries()[0]);
        assert_eq!(line.label, "Capture…");
        assert_eq!(line.shortcut, "Ctrl+Alt+C");
        assert_eq!(line.key, Some('c'));
        assert!(line.choosable());
        assert!(!Line::from(&entries()[1]).choosable(), "disabled");
        assert!(!Line::from(&Entry::Separator).choosable());
    }

    #[test]
    fn a_menu_opens_where_it_fits() {
        let work = rect(0, 0, 1920, 1040);
        let below = place(Anchor::Below(POINT { x: 100, y: 100 }), 200, 300, work);
        assert_eq!((below.x, below.y), (100, 100));
        let flipped = place(Anchor::Below(POINT { x: 100, y: 900 }), 200, 300, work);
        assert_eq!(flipped.y, 600, "no room below: above the point");
        let tray = place(Anchor::Above(POINT { x: 1900, y: 1040 }), 200, 300, work);
        assert_eq!(
            (tray.x, tray.y),
            (1720, 740),
            "kept on the screen, above the taskbar"
        );
    }

    #[test]
    fn the_keyboard_skips_what_can_t_be_chosen_and_enter_chooses() {
        theme::set_preference(ThemePreference::Dark);
        let popup = open_menu(
            std::ptr::null_mut(),
            Anchor::Below(POINT { x: 10, y: 10 }),
            &entries(),
        )
        .unwrap();
        assert_eq!(selected(popup), 0, "the first item");
        let list = list_of(popup);
        unsafe { SendMessageW(list, WM_KEYDOWN, VK_DOWN as WPARAM, 0) };
        assert_eq!(
            selected(popup),
            3,
            "past the disabled item and the separator"
        );
        unsafe { SendMessageW(list, WM_KEYDOWN, VK_UP as WPARAM, 0) };
        assert_eq!(selected(popup), 0);
        unsafe { SendMessageW(list, WM_KEYDOWN, VK_END as WPARAM, 0) };
        assert_eq!(selected(popup), 4);
        assert_eq!(menu_result(popup), None, "still open");
        unsafe { SendMessageW(list, WM_KEYDOWN, VK_RETURN as WPARAM, 0) };
        assert_eq!(menu_result(popup), Some(4));
        unsafe { DestroyWindow(popup) };

        let popup = open_menu(
            std::ptr::null_mut(),
            Anchor::Below(POINT { x: 10, y: 10 }),
            &entries(),
        )
        .unwrap();
        unsafe { SendMessageW(list_of(popup), WM_CHAR, 's' as WPARAM, 0) };
        assert_eq!(menu_result(popup), Some(3), "its access key");
        unsafe { DestroyWindow(popup) };

        let popup = open_menu(
            std::ptr::null_mut(),
            Anchor::Below(POINT { x: 10, y: 10 }),
            &entries(),
        )
        .unwrap();
        unsafe { SendMessageW(list_of(popup), WM_KEYDOWN, VK_ESCAPE as WPARAM, 0) };
        assert_eq!(menu_result(popup), Some(0), "Esc chooses nothing");
        unsafe { DestroyWindow(popup) };
        theme::set_preference(ThemePreference::System);
    }

    #[test]
    fn a_menu_is_painted_in_the_theme() {
        theme::set_preference(ThemePreference::Dark);
        let popup = open_menu(
            std::ptr::null_mut(),
            Anchor::Below(POINT { x: 10, y: 10 }),
            &entries(),
        )
        .unwrap();
        let list = list_of(popup);
        let mut area = rect(0, 0, 0, 0);
        unsafe { GetClientRect(list, &mut area) };
        let canvas = crate::ui::paint::tests::Canvas::new(area.right, area.bottom);
        unsafe {
            SendMessageW(
                list,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_PRINTCLIENT,
                canvas.dc as WPARAM,
                windows_sys::Win32::UI::WindowsAndMessaging::PRF_CLIENT as LPARAM,
            )
        };
        let palette = theme::current();
        assert_eq!(
            canvas.pixel(area.right / 2, 2),
            palette.hover_background,
            "the selected first item"
        );
        let line = scale_text(ITEM, 96) + scale(SEPARATOR, 96) / 2 + scale_text(ITEM, 96);
        assert_eq!(
            canvas.pixel(area.right / 2, line),
            palette.stroke,
            "the separator"
        );
        unsafe { DestroyWindow(popup) };
        theme::set_preference(ThemePreference::System);
    }

    #[test]
    fn a_question_answers_with_the_button_enter_or_esc() {
        let question = Question {
            title: "Discard this note?",
            text: "The note goes to the Recycle Bin.",
            buttons: &["Discard", "Keep"],
            default: 1,
            cancel: 1,
        };
        let window = open_question(std::ptr::null_mut(), &question).unwrap();
        let state = question_state(window).unwrap();
        assert_eq!(state.buttons.len(), 2);
        let style = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongW(
                state.buttons[1],
                windows_sys::Win32::UI::WindowsAndMessaging::GWL_STYLE,
            )
        } as u32;
        assert_eq!(style & 0xF, BS_DEFPUSHBUTTON as u32, "Keep is the default");
        assert_eq!(answer_of(window), None);
        unsafe { SendMessageW(window, WM_COMMAND, BUTTON_ID, 0) };
        assert_eq!(answer_of(window), Some(0), "Discard");
        unsafe { DestroyWindow(window) };

        let window = open_question(std::ptr::null_mut(), &question).unwrap();
        unsafe { SendMessageW(window, WM_COMMAND, IDCANCEL as WPARAM, 0) };
        assert_eq!(answer_of(window), Some(1), "Esc");
        unsafe { DestroyWindow(window) };

        let window = open_question(std::ptr::null_mut(), &question).unwrap();
        unsafe { SendMessageW(window, WM_CLOSE, 0, 0) };
        assert_eq!(answer_of(window), Some(1), "the close button");
        unsafe { DestroyWindow(window) };

        let long = "A line of the first-use screen. ".repeat(80);
        let window = open_question(
            std::ptr::null_mut(),
            &Question {
                text: &long,
                ..question
            },
        )
        .unwrap();
        let mut outer = rect(0, 0, 0, 0);
        unsafe { GetWindowRect(window, &mut outer) };
        assert!(
            outer.bottom - outer.top < scale(MOST_TEXT + 200, 96) + 40,
            "long text scrolls"
        );
        unsafe { DestroyWindow(window) };
    }

    #[test]
    fn a_question_moved_to_a_monitor_at_another_scale_is_laid_out_again() {
        use crate::ui::theme::metrics::TITLE_STRIP;
        use windows_sys::Win32::Graphics::Gdi::MapWindowPoints;
        let question = Question {
            title: "Discard this note?",
            text: "The note goes to the Recycle Bin.",
            buttons: &["Discard", "Keep"],
            default: 1,
            cancel: 1,
        };
        let window = open_question(std::ptr::null_mut(), &question).unwrap();
        let suggested = rect(40, 30, 40 + scale(WIDTH, 144), 30 + 300);
        unsafe {
            SendMessageW(
                window,
                WM_DPICHANGED,
                (144 << 16) | 144,
                &suggested as *const RECT as isize,
            )
        };
        let state = question_state(window).unwrap();
        assert_eq!(state.frame.height(), scale(TITLE_STRIP, 144));
        let mut keep = rect(0, 0, 0, 0);
        unsafe {
            GetWindowRect(state.buttons[1], &mut keep);
            MapWindowPoints(std::ptr::null_mut(), window, &mut keep as *mut RECT as _, 2);
        }
        let pad = scale(DIALOG_PADDING, 144);
        assert_eq!(keep.right, scale(WIDTH, 144) - pad);
        assert_eq!(keep.right - keep.left, scale(96, 144));
        unsafe { DestroyWindow(window) };
    }
}
