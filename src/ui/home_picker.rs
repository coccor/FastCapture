//! The home picker (H in the review window): a box to type in above the homes that match, the
//! suggested one first. **Enter** files the note in the selected home (the first match when none
//! is selected), **Up** and **Down** move through the list from the box, **Esc** closes. A
//! standard edit and list box give keyboard and screen-reader support from Windows; they are
//! painted in the theme, under FastPad's title strip, and the list's rows are FastPad's.

use super::controls::{self, Look, Surface};
use super::frame::{self, Buttons, Frame};
use super::paint;
use super::theme::metrics::ROW;
use super::theme::ramp::{self, TextStyle, scale_text};
use super::theme::{self, scale};
use crate::file::Target;
use crate::file::home::{self, HomeOption};
use crate::platform::window::SendHwnd;
use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{DeleteObject, HFONT, InvalidateRect};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_DOWN, VK_UP};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, ES_AUTOHSCROLL, FindWindowW, GWLP_USERDATA,
    GetClientRect, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    IDC_ARROW, IDCANCEL, IDOK, IsDialogMessageW, LB_ADDSTRING, LB_ERR, LB_GETCURSEL,
    LB_RESETCONTENT, LB_SETCURSEL, LB_SETITEMHEIGHT, LBN_DBLCLK, LBS_HASSTRINGS,
    LBS_NOINTEGRALHEIGHT, LBS_NOTIFY, LBS_OWNERDRAWFIXED, LoadCursorW, MSG, RegisterClassW,
    SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, WM_CLOSE, WM_COMMAND, WM_DPICHANGED, WM_DRAWITEM, WM_ERASEBKGND, WM_KEYDOWN,
    WM_NCDESTROY, WM_PAINT, WM_SETFONT, WM_SIZE, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN,
    WS_EX_CLIENTEDGE, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_THICKFRAME, WS_VISIBLE, WS_VSCROLL,
};

const EDIT_ID: usize = 100;
const LIST_ID: usize = 101;
const EN_CHANGE: u32 = 0x0300;
const WIDTH_AT_96_DPI: i32 = 420;
const HEIGHT_AT_96_DPI: i32 = 360;
const PAD_AT_96_DPI: i32 = 12;
const EDIT_AT_96_DPI: i32 = 26;

/// What the picker does with the home chosen.
pub type OnPick = Box<dyn FnOnce(Target)>;

struct Picker {
    options: Vec<HomeOption>,
    /// The options the list shows, by index into `options`.
    shown: Vec<usize>,
    edit: HWND,
    list: HWND,
    font: HFONT,
    dpi: u32,
    on_pick: Option<OnPick>,
    frame: Frame,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn class_name() -> &'static [u16] {
    static NAME: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| wide("FastCaptureHomePicker"))
}

fn register_class() -> bool {
    static REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *REGISTERED.get_or_init(|| {
        let class = WNDCLASSW {
            lpfnWndProc: Some(picker_proc),
            hInstance: unsafe { GetModuleHandleW(std::ptr::null()) },
            hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
            lpszClassName: class_name().as_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        unsafe { RegisterClassW(&class) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS }
    })
}

/// The open picker, if any.
pub fn find() -> Option<HWND> {
    let hwnd = unsafe { FindWindowW(class_name().as_ptr(), std::ptr::null()) };
    (!hwnd.is_null()).then_some(hwnd)
}

fn state(hwnd: HWND) -> Option<&'static mut Picker> {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Picker;
    (!pointer.is_null()).then(|| unsafe { &mut *pointer })
}

/// Lets the picker handle Tab, Enter, Esc, and Up and Down from its box. Call from the message
/// loop; `true` means the message was handled.
pub fn dialog_message(message: &MSG) -> bool {
    let Some(hwnd) = find() else {
        return false;
    };
    if let Some(picker) = state(hwnd)
        && message.hwnd == picker.edit
        && message.message == WM_KEYDOWN
        && (message.wParam == usize::from(VK_DOWN) || message.wParam == usize::from(VK_UP))
    {
        move_selection(picker, message.wParam == usize::from(VK_DOWN));
        return true;
    }
    unsafe { IsDialogMessageW(hwnd, message) != 0 }
}

