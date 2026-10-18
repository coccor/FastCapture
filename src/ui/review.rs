//! The review window: queued captures on the left, newest first, with
//! the selected raw source read-only on the right. In M1 there are no written notes yet, so the
//! keys are: **E** (or a double-click) opens the raw source in FastPad or the default editor,
//! **Del** sends it and the attachments only it links to to the Recycle Bin, **Enter** opens
//! the notebook's raw folder, **F5** refreshes, **Esc** closes. A standard ListView and a
//! read-only edit give keyboard and screen-reader support from Windows.

use super::review_list::{self, Row};
use super::theme::{self, Palette, scale};
use crate::capture::read::{self, RawInfo};
use crate::config::settings::SettingsFile;
use crate::queue::Store;
use crate::queue::store::raw_sources;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    DeleteObject, DrawTextW, EndPaint, FillRect, HBRUSH, HDC, HFONT, InvalidateRect, PAINTSTRUCT,
    SelectObject, SetBkColor, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    ICC_LISTVIEW_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, LVCF_TEXT, LVCF_WIDTH,
    LVCOLUMNW, LVIF_PARAM, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW, LVM_DELETEALLITEMS,
    LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETBKCOLOR, LVM_SETCOLUMNWIDTH,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMSTATE, LVM_SETITEMTEXTW, LVM_SETTEXTBKCOLOR,
    LVM_SETTEXTCOLOR, LVN_ITEMCHANGED, LVN_KEYDOWN, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER,
    LVS_EX_FULLROWSELECT, LVS_REPORT, LVS_SHOWSELALWAYS, LVS_SINGLESEL, NM_DBLCLK, NMHDR,
    NMLISTVIEW, NMLVKEYDOWN, SetWindowTheme,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_DELETE, VK_F5};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, ES_AUTOVSCROLL, ES_MULTILINE,
    ES_READONLY, FindWindowW, GWLP_USERDATA, GetClientRect, GetWindowLongPtrW, GetWindowRect,
    IDC_ARROW, IDCANCEL, IDOK, IsDialogMessageW, IsIconic, LoadCursorW, MSG, RegisterClassW,
    SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, WM_APP, WM_CLOSE, WM_COMMAND,
    WM_CTLCOLORSTATIC, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_NCDESTROY, WM_NOTIFY, WM_PAINT,
    WM_SETFONT, WM_SIZE, WNDCLASSW, WS_CHILD, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_TABSTOP,
    WS_VISIBLE, WS_VSCROLL,
};

/// Sent to the window when the queue changed elsewhere: it reloads.
pub const WM_REVIEW_REFRESH: u32 = WM_APP + 0x70;

const PAD_AT_96_DPI: i32 = 10;
const FOOTER_AT_96_DPI: i32 = 30;
const DEFAULT_WIDTH: i32 = 900;
const DEFAULT_HEIGHT: i32 = 560;
/// Each column's share of the list's width, in percent: Capture, Kind, Time, State.
const COLUMN_SHARES: [i32; 4] = [41, 22, 17, 20];
const HINT: &str =
    "E open \u{00B7} Del recycle \u{00B7} Enter open folder \u{00B7} F5 refresh \u{00B7} Esc close";
const EMPTY: &str = "Nothing waiting. Captures appear here until they are turned into notes.";

/// What the window does to the world, so tests can watch instead.
pub trait Actions {
    /// Sends files to the Recycle Bin.
    fn recycle(&mut self, paths: &[PathBuf]) -> Result<(), String>;
    /// Opens a raw source in an editor.
    fn open_raw(&mut self, path: &Path);
    /// Opens a folder in Explorer.
    fn open_folder(&mut self, folder: &Path);
    /// The queue changed here (a capture was recycled).
    fn changed(&mut self) {}
}

/// The real actions: the Recycle Bin, FastPad or the default editor, Explorer.
pub struct ShellActions {
    pub on_change: Box<dyn FnMut()>,
}

impl Actions for ShellActions {
    fn recycle(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        crate::platform::shell::recycle(paths).map_err(|error| error.to_string())
    }

    fn open_raw(&mut self, path: &Path) {
        if let Err(error) = crate::platform::shell::open_in_editor(path) {
            crate::log::line(&format!("review: open: {error}"));
        }
    }