/// Opens the picker over `owner` with `options`; `on_pick` gets the home chosen. An open picker
/// is closed first.
pub fn open(owner: SendHwnd, options: Vec<HomeOption>, on_pick: OnPick) -> Option<HWND> {
    let owner = owner.get();
    if let Some(open) = find() {
        unsafe { DestroyWindow(open) };
    }
    if !register_class() {
        return None;
    }
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let dpi = unsafe { GetDpiForWindow(owner) }.max(96);
    let mut place: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(owner, &mut place) };
    let (width, height) = (scale(WIDTH_AT_96_DPI, dpi), scale(HEIGHT_AT_96_DPI, dpi));
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name().as_ptr(),
            wide("Choose a home").as_ptr(),
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_CLIPCHILDREN,
            place.left + ((place.right - place.left) - width) / 2,
            place.top + ((place.bottom - place.top) - height) / 3,
            width,
            height,
            owner,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        return None;
    }
    let child = |class: &str, style: u32, id: usize| unsafe {
        CreateWindowExW(
            WS_EX_CLIENTEDGE,
            wide(class).as_ptr(),
            std::ptr::null(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | style,
            0,
            0,
            0,
            0,
            hwnd,
            id as _,
            instance,
            std::ptr::null(),
        )
    };
    let edit = child("EDIT", ES_AUTOHSCROLL as u32, EDIT_ID);
    let list = child(
        "LISTBOX",
        WS_VSCROLL
            | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | LBS_OWNERDRAWFIXED | LBS_HASSTRINGS) as u32,
        LIST_ID,
    );
    let font = ramp::font(TextStyle::Body, dpi);
    unsafe {
        SendMessageW(edit, WM_SETFONT, font as usize, 0);
        SendMessageW(list, WM_SETFONT, font as usize, 0);
        SendMessageW(list, LB_SETITEMHEIGHT, 0, scale_text(ROW, dpi) as isize);
    }
    controls::paint_as(edit, Look::Field, Surface::Content);
    controls::paint_as(list, Look::Field, Surface::Content);
    let shown = (0..options.len()).collect();
    let picker = Box::new(Picker {
        options,
        shown,
        edit,
        list,
        font,
        dpi,
        on_pick: Some(on_pick),
        frame: Frame::new("Choose a home", Buttons::default(), true, dpi),
    });
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(picker) as isize) };
    frame::attach(hwnd, &theme::current());
    lay_out(hwnd);
    if let Some(picker) = state(hwnd) {
        fill(picker);
    }
    unsafe {
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetFocus(edit);
    }
    Some(hwnd)
}

fn lay_out(hwnd: HWND) {
    let Some(picker) = state(hwnd) else {
        return;
    };
    let mut client: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut client) };
    let pad = scale(PAD_AT_96_DPI, picker.dpi);
    let edit = scale(EDIT_AT_96_DPI, picker.dpi);
    let width = (client.right - 2 * pad).max(0);
    let top = picker.frame.height() + scale(4, picker.dpi);
    unsafe {
        SetWindowPos(
            picker.edit,
            std::ptr::null_mut(),
            pad,
            top,
            width,
            edit,
            SWP_NOZORDER,
        );
        SetWindowPos(
            picker.list,
            std::ptr::null_mut(),
            pad,
            top + pad + edit,
            width,
            (client.bottom - top - 2 * pad - edit).max(0),
            SWP_NOZORDER,
        );
    }
}

/// Follows the picker to a monitor with another scale: the size Windows suggests, the fonts,
/// the strip and the rows at the new DPI.
fn change_dpi(hwnd: HWND, dpi: u32, suggested: &RECT) {
    let Some(picker) = state(hwnd) else {
        return;
    };
    picker.dpi = dpi;
    picker.frame.set_dpi(dpi);
    let font = ramp::font(TextStyle::Body, dpi);
    unsafe {
        SendMessageW(picker.edit, WM_SETFONT, font as usize, 0);
        SendMessageW(picker.list, WM_SETFONT, font as usize, 0);
        SendMessageW(
            picker.list,
            LB_SETITEMHEIGHT,
            0,
            scale_text(ROW, dpi) as isize,
        );
        DeleteObject(std::mem::replace(&mut picker.font, font));
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            suggested.left,
            suggested.top,
            suggested.right - suggested.left,
            suggested.bottom - suggested.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    lay_out(hwnd);
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 1) };
}

/// Shows the options matching what is typed, the first selected.
fn fill(picker: &mut Picker) {
    let length = unsafe { GetWindowTextLengthW(picker.edit) }.max(0) as usize;
    let mut buffer = vec![0u16; length + 1];
    let read = unsafe { GetWindowTextW(picker.edit, buffer.as_mut_ptr(), buffer.len() as i32) };
    let typed = String::from_utf16_lossy(&buffer[..read.max(0) as usize]);
    picker.shown = home::filter(&picker.options, &typed);
    unsafe { SendMessageW(picker.list, LB_RESETCONTENT, 0, 0) };
    for &index in &picker.shown {
        let option = &picker.options[index];
        let line = wide(&format!("{}   \u{00B7} {}", option.label, option.detail));
        unsafe { SendMessageW(picker.list, LB_ADDSTRING, 0, line.as_ptr() as isize) };
    }
    if !picker.shown.is_empty() {
        unsafe { SendMessageW(picker.list, LB_SETCURSEL, 0, 0) };
    }
}

fn move_selection(picker: &Picker, down: bool) {
    if picker.shown.is_empty() {
        return;
    }
    let current = unsafe { SendMessageW(picker.list, LB_GETCURSEL, 0, 0) };
    let last = picker.shown.len() as isize - 1;
    let next = match (current, down) {
        (LB_ERR_ISIZE, _) => 0,
        (at, true) => (at + 1).min(last),
        (at, false) => (at - 1).max(0),
    };
    unsafe { SendMessageW(picker.list, LB_SETCURSEL, next as usize, 0) };
}

const LB_ERR_ISIZE: isize = LB_ERR as isize;