    fn open_folder(&mut self, folder: &Path) {
        if let Err(error) = crate::platform::shell::open(folder) {
            crate::log::line(&format!("review: open folder: {error}"));
        }
    }

    fn changed(&mut self) {
        (self.on_change)();
    }
}

pub struct Open {
    pub queue: PathBuf,
    /// `fastcapture.ini`, where the window's size and place are kept.
    pub settings: Option<PathBuf>,
    /// Opened by Enter when nothing is selected.
    pub notebook: Option<PathBuf>,
    pub actions: Box<dyn Actions>,
}

struct Review {
    queue: PathBuf,
    settings: Option<PathBuf>,
    notebook: Option<PathBuf>,
    actions: Box<dyn Actions>,
    colors: Palette,
    dpi: u32,
    list: HWND,
    preview: HWND,
    font: HFONT,
    surface_brush: HBRUSH,
    rows: Vec<Row>,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn class_name() -> &'static [u16] {
    static NAME: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| wide("FastCaptureReview"))
}

fn register_class() -> bool {
    static REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *REGISTERED.get_or_init(|| {
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let class = WNDCLASSW {
            lpfnWndProc: Some(review_proc),
            hInstance: instance,
            hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
            // The executable's icon (resource 1), drawn by `crate::icon`.
            hIcon: unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::LoadIconW(instance, 1 as _)
            },
            lpszClassName: class_name().as_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        unsafe { RegisterClassW(&class) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS }
    })
}

/// The open review window, if any.
pub fn find() -> Option<HWND> {
    let hwnd = unsafe { FindWindowW(class_name().as_ptr(), std::ptr::null()) };
    (!hwnd.is_null()).then_some(hwnd)
}

/// Lets the review window handle Tab, Enter and Esc for its controls. Call from the message
/// loop; `true` means the message was handled.
pub fn dialog_message(message: &MSG) -> bool {
    match find() {
        Some(hwnd) => unsafe { IsDialogMessageW(hwnd, message) != 0 },
        None => false,
    }
}

fn state(hwnd: HWND) -> Option<&'static mut Review> {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Review;
    (!pointer.is_null()).then(|| unsafe { &mut *pointer })
}

/// The window's last place, from `fastcapture.ini`, when all four numbers are there.
fn saved_place(settings: Option<&Path>) -> Option<RECT> {
    let file = SettingsFile::load(settings?);
    let number = |key: &str| file.value(key)?.parse::<i32>().ok();
    let (x, y) = (number("review_x")?, number("review_y")?);
    let (width, height) = (number("review_width")?, number("review_height")?);
    (width >= 300 && height >= 200).then_some(RECT {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    })
}

fn save_place(hwnd: HWND, settings: Option<&Path>) {
    let Some(path) = settings else {
        return;
    };
    if unsafe { IsIconic(hwnd) } != 0 {
        return;
    }
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(hwnd, &mut rect) };
    let mut file = SettingsFile::load(path);
    for (key, value) in [
        ("review_x", rect.left),
        ("review_y", rect.top),
        ("review_width", rect.right - rect.left),
        ("review_height", rect.bottom - rect.top),
    ] {
        file.set(key, &value.to_string());
    }
    if let Err(error) = file.save(path) {
        crate::log::line(&format!("review: {error}"));
    }
}

/// Opens the review window, or brings the open one forward and refreshes it.
pub fn open(options: Open) -> Option<HWND> {
    if let Some(hwnd) = find() {
        reload(hwnd);
        unsafe {
            ShowWindow(hwnd, SW_SHOWNORMAL);
            SetForegroundWindow(hwnd);
        }
        return Some(hwnd);
    }
    if !register_class() {
        return None;
    }
    let controls = INITCOMMONCONTROLSEX {
        dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_LISTVIEW_CLASSES,
    };
    unsafe { InitCommonControlsEx(&controls) };
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let place = saved_place(options.settings.as_deref());
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name().as_ptr(),
            wide("FastCapture \u{2014} Review").as_ptr(),
            WS_OVERLAPPEDWINDOW,
            place.map_or(CW_USEDEFAULT, |rect| rect.left),
            place.map_or(CW_USEDEFAULT, |rect| rect.top),
            place.map_or(CW_USEDEFAULT, |rect| rect.right - rect.left),
            place.map_or(CW_USEDEFAULT, |rect| rect.bottom - rect.top),
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
    if place.is_none() {
        unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                scale(DEFAULT_WIDTH, dpi),
                scale(DEFAULT_HEIGHT, dpi),
                windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                    | SWP_NOZORDER
                    | SWP_NOACTIVATE,
            )
        };
    }
    let colors = theme::current();
    let font = theme::font(10, false, dpi);
    let list = unsafe {
        CreateWindowExW(
            WS_EX_CLIENTEDGE,
            wide("SysListView32").as_ptr(),
            wide("Captures").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS,
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
    let preview = unsafe {
        CreateWindowExW(
            WS_EX_CLIENTEDGE,
            wide("EDIT").as_ptr(),
            std::ptr::null(),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WS_VSCROLL
                | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32,
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
    unsafe {
        SendMessageW(list, WM_SETFONT, font as usize, 0);
        SendMessageW(preview, WM_SETFONT, font as usize, 0);
        let extended = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
        SendMessageW(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            extended as usize,
            extended as isize,
        );
        SendMessageW(list, LVM_SETBKCOLOR, 0, colors.surface as isize);
        SendMessageW(list, LVM_SETTEXTBKCOLOR, 0, colors.surface as isize);
        SendMessageW(list, LVM_SETTEXTCOLOR, 0, colors.text as isize);
        if colors.dark {
            let theme = wide("DarkMode_Explorer");
            SetWindowTheme(list, theme.as_ptr(), std::ptr::null());
            SetWindowTheme(preview, theme.as_ptr(), std::ptr::null());
            let dark: i32 = 1;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                (&raw const dark).cast(),
                size_of_val(&dark) as u32,
            );
        }
    }
    for (index, title) in ["Capture", "Kind", "Time", "State"].iter().enumerate() {
        let text = wide(title);
        let column = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH,
            cx: scale([240, 110, 90, 70][index], dpi),
            pszText: text.as_ptr() as *mut u16,
            ..unsafe { std::mem::zeroed() }
        };
        unsafe { SendMessageW(list, LVM_INSERTCOLUMNW, index, (&raw const column) as isize) };
    }
    let review = Box::new(Review {
        queue: options.queue,
        settings: options.settings,
        notebook: options.notebook,
        actions: options.actions,
        colors,
        dpi,
        list,
        preview,
        font,
        surface_brush: unsafe { CreateSolidBrush(colors.surface) },
        rows: Vec::new(),
    });
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(review) as isize) };
    lay_out(hwnd);
    reload(hwnd);
    unsafe {
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
        SetFocus(list);
    }
    Some(hwnd)
}