/// Types `text` in the open picker's box, as you would.
#[cfg(test)]
pub(crate) fn type_text(text: &str) {
    if let Some(hwnd) = find()
        && let Some(picker) = state(hwnd)
    {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                picker.edit,
                wide(text).as_ptr(),
            )
        };
    }
}

/// Files to the selected home, or the first match, and closes.
fn pick(hwnd: HWND) {
    let Some(picker) = state(hwnd) else {
        return;
    };
    let current = unsafe { SendMessageW(picker.list, LB_GETCURSEL, 0, 0) };
    let at = if current == LB_ERR_ISIZE {
        0
    } else {
        current as usize
    };
    let Some(&index) = picker.shown.get(at) else {
        return;
    };
    let target = picker.options[index].target.clone();
    let on_pick = picker.on_pick.take();
    unsafe { DestroyWindow(hwnd) };
    if let Some(on_pick) = on_pick {
        on_pick(target);
    }
}

unsafe extern "system" fn picker_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } == 0 {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    if let Some(picker) = state(hwnd)
        && let Some(answer) = picker.frame.handle(hwnd, message, wparam, lparam)
    {
        return answer;
    }
    if let Some(answer) = controls::colors(message, wparam, lparam, Surface::Content) {
        return answer;
    }
    match message {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let palette = theme::current();
            if let Some(picker) = state(hwnd) {
                paint::buffered(hwnd, |dc, area| {
                    paint::fill(dc, area, palette.content_background);
                    picker.frame.paint(dc, area.right, &palette, false);
                });
            }
            0
        }
        WM_DRAWITEM => {
            let item = unsafe { &*(lparam as *const DRAWITEMSTRUCT) };
            controls::draw_list_item(item);
            1
        }
        _ if message == theme::changed_message() => {
            frame::apply_theme(hwnd, &theme::current());
            controls::retheme(hwnd);
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            0
        }
        WM_SIZE => {
            lay_out(hwnd);
            0
        }
        WM_DPICHANGED => {
            let dpi = u32::from((wparam & 0xFFFF) as u16).max(96);
            change_dpi(hwnd, dpi, unsafe { &*(lparam as *const RECT) });
            0
        }
        WM_COMMAND => {
            let id = wparam & 0xFFFF;
            let code = (wparam >> 16) as u32;
            match (id, code) {
                (EDIT_ID, EN_CHANGE) => {
                    if let Some(picker) = state(hwnd) {
                        fill(picker);
                    }
                }
                (LIST_ID, LBN_DBLCLK) => pick(hwnd),
                (id, _) if id == IDOK as usize => pick(hwnd),
                (id, _) if id == IDCANCEL as usize => unsafe {
                    DestroyWindow(hwnd);
                },
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_NCDESTROY => {
            let pointer = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut Picker;
            if !pointer.is_null() {
                let picker = unsafe { Box::from_raw(pointer) };
                unsafe { DeleteObject(picker.font) };
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, SetWindowTextW, WS_OVERLAPPEDWINDOW,
    };

    fn option(label: &str, target: Target) -> HomeOption {
        HomeOption {
            label: label.to_owned(),
            detail: "folder".to_owned(),
            target,
        }
    }

    #[test]
    fn typing_narrows_the_list_and_enter_picks_the_selection() {
        let owner = unsafe {
            CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                std::ptr::null(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                600,
                400,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        let picked = Rc::new(RefCell::new(None));
        let seen = picked.clone();
        let hwnd = open(
            SendHwnd::new(owner),
            vec![
                option("Payments/", Target::Folder(PathBuf::from("Payments"))),
                option(
                    "Learning/Rust/",
                    Target::Folder(PathBuf::from(r"Learning\Rust")),
                ),
                option("Learning/", Target::Folder(PathBuf::from("Learning"))),
            ],
            Box::new(move |target| *seen.borrow_mut() = Some(target)),
        )
        .unwrap();
        let picker = state(hwnd).unwrap();
        assert_eq!(picker.shown, [0, 1, 2]);
        // Typing changes the box, which narrows the list.
        unsafe { SetWindowTextW(picker.edit, wide("learn").as_ptr()) };
        assert_eq!(state(hwnd).unwrap().shown, [1, 2]);
        move_selection(state(hwnd).unwrap(), true);
        unsafe { SendMessageW(hwnd, WM_COMMAND, IDOK as usize, 0) };
        assert_eq!(
            *picked.borrow(),
            Some(Target::Folder(PathBuf::from("Learning")))
        );
        assert!(find().is_none(), "the picker closes");

        // Esc closes without a pick.
        *picked.borrow_mut() = None;
        let seen = picked.clone();
        let hwnd = open(
            SendHwnd::new(owner),
            vec![option(
                "Payments/",
                Target::Folder(PathBuf::from("Payments")),
            )],
            Box::new(move |target| *seen.borrow_mut() = Some(target)),
        )
        .unwrap();
        unsafe { SendMessageW(hwnd, WM_COMMAND, IDCANCEL as usize, 0) };
        assert!(picked.borrow().is_none());
        assert!(find().is_none());
        unsafe { DestroyWindow(owner) };
    }
}