fn lay_out(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let mut client: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut client) };
    let pad = scale(PAD_AT_96_DPI, review.dpi);
    let footer = scale(FOOTER_AT_96_DPI, review.dpi);
    let height = (client.bottom - footer - pad).max(0);
    let list_width = ((client.right - 3 * pad) * 45 / 100).max(0);
    unsafe {
        SetWindowPos(
            review.list,
            std::ptr::null_mut(),
            pad,
            pad,
            list_width,
            height - pad,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        SetWindowPos(
            review.preview,
            std::ptr::null_mut(),
            2 * pad + list_width,
            pad,
            (client.right - 3 * pad - list_width).max(0),
            height - pad,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        // The columns share the list's width, less room for a vertical scroll bar, so no
        // horizontal one ever shows.
        let mut inside: RECT = std::mem::zeroed();
        GetClientRect(review.list, &mut inside);
        let room = (inside.right - scale(20, review.dpi)).max(0);
        for (column, share) in COLUMN_SHARES.iter().enumerate() {
            SendMessageW(
                review.list,
                LVM_SETCOLUMNWIDTH,
                column,
                (room * share / 100) as isize,
            );
        }
        InvalidateRect(hwnd, std::ptr::null(), 1);
    }
}

/// Reads the queue again and fills the list, keeping the selected capture when it is still
/// there. Jobs whose raw source is gone are removed: there is nothing left to process.
fn reload(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let selected = selected_row(review).map(|row| row.capture_id.clone());
    let store = Store::new(review.queue.clone());
    let (rows, gone) = review_list::rows(store.list(), |path| std::fs::read_to_string(path).ok());
    for job in gone {
        crate::log::line(&format!(
            "review: {} is gone; its job is removed",
            job.raw.display()
        ));
        store.remove(&job.capture_id);
    }
    let today = crate::platform::clock::local_now().0;
    unsafe { SendMessageW(review.list, LVM_DELETEALLITEMS, 0, 0) };
    for (index, row) in rows.iter().enumerate() {
        let title = wide(&row.title);
        let item = LVITEMW {
            mask: LVIF_TEXT | LVIF_PARAM,
            iItem: index as i32,
            pszText: title.as_ptr() as *mut u16,
            lParam: index as isize,
            ..unsafe { std::mem::zeroed() }
        };
        unsafe { SendMessageW(review.list, LVM_INSERTITEMW, 0, (&raw const item) as isize) };
        let time = review_list::time_label(&row.created, today);
        for (column, text) in [
            (1, row.kinds.as_str()),
            (2, time.as_str()),
            (3, review_list::state_label(row.state)),
        ] {
            let text = wide(text);
            let sub = LVITEMW {
                iSubItem: column,
                pszText: text.as_ptr() as *mut u16,
                ..unsafe { std::mem::zeroed() }
            };
            unsafe {
                SendMessageW(
                    review.list,
                    LVM_SETITEMTEXTW,
                    index,
                    (&raw const sub) as isize,
                )
            };
        }
    }
    let count = rows.len();
    let keep = selected
        .and_then(|id| rows.iter().position(|row| row.capture_id == id))
        .unwrap_or(0);
    review.rows = rows;
    unsafe {
        SetWindowTextW(
            hwnd,
            wide(&format!("FastCapture \u{2014} Review ({count})")).as_ptr(),
        )
    };
    if count > 0 {
        select(review, keep);
    }
    show_preview(review);
}

fn select(review: &Review, index: usize) {
    let item = LVITEMW {
        stateMask: LVIS_SELECTED | LVIS_FOCUSED,
        state: LVIS_SELECTED | LVIS_FOCUSED,
        ..unsafe { std::mem::zeroed() }
    };
    unsafe {
        SendMessageW(
            review.list,
            LVM_SETITEMSTATE,
            index,
            (&raw const item) as isize,
        )
    };
}

fn selected_index(review: &Review) -> Option<usize> {
    let index = unsafe {
        SendMessageW(
            review.list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_SELECTED as isize,
        )
    };
    (index >= 0 && (index as usize) < review.rows.len()).then_some(index as usize)
}

fn selected_row(review: &Review) -> Option<&Row> {
    selected_index(review).and_then(|index| review.rows.get(index))
}

fn show_preview(review: &Review) {
    let text = match selected_row(review) {
        Some(row) => review_list::preview_text(row),
        None if review.rows.is_empty() => EMPTY.to_owned(),
        None => String::new(),
    };
    unsafe { SetWindowTextW(review.preview, wide(&text).as_ptr()) };
}

/// Del: the selected capture's raw source and the attachments no other raw source links to go
/// to the Recycle Bin, and its job is removed.
fn recycle_selected(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let Some(row) = selected_row(review).cloned() else {
        return;
    };
    let conventions = crate::notebook::Conventions::load(&row.notebook);
    let others: Vec<RawInfo> = raw_sources(&row.notebook, conventions.raw_folder())
        .into_iter()
        .filter(|found| found.capture_id != row.capture_id)
        .filter_map(|found| {
            let text = std::fs::read_to_string(row.notebook.join(&found.path)).ok()?;
            Some(read::read(&found.path, &text))
        })
        .collect();
    let mut paths = vec![row.raw_path()];
    paths.extend(
        read::own_attachments(&row.info, &others)
            .into_iter()
            .map(|path| row.notebook.join(path)),
    );
    match review.actions.recycle(&paths) {
        Ok(()) => {
            Store::new(review.queue.clone()).remove(&row.capture_id);
            review.actions.changed();
            reload(hwnd);
        }
        Err(error) => {
            unsafe {
                SetWindowTextW(
                    review.preview,
                    wide(&format!("Couldn't recycle this capture: {error}")).as_ptr(),
                )
            };
        }
    }
}

fn open_selected(hwnd: HWND) {
    if let Some(review) = state(hwnd)
        && let Some(path) = selected_row(review).map(Row::raw_path)
    {
        review.actions.open_raw(&path);
    }
}

/// Enter: the selected capture's raw folder, or the notebook's when nothing is selected.
fn open_folder(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let folder = match selected_row(review) {
        Some(row) => row.raw_path().parent().map(Path::to_path_buf),
        None => review.notebook.clone(),
    };
    if let Some(folder) = folder {
        review.actions.open_folder(&folder);
    }
}

fn notify(hwnd: HWND, header: &NMHDR, lparam: LPARAM) -> LRESULT {
    let Some(review) = state(hwnd) else {
        return 0;
    };
    if header.hwndFrom != review.list {
        return 0;
    }
    match header.code {
        LVN_ITEMCHANGED => {
            let change = unsafe { &*(lparam as *const NMLISTVIEW) };
            if change.uNewState & LVIS_SELECTED != 0 {
                show_preview(review);
            }
        }
        LVN_KEYDOWN => {
            let key = unsafe { &*(lparam as *const NMLVKEYDOWN) }.wVKey;
            match key {
                k if k == u16::from(b'E') => open_selected(hwnd),
                VK_DELETE => recycle_selected(hwnd),
                VK_F5 => reload(hwnd),
                _ => {}
            }
        }
        NM_DBLCLK => open_selected(hwnd),
        _ => {}
    }
    0
}

fn paint(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let mut ps: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    let dc: HDC = unsafe { BeginPaint(hwnd, &mut ps) };
    let mut client: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut client) };
    unsafe {
        let brush = CreateSolidBrush(review.colors.background);
        FillRect(dc, &client, brush);
        DeleteObject(brush);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, review.colors.muted);
    }
    let pad = scale(PAD_AT_96_DPI, review.dpi);
    let mut footer = RECT {
        left: pad,
        top: client.bottom - scale(FOOTER_AT_96_DPI, review.dpi),
        right: client.right - pad,
        bottom: client.bottom,
    };
    let text: Vec<u16> = HINT.encode_utf16().collect();
    unsafe {
        let old = SelectObject(dc, review.font);
        DrawTextW(
            dc,
            text.as_ptr(),
            text.len() as i32,
            &mut footer,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        SelectObject(dc, old);
        EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn review_proc(
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
        WM_SIZE => {
            lay_out(hwnd);
            0
        }
        WM_DPICHANGED => {
            let suggested = unsafe { &*(lparam as *const RECT) };
            if let Some(review) = state(hwnd) {
                review.dpi = u32::from((wparam & 0xFFFF) as u16).max(96);
            }
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
            0
        }
        WM_NOTIFY => {
            let header = unsafe { &*(lparam as *const NMHDR) };
            notify(hwnd, header, lparam)
        }
        WM_COMMAND => {
            match (wparam & 0xFFFF) as i32 {
                IDOK => open_folder(hwnd),
                IDCANCEL => unsafe {
                    DestroyWindow(hwnd);
                },
                _ => {}
            }
            0
        }
        WM_CTLCOLORSTATIC => {
            let Some(review) = state(hwnd) else {
                return 0;
            };
            let dc = wparam as HDC;
            unsafe {
                SetTextColor(dc, review.colors.text);
                SetBkColor(dc, review.colors.surface);
            }
            review.surface_brush as LRESULT
        }
        WM_REVIEW_REFRESH => {
            reload(hwnd);
            0
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            if let Some(review) = state(hwnd) {
                save_place(hwnd, review.settings.as_deref());
            }
            0
        }
        WM_NCDESTROY => {
            let pointer = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut Review;
            if !pointer.is_null() {
                let review = unsafe { Box::from_raw(pointer) };
                unsafe {
                    DeleteObject(review.font);
                    DeleteObject(review.surface_brush);
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::submit::submit;
    use crate::capture::{Attachment, Capture};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct Seen {
        recycled: Vec<PathBuf>,
        opened: Vec<PathBuf>,
        folders: Vec<PathBuf>,
        changes: usize,
    }

    /// Deletes instead of recycling, and records what was opened.
    struct Watch(Rc<RefCell<Seen>>);

    impl Actions for Watch {
        fn recycle(&mut self, paths: &[PathBuf]) -> Result<(), String> {
            for path in paths {
                std::fs::remove_file(path).map_err(|error| error.to_string())?;
            }
            self.0.borrow_mut().recycled.extend(paths.iter().cloned());
            Ok(())
        }
        fn open_raw(&mut self, path: &Path) {
            self.0.borrow_mut().opened.push(path.to_path_buf());
        }
        fn open_folder(&mut self, folder: &Path) {
            self.0.borrow_mut().folders.push(folder.to_path_buf());
        }
        fn changed(&mut self) {
            self.0.borrow_mut().changes += 1;
        }
    }

    #[test]
    fn captures_are_listed_and_del_recycles_only_what_is_theirs() {
        let temp = std::env::temp_dir().join(format!("fastcapture-review-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let notebook = temp.join("Notes");
        let queue = temp.join("queue");
        std::fs::create_dir_all(&notebook).unwrap();
        let first = submit(
            &notebook,
            &Capture {
                text: "first".to_owned(),
                attachments: vec![Attachment::Png(b"png".to_vec())],
                source_window: None,
            },
            Some(&queue),
            false,
        )
        .unwrap();
        // Copies of one file in two captures are two attachments; make the second capture
        // link to the first one's image instead, as an edited raw source might.
        let second = submit(
            &notebook,
            &Capture {
                text: "second".to_owned(),
                ..Capture::default()
            },
            Some(&queue),
            false,
        )
        .unwrap();
        let image_link = std::fs::read_to_string(&first)
            .unwrap()
            .lines()
            .find(|line| line.starts_with("!["))
            .unwrap()
            .to_owned();
        let mut text = std::fs::read_to_string(&second).unwrap();
        text.push_str(&format!("\n{image_link}\n"));
        std::fs::write(&second, text).unwrap();

        let seen = Rc::new(RefCell::new(Seen::default()));
        let hwnd = open(Open {
            queue: queue.clone(),
            settings: None,
            notebook: Some(notebook.clone()),
            actions: Box::new(Watch(seen.clone())),
        })
        .unwrap();
        let review = state(hwnd).unwrap();
        assert_eq!(review.rows.len(), 2);
        let titles: Vec<&str> = review.rows.iter().map(|row| row.title.as_str()).collect();
        assert!(
            titles.contains(&"first") && titles.contains(&"second"),
            "{titles:?}"
        );

        // Select the first capture and recycle it: its image stays, the second links to it.
        let index = review
            .rows
            .iter()
            .position(|row| row.title == "first")
            .unwrap();
        select(review, index);
        recycle_selected(hwnd);
        assert_eq!(seen.borrow().recycled, std::slice::from_ref(&first));
        assert_eq!(seen.borrow().changes, 1);
        assert_eq!(state(hwnd).unwrap().rows.len(), 1);
        assert_eq!(Store::new(queue.clone()).list().len(), 1);

        // Now the second owns the image: recycling it takes both.
        select(state(hwnd).unwrap(), 0);
        open_selected(hwnd);
        assert_eq!(seen.borrow().opened, std::slice::from_ref(&second));
        open_folder(hwnd);
        assert_eq!(seen.borrow().folders, [notebook.join("raw")]);
        recycle_selected(hwnd);
        assert_eq!(seen.borrow().recycled.len(), 3);
        assert!(state(hwnd).unwrap().rows.is_empty());
        assert!(Store::new(queue).list().is_empty());

        unsafe { DestroyWindow(hwnd) };
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn the_window_keeps_its_place_in_the_settings() {
        let temp =
            std::env::temp_dir().join(format!("fastcapture-review-place-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let settings = temp.join("fastcapture.ini");
        let hwnd = open(Open {
            queue: temp.join("queue"),
            settings: Some(settings.clone()),
            notebook: None,
            actions: Box::new(Watch(Rc::default())),
        })
        .unwrap();
        unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                40,
                50,
                700,
                420,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            DestroyWindow(hwnd);
        }
        assert_eq!(
            saved_place(Some(&settings)).map(|rect| (rect.left, rect.top, rect.right, rect.bottom)),
            Some((40, 50, 740, 470))
        );
        let _ = std::fs::remove_dir_all(&temp);
    }
}
