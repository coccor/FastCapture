//! The review window: the notes waiting in the inbox and the captures still being written on
//! the left, newest first, each with its suggested home; the selected note above its raw
//! source, read-only, on the right. **Enter** files the note in its suggested home (or opens the
//! folder when it has none); **H** picks another home; **I** keeps it in the inbox; **Ctrl+Z**
//! undoes the last file, keep or discard; **E** (or a double-click) opens the note, or the raw
//! source when there is none, in FastPad or the default editor; **R** writes the note again with
//! the current settings, or places an agent's finding again; **Del** sends the note to the Recycle Bin and keeps the raw source;
//! **Shift+Del** sends the note, the raw source and the attachments only it links to; **F5**
//! refreshes; **Esc** closes. A standard ListView and a read-only edit give keyboard and
//! screen-reader support from Windows.

use super::controls::{self, Look, Surface};
use super::frame::{self, Buttons, Frame};
use super::review_list::{self, Row};
use super::theme::ramp::{self, TextStyle};
use super::theme::{self, Palette, scale};
use crate::capture::read::{self, RawInfo};
use crate::config::settings::SettingsFile;
use crate::file::undo::{self, Undo};
use crate::file::{self as filing, Target, home};
use crate::index::service::{Indexes, Snapshot};
use crate::notebook::Conventions;
use crate::notebook::links::{Resolver, wiki_target};
use crate::platform::window::SendHwnd;
use crate::queue::store::raw_sources;
use crate::queue::{Job, State, Store};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM,
};

use windows_sys::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteObject, HBRUSH, HFONT, InvalidateRect,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    ICC_LISTVIEW_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, LVCF_TEXT, LVCF_WIDTH,
    LVCOLUMNW, LVIF_PARAM, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW, LVM_DELETEALLITEMS,
    LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETCOLUMNWIDTH,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMSTATE, LVM_SETITEMTEXTW, LVN_ITEMCHANGED, LVN_KEYDOWN,
    LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_REPORT, LVS_SHOWSELALWAYS,
    LVS_SINGLESEL, NM_DBLCLK, NMHDR, NMLISTVIEW, NMLVKEYDOWN,
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
    WM_SETFONT, WM_SIZE, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_EX_CLIENTEDGE,
    WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

/// Sent to the window when the queue changed elsewhere: it reloads.
pub const WM_REVIEW_REFRESH: u32 = WM_APP + 0x70;

const PAD_AT_96_DPI: i32 = 10;
const FOOTER_AT_96_DPI: i32 = 30;
const DEFAULT_WIDTH: i32 = 900;
const DEFAULT_HEIGHT: i32 = 560;
/// Each column's share of the list's width, in percent: Capture, Home, Time, State.
const COLUMN_SHARES: [i32; 4] = [41, 26, 15, 18];
/// How long filing waits for the notebook's index, which names topic notes.
const INDEX_WAIT: Duration = Duration::from_secs(3);
const EMPTY: &str = "Nothing here yet. Captures and the notes written for them appear here.";

/// What the window does to the world, so tests can watch instead.
pub trait Actions {
    /// Sends files to the Recycle Bin.
    fn recycle(&mut self, paths: &[PathBuf]) -> Result<(), String>;
    /// Opens a note or raw source in an editor.
    fn open_raw(&mut self, path: &Path);
    /// Opens a folder in Explorer.
    fn open_folder(&mut self, folder: &Path);
    /// Asks a yes-or-no question.
    fn confirm(&mut self, owner: SendHwnd, question: &str) -> bool;
    /// The queue changed here (a capture was recycled or queued again).
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

    fn confirm(&mut self, owner: SendHwnd, question: &str) -> bool {
        let question = super::popup::Question {
            title: "FastCapture",
            text: question,
            buttons: &["Yes", "No"],
            default: 1,
            cancel: 1,
        };
        super::popup::ask(owner.get(), &question) == 0
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
    /// The notebooks' indexes, for the homes and the topic notes' names.
    pub indexes: Option<Indexes>,
    pub actions: Box<dyn Actions>,
}

/// A file, keep or discard done in this window, newest last, for Ctrl+Z.
struct Done {
    /// The capture's job before.
    job: Job,
    undo: Undo,
}

struct Review {
    queue: PathBuf,
    settings: Option<PathBuf>,
    notebook: Option<PathBuf>,
    indexes: Option<Indexes>,
    actions: Box<dyn Actions>,
    done: Vec<Done>,
    colors: Palette,
    dpi: u32,
    list: HWND,
    preview: HWND,
    font: HFONT,
    surface_brush: HBRUSH,
    rows: Vec<Row>,
    frame: Frame,
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
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
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
    let font = ramp::font(TextStyle::Body, dpi);
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
    }
    // The list's rows and header, and both fields' borders, in the theme.
    controls::paint_list(list);
    controls::paint_as(list, Look::Field, Surface::Strip);
    controls::paint_as(preview, Look::Field, Surface::Strip);
    for (index, title) in ["Capture", "Home", "Time", "State"].iter().enumerate() {
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
        indexes: options.indexes,
        actions: options.actions,
        done: Vec::new(),
        colors,
        dpi,
        list,
        preview,
        font,
        surface_brush: unsafe { CreateSolidBrush(colors.content_background) },
        rows: Vec::new(),
        frame: Frame::new(
            "FastCapture \u{2014} Review",
            Buttons {
                minimize: true,
                maximize: true,
            },
            true,
            dpi,
        ),
    });
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(review) as isize) };
    frame::attach(hwnd, &colors);
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
    // Below the title strip.
    let top = review.frame.height() + scale(4, review.dpi);
    let height = (client.bottom - footer - top).max(0);
    let list_width = ((client.right - 3 * pad) * 45 / 100).max(0);
    unsafe {
        SetWindowPos(
            review.list,
            std::ptr::null_mut(),
            pad,
            top,
            list_width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        SetWindowPos(
            review.preview,
            std::ptr::null_mut(),
            2 * pad + list_width,
            top,
            (client.right - 3 * pad - list_width).max(0),
            height,
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
    let folders = std::cell::RefCell::new(HashMap::<PathBuf, PathBuf>::new());
    let (rows, gone) = review_list::rows(
        store.list(),
        |path| std::fs::read_to_string(path).ok(),
        |notebook| {
            folders
                .borrow_mut()
                .entry(notebook.to_path_buf())
                .or_insert_with(|| Conventions::load(notebook).capture_folder().to_path_buf())
                .clone()
        },
    );
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
        let home = row.home_label();
        let status = row.status();
        for (column, text) in [(1, home.as_str()), (2, time.as_str()), (3, status.as_str())] {
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
    review
        .frame
        .set_title(hwnd, &format!("FastCapture \u{2014} Review ({count})"));
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

fn show_problem(review: &Review, problem: &str) {
    unsafe { SetWindowTextW(review.preview, wide(problem).as_ptr()) };
}

/// The footer: the keys that apply to the selected row.
fn footer_text(review: &Review) -> String {
    let keys: Vec<String> = match selected_row(review) {
        Some(row) if row.note_text.is_some() => {
            let enter = match &row.home {
                Some(_) => format!("Enter file {}", row.home_label()),
                None => "Enter folder".to_owned(),
            };
            let again = if row.finding {
                "R place again"
            } else {
                "R regenerate"
            };
            vec![
                enter,
                "H home".to_owned(),
                "I keep".to_owned(),
                "E open".to_owned(),
                again.to_owned(),
                "Del discard".to_owned(),
                "Ctrl+Z undo".to_owned(),
                "Esc close".to_owned(),
            ]
        }
        Some(_) => [
            "E open",
            "R regenerate",
            "Del delete capture",
            "Enter folder",
            "Ctrl+Z undo",
            "Esc close",
        ]
        .map(str::to_owned)
        .to_vec(),
        None => ["Ctrl+Z undo", "F5 refresh", "Esc close"]
            .map(str::to_owned)
            .to_vec(),
    };
    keys.join(" \u{00B7} ")
}

/// Repaints the footer, after the selection changed.
fn refresh_footer(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let mut client: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut client) };
    let footer = RECT {
        top: client.bottom - scale(FOOTER_AT_96_DPI, review.dpi),
        ..client
    };
    unsafe { InvalidateRect(hwnd, &footer, 0) };
}

/// The notebook's index, waiting a little while it is built.
fn snapshot(review: &Review, notebook: &Path) -> Option<Arc<Snapshot>> {
    review.indexes.as_ref()?.snapshot(notebook, INDEX_WAIT)
}

/// The notebook's notes, for naming topic notes and the note moved under them.
fn resolver(snapshot: &Snapshot, without: &Path, with: Option<&Path>) -> Resolver {
    let notes = &snapshot.searcher.index().notes;
    let key = |path: &Path| path.to_string_lossy().to_lowercase();
    Resolver::new(
        notes
            .values()
            .map(|note| note.path.as_path())
            .filter(|path| key(path) != key(without))
            .chain(with),
        notes.values().flat_map(|note| {
            note.aliases
                .iter()
                .map(|alias| (alias.clone(), note.path.clone()))
        }),
    )
}

/// Where `row`'s suggested home is: a folder, or the topic note its link reaches.
fn suggested_target(review: &Review, row: &Row) -> Result<Option<Target>, String> {
    let Some(home) = &row.home else {
        return Ok(None);
    };
    let Some(name) = home
        .strip_prefix("[[")
        .and_then(|name| name.strip_suffix("]]"))
    else {
        let folder = home.trim().trim_matches('/').replace('/', "\\");
        return Ok((!folder.is_empty()).then(|| Target::Folder(PathBuf::from(folder))));
    };
    let note = row.note.as_ref().map(|note| note.path.as_path());
    let snapshot = snapshot(review, &row.notebook)
        .ok_or("The notebook's index isn't ready yet, so its topic notes can't be found.")?;
    resolver(&snapshot, Path::new(""), None)
        .resolve_wiki(name, note.unwrap_or(Path::new("note.md")))
        .map(|path| Some(Target::Topic(path)))
        .ok_or_else(|| format!("No note named [[{name}]] was found in the notebook."))
}

/// Puts `job` in the queue, saying so when it can't.
fn save_job(store: &Store, job: &Job) {
    if let Err(error) = store.put(job) {
        crate::log::line(&format!("review: {error}"));
    }
}

/// Files the note of the capture `capture_id` in `target`, and remembers how to undo it. A note
/// that can't be filed stays in the list with the reason.
fn file_to(hwnd: HWND, capture_id: &str, target: &Target) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let Some(row) = review
        .rows
        .iter()
        .find(|row| row.capture_id == capture_id)
        .cloned()
    else {
        return;
    };
    let Some(note) = row.note.clone().filter(|_| row.note_text.is_some()) else {
        return;
    };
    let today = crate::notebook::dates::format(crate::platform::clock::local_now().0, "YYYY-MM-DD");
    let snapshot = match target {
        Target::Topic(_) => snapshot(review, &row.notebook),
        Target::Folder(_) => None,
    };
    let link_name = |to: &Path| match (&snapshot, target) {
        (Some(snapshot), Target::Topic(topic)) => {
            wiki_target(&resolver(snapshot, &note.path, Some(to)), to, topic)
        }
        _ => to
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    };
    match filing::file_note(&row.notebook, &note.path, target, &today, &link_name) {
        Ok((written, undo)) => remember(review, &row.capture_id, written, undo),
        Err(error) => {
            show_problem(review, &format!("Couldn't file this note: {error}"));
            return;
        }
    }
    review.actions.changed();
    reload(hwnd);
}

/// Records the note's new place in its job, and the undo.
fn remember(
    review: &mut Review,
    capture_id: &str,
    written: crate::note::write::Written,
    undo: Undo,
) {
    let store = Store::new(review.queue.clone());
    if let Some(job) = store.get(capture_id) {
        let mut changed = job.clone();
        changed.note = Some(written);
        save_job(&store, &changed);
        review.done.push(Done { job, undo });
    }
}

/// Enter: files the note in its suggested home; without one, or for a capture with no note,
/// opens the folder.
fn enter(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let Some(row) = selected_row(review).cloned() else {
        open_folder(hwnd);
        return;
    };
    if row.note_text.is_none() {
        open_folder(hwnd);
        return;
    }
    match suggested_target(review, &row) {
        Ok(Some(target)) => file_to(hwnd, &row.capture_id, &target),
        Ok(None) => open_folder(hwnd),
        Err(problem) => show_problem(review, &problem),
    }
}

/// H: the home picker, the suggestion first; the note is filed where you pick.
fn pick_home(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let Some(row) = selected_row(review).cloned() else {
        return;
    };
    if row.note_text.is_none() {
        return;
    }
    let Some(snapshot) = snapshot(review, &row.notebook) else {
        show_problem(
            review,
            "The notebook's index isn't ready yet; try again in a moment.",
        );
        return;
    };
    let conventions = Conventions::load(&row.notebook);
    let suggested = suggested_target(review, &row).ok().flatten();
    let own = row
        .note
        .as_ref()
        .map(|note| note.path.to_string_lossy().to_lowercase());
    let options = home::options(
        snapshot
            .searcher
            .index()
            .notes
            .values()
            .filter(|note| own.as_deref() != Some(&*note.path.to_string_lossy().to_lowercase()))
            .map(|note| (note.path.as_path(), note.kind.as_deref())),
        &snapshot.folders,
        &conventions.capture.home_types,
        conventions.capture_folder(),
        suggested.as_ref(),
    );
    let owner = SendHwnd::new(hwnd);
    let capture_id = row.capture_id.clone();
    let opened = super::home_picker::open(
        SendHwnd::new(hwnd),
        options,
        Box::new(move |target| file_to(owner.get(), &capture_id, &target)),
    );
    if opened.is_none() {
        show_problem(review, "The home picker couldn't open.");
    }
}

/// I: the note stays in the inbox as `status: kept`, and leaves the list.
fn keep_selected(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let Some(row) = selected_row(review).cloned() else {
        return;
    };
    let Some(note) = row.note.clone().filter(|_| row.note_text.is_some()) else {
        return;
    };
    match filing::keep_note(&row.notebook, &note.path) {
        Ok((written, undo)) => remember(review, &row.capture_id, written, undo),
        Err(error) => {
            show_problem(review, &format!("Couldn't keep this note: {error}"));
            return;
        }
    }
    review.actions.changed();
    reload(hwnd);
}

/// Ctrl+Z: puts back the last file, keep or discard of this window, and selects it.
fn undo_last(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let Some(done) = review.done.pop() else {
        return;
    };
    match undo::undo(&done.job.notebook, &done.undo) {
        Ok(written) => {
            let mut job = done.job;
            job.note = Some(written);
            save_job(&Store::new(review.queue.clone()), &job);
            review.actions.changed();
            reload(hwnd);
            if let Some(review) = state(hwnd)
                && let Some(index) = review
                    .rows
                    .iter()
                    .position(|row| row.capture_id == job.capture_id)
            {
                select(review, index);
            }
        }
        Err(error) => show_problem(review, &format!("Couldn't undo: {error}")),
    }
}

/// Del: a written note goes to the Recycle Bin and its capture is marked discarded, so it isn't
/// written again (Ctrl+Z brings it back). For a capture without a note, Del is Shift+Del.
fn discard_selected(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let Some(row) = selected_row(review).cloned() else {
        return;
    };
    let Some(note) = row
        .note
        .clone()
        .filter(|_| row.state == State::Done && row.note_text.is_some())
    else {
        recycle_selected(hwnd);
        return;
    };
    let before = undo::before_discard(&row.notebook, &note.path);
    if let Err(error) = review.actions.recycle(&[row.notebook.join(&note.path)]) {
        show_problem(review, &format!("Couldn't recycle this note: {error}"));
        return;
    }
    let store = Store::new(review.queue.clone());
    if let Some(job) = store.get(&row.capture_id) {
        let mut discarded = job.clone();
        discarded.state = State::Discarded;
        // A finding's job keeps the note it recycled, so that very file, put back, isn't
        // taken for a new finding.
        if !discarded.finding {
            discarded.note = None;
        }
        discarded.regenerate = false;
        save_job(&store, &discarded);
        if let Ok(undo) = before {
            review.done.push(Done { job, undo });
        }
    }
    review.actions.changed();
    reload(hwnd);
}

/// R: the capture is queued again with the current settings; its note, when there is one, is
/// replaced. A note you edited is replaced only after you say so.
fn regenerate_selected(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let Some(row) = selected_row(review).cloned() else {
        return;
    };
    let store = Store::new(review.queue.clone());
    let Some(mut job) = store.get(&row.capture_id) else {
        return;
    };
    if job.state == State::Working {
        return;
    }
    // Placing a finding again changes only its home and links, so your edits are safe.
    if let Some(note) = job.note.as_mut().filter(|_| !job.finding) {
        let now = crate::note::write::Stamp::of(&row.notebook.join(&note.path));
        if let Some(now) = now
            && now != note.stamp
        {
            let question = "You edited this note since FastCapture wrote it. Write it again anyway? Your edits will be replaced.";
            if !review.actions.confirm(SendHwnd::new(hwnd), question) {
                return;
            }
            // You agreed: the note as it is now is what gets replaced.
            note.stamp = now;
        }
    }
    let Some(review) = state(hwnd) else {
        return;
    };
    job.regenerate = job.note.is_some();
    job.state = State::Queued;
    job.attempts = 0;
    job.retry_at = None;
    job.last_error = None;
    if let Err(error) = store.put(&job) {
        show_problem(
            review,
            &format!("Couldn't queue this capture again: {error}"),
        );
        return;
    }
    review.actions.changed();
    reload(hwnd);
}

/// Shift+Del: the capture's note, its raw source with its pages, and the attachments no other
/// raw source links to go to the Recycle Bin, and its job is removed. For a finding, only the
/// note.
fn recycle_selected(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let Some(row) = selected_row(review).cloned() else {
        return;
    };
    let paths = if row.finding {
        // A finding is its own source, with no pages or attachments of its own.
        row.note_path().into_iter().collect()
    } else {
        capture_files(&row)
    };
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

/// A capture's files: its note when it is there, its raw source with its pages, and the
/// attachments no other raw source links to.
fn capture_files(row: &Row) -> Vec<PathBuf> {
    let conventions = crate::notebook::Conventions::load(&row.notebook);
    let others: Vec<RawInfo> = raw_sources(&row.notebook, conventions.raw_folder())
        .into_iter()
        .filter(|found| found.capture_id != row.capture_id)
        .filter_map(|found| {
            let text = std::fs::read_to_string(row.notebook.join(&found.path)).ok()?;
            Some(read::read(&found.path, &text))
        })
        .collect();
    let mut paths: Vec<PathBuf> = row
        .note_path()
        .filter(|path| path.exists())
        .into_iter()
        .collect();
    paths.push(row.raw_path());
    paths.extend(
        crate::capture::sidecar::find(&row.notebook, &row.raw, &row.capture_id)
            .into_iter()
            .map(|(path, _)| row.notebook.join(path)),
    );
    paths.extend(
        read::own_attachments(&row.info, &others)
            .into_iter()
            .map(|path| row.notebook.join(path)),
    );
    paths
}

/// E: the note, or the raw source when there is no note.
fn open_selected(hwnd: HWND) {
    if let Some(review) = state(hwnd)
        && let Some(row) = selected_row(review)
    {
        let path = match (&row.note_text, row.note_path()) {
            (Some(_), Some(note)) => note,
            _ => row.raw_path(),
        };
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
                refresh_footer(hwnd);
            }
        }
        LVN_KEYDOWN => {
            let key = unsafe { &*(lparam as *const NMLVKEYDOWN) }.wVKey;
            let down = |key| unsafe {
                windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(i32::from(key)) < 0
            };
            let shift = down(windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT);
            let control = down(windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_CONTROL);
            match key {
                k if k == u16::from(b'Z') && control => undo_last(hwnd),
                k if k == u16::from(b'H') => pick_home(hwnd),
                k if k == u16::from(b'I') => keep_selected(hwnd),
                k if k == u16::from(b'E') => open_selected(hwnd),
                k if k == u16::from(b'R') => regenerate_selected(hwnd),
                VK_DELETE if shift => recycle_selected(hwnd),
                VK_DELETE => discard_selected(hwnd),
                VK_F5 => reload(hwnd),
                _ => {}
            }
        }
        NM_DBLCLK => open_selected(hwnd),
        _ => {}
    }
    0
}

/// The strip, the status line below the panes, and the frame's title strip, in the theme.
fn paint(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    let colors = review.colors;
    let maximized = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsZoomed(hwnd) } != 0;
    super::paint::buffered(hwnd, |dc, client| {
        super::paint::fill(dc, client, colors.strip_background);
        let pad = scale(PAD_AT_96_DPI, review.dpi);
        let footer = RECT {
            left: pad,
            top: client.bottom - scale(FOOTER_AT_96_DPI, review.dpi),
            right: client.right - pad,
            bottom: client.bottom,
        };
        super::paint::text(
            dc,
            &footer_text(review),
            footer,
            review.font,
            colors.muted_foreground,
            super::paint::Align::Left,
        );
        review.frame.paint(dc, client.right, &colors, maximized);
    });
}

/// Takes the theme now in force.
fn retheme(hwnd: HWND) {
    let Some(review) = state(hwnd) else {
        return;
    };
    review.colors = theme::current();
    unsafe {
        DeleteObject(review.surface_brush);
        review.surface_brush = CreateSolidBrush(review.colors.content_background);
    }
    frame::apply_theme(hwnd, &review.colors);
    controls::retheme(hwnd);
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
    if let Some(review) = state(hwnd)
        && let Some(answer) = review.frame.handle(hwnd, message, wparam, lparam)
    {
        return answer;
    }
    match message {
        _ if message == theme::changed_message() => {
            retheme(hwnd);
            0
        }
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
                review.frame.set_dpi(review.dpi);
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
            // The list's rows, painted in the theme; a failed capture's state in the error
            // color.
            let failed = |row: usize, column: usize| {
                let review = state(hwnd)?;
                (column == 3
                    && review
                        .rows
                        .get(row)
                        .is_some_and(|row| row.state == State::Failed))
                .then_some(review.colors.error_foreground)
            };
            if let Some(answer) = controls::list_rows(lparam, failed) {
                return answer;
            }
            let header = unsafe { &*(lparam as *const NMHDR) };
            notify(hwnd, header, lparam)
        }
        WM_COMMAND => {
            match (wparam & 0xFFFF) as i32 {
                IDOK => enter(hwnd),
                IDCANCEL => unsafe {
                    DestroyWindow(hwnd);
                },
                _ => {}
            }
            0
        }
        // The read-only preview asks as a label does.
        WM_CTLCOLORSTATIC => controls::colors(message, wparam, lparam, Surface::Strip).unwrap_or(0),
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
        /// Questions asked, and the answer to give.
        asked: Vec<String>,
        answer: bool,
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
        fn confirm(&mut self, _owner: SendHwnd, question: &str) -> bool {
            let mut seen = self.0.borrow_mut();
            seen.asked.push(question.to_owned());
            seen.answer
        }
        fn changed(&mut self) {
            self.0.borrow_mut().changes += 1;
        }
    }

    #[test]
    fn an_open_review_window_follows_a_theme_change_and_paints_its_rows() {
        use crate::runner::process::tests::Temp;
        use crate::ui::theme::{DARK, LIGHT, ThemePreference};
        theme::set_system_for_test(theme::System::default());
        theme::set_preference(ThemePreference::Light);
        let temp = Temp::new("review-theme");
        temp.capture("lag spikes during ledger batch");
        let seen = Rc::new(RefCell::new(Seen::default()));
        let hwnd = open(Open {
            queue: temp.0.join("queue"),
            settings: None,
            notebook: Some(temp.notebook()),
            indexes: None,
            actions: Box::new(Watch(seen.clone())),
        })
        .unwrap();
        assert_eq!(state(hwnd).unwrap().colors, LIGHT);
        let list = state(hwnd).unwrap().list;
        let mut client: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetClientRect(list, &mut client) };
        let canvas = crate::ui::paint::tests::Canvas::new(client.right, client.bottom);
        unsafe {
            SendMessageW(
                list,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_PRINTCLIENT,
                canvas.dc as usize,
                windows_sys::Win32::UI::WindowsAndMessaging::PRF_CLIENT as isize,
            )
        };
        let mut row = RECT {
            left: windows_sys::Win32::UI::Controls::LVIR_BOUNDS as i32,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe {
            SendMessageW(
                list,
                windows_sys::Win32::UI::Controls::LVM_GETITEMRECT,
                0,
                (&raw mut row) as isize,
            )
        };
        let middle = (row.top + row.bottom) / 2;
        assert_eq!(
            canvas.pixel(1, middle),
            LIGHT.content_background,
            "beside the row's plate"
        );
        let plate = canvas.pixel(row.right / 2 + 40, middle);
        assert!(
            [
                LIGHT.selection_background,
                LIGHT.inactive_selection_background
            ]
            .contains(&plate),
            "the selected row's plate: {plate:06x}"
        );
        theme::set_preference(ThemePreference::Dark);
        unsafe { SendMessageW(hwnd, theme::changed_message(), 0, 0) };
        assert_eq!(state(hwnd).unwrap().colors, DARK);
        theme::set_preference(ThemePreference::System);
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn notes_are_shown_regenerated_and_discarded() {
        use crate::runner::process::tests::{Scripted, Temp, note};
        use crate::runner::process::{Outcome, process};
        let temp = Temp::new("review-notes");
        let job = temp.capture("lag spikes during ledger batch");
        let store = temp.store();
        let backend = Scripted::new(vec![Ok(note("Consumer lag spikes"))]);
        let Outcome::Written(written) = process(&store, job.clone(), &backend, 0, "2026-10-19")
        else {
            panic!()
        };
        let note_path = temp.notebook().join(&written.path);

        let seen = Rc::new(RefCell::new(Seen::default()));
        let hwnd = open(Open {
            queue: temp.0.join("queue"),
            settings: None,
            notebook: Some(temp.notebook()),
            indexes: None,
            actions: Box::new(Watch(seen.clone())),
        })
        .unwrap();
        let review = state(hwnd).unwrap();
        assert_eq!(review.rows[0].title, "Consumer lag spikes");
        assert_eq!(review.rows[0].status(), "Note");
        open_selected(hwnd);
        assert_eq!(
            seen.borrow().opened,
            std::slice::from_ref(&note_path),
            "E opens the note"
        );

        // R on an unedited note: queued again to replace it, no question.
        regenerate_selected(hwnd);
        let queued = store.get(&job.capture_id).unwrap();
        assert_eq!((queued.state, queued.regenerate), (State::Queued, true));
        assert!(seen.borrow().asked.is_empty());

        // R on an edited note asks; no keeps it as it was.
        let mut done = queued.clone();
        done.state = State::Done;
        done.regenerate = false;
        store.put(&done).unwrap();
        reload(hwnd);
        let edited = format!(
            "{}\nmy own words, longer than before\n",
            std::fs::read_to_string(&note_path).unwrap()
        );
        std::fs::write(&note_path, &edited).unwrap();
        regenerate_selected(hwnd);
        assert_eq!(seen.borrow().asked.len(), 1);
        assert_eq!(
            store.get(&job.capture_id).unwrap().state,
            State::Done,
            "declined"
        );
        seen.borrow_mut().answer = true;
        regenerate_selected(hwnd);
        let accepted = store.get(&job.capture_id).unwrap();
        assert_eq!(accepted.state, State::Queued);
        assert_eq!(
            accepted.note.unwrap().stamp,
            crate::note::write::Stamp::of(&note_path).unwrap(),
            "the edited note is what gets replaced"
        );

        // Del recycles the note and keeps the raw source; Shift+Del takes the rest.
        store.put(&done).unwrap();
        reload(hwnd);
        discard_selected(hwnd);
        assert_eq!(seen.borrow().recycled, std::slice::from_ref(&note_path));
        assert_eq!(store.get(&job.capture_id).unwrap().state, State::Discarded);
        assert!(
            state(hwnd).unwrap().rows.is_empty(),
            "a discarded note leaves the list"
        );
        assert!(temp.notebook().join(&job.raw).exists());

        // Ctrl+Z writes it back and the capture is done again.
        undo_last(hwnd);
        assert_eq!(std::fs::read_to_string(&note_path).unwrap(), edited);
        let back = store.get(&job.capture_id).unwrap();
        assert_eq!(back.state, State::Done);
        assert_eq!(back.note.unwrap().path, written.path);

        // Without a note, Del deletes the capture.
        let mut queued = store.get(&job.capture_id).unwrap();
        queued.state = State::Failed;
        queued.note = None;
        store.put(&queued).unwrap();
        reload(hwnd);
        discard_selected(hwnd);
        assert!(!temp.notebook().join(&job.raw).exists());
        assert!(store.get(&job.capture_id).is_none());

        unsafe { DestroyWindow(hwnd) };
    }

    /// A note waiting in the inbox for a new capture of `title`, suggesting `home`.
    fn inbox_note(
        temp: &crate::runner::process::tests::Temp,
        title: &str,
        home: Option<&str>,
    ) -> Job {
        let mut job = temp.capture(title);
        let path = PathBuf::from(format!(r"inbox\{title}.md"));
        let home = home
            .map(|home| format!("suggested_home: \"{home}\"\n"))
            .unwrap_or_default();
        let text = format!(
            "---\ntype: capture\ncaptured: 2026-10-19T10:00\nsummary: \"About {title}.\"\n{home}status: inbox\n---\n\n# {title}\n"
        );
        let absolute = temp.notebook().join(&path);
        std::fs::create_dir_all(absolute.parent().unwrap()).unwrap();
        std::fs::write(&absolute, text).unwrap();
        job.state = State::Done;
        job.note = Some(crate::note::write::Written {
            path,
            stamp: crate::note::write::Stamp::of(&absolute).unwrap(),
        });
        temp.store().put(&job).unwrap();
        job
    }

    fn select_titled(hwnd: HWND, title: &str) {
        let review = state(hwnd).unwrap();
        let index = review
            .rows
            .iter()
            .position(|row| row.title == title)
            .unwrap_or_else(|| panic!("{title} isn't listed"));
        select(review, index);
    }

    fn titles(hwnd: HWND) -> Vec<String> {
        let mut titles: Vec<String> = state(hwnd)
            .unwrap()
            .rows
            .iter()
            .map(|row| row.title.clone())
            .collect();
        titles.sort();
        titles
    }

    fn preview(hwnd: HWND) -> String {
        let review = state(hwnd).unwrap();
        let mut buffer = vec![0u16; 4096];
        let length = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowTextW(
                review.preview,
                buffer.as_mut_ptr(),
                buffer.len() as i32,
            )
        };
        String::from_utf16_lossy(&buffer[..length.max(0) as usize])
    }

    #[test]
    fn notes_are_filed_kept_and_put_back_from_the_keyboard() {
        use crate::runner::process::tests::Temp;
        let temp = Temp::new("review-filing");
        let notebook = temp.notebook();
        let write = |path: &str, text: &str| {
            let path = notebook.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            r"Payments\Payments incidents.md",
            "---\ntype: topic\n---\n# Payments incidents\n\n- 2026-10-01 [[Old outage]]\n",
        );
        write(r"Payments\Incidents\Earlier.md", "# Earlier\n");
        let lag = inbox_note(&temp, "Lag spikes", Some("[[Payments incidents]]"));
        let rebalance = inbox_note(&temp, "Rebalance", Some("Payments/Incidents/"));
        inbox_note(&temp, "Soup", None);
        let queued = temp.capture("still being written");

        let indexes = Indexes::new(None);
        let seen = Rc::new(RefCell::new(Seen::default()));
        let hwnd = open(Open {
            queue: temp.0.join("queue"),
            settings: None,
            notebook: Some(notebook.clone()),
            indexes: Some(indexes.clone()),
            actions: Box::new(Watch(seen.clone())),
        })
        .unwrap();
        assert_eq!(
            titles(hwnd),
            ["Lag spikes", "Rebalance", "Soup", "still being written"]
        );
        select_titled(hwnd, "Lag spikes");
        assert!(
            footer_text(state(hwnd).unwrap())
                .starts_with("Enter file \u{2192} Payments incidents \u{00B7} H home")
        );

        // Enter files to the topic note: listed there, moved beside it.
        enter(hwnd);
        assert_eq!(
            std::fs::read_to_string(notebook.join(r"Payments\Payments incidents.md")).unwrap(),
            "---\ntype: topic\n---\n# Payments incidents\n\n- 2026-10-01 [[Old outage]]\n- 2026-10-19 [[Lag spikes]]: About Lag spikes.\n"
                .replace("2026-10-19", &crate::notebook::dates::format(crate::platform::clock::local_now().0, "YYYY-MM-DD"))
        );
        let filed = temp.store().get(&lag.capture_id).unwrap().note.unwrap();
        assert_eq!(filed.path, PathBuf::from(r"Payments\Lag spikes.md"));
        assert!(
            std::fs::read_to_string(notebook.join(&filed.path))
                .unwrap()
                .contains("status: filed\n")
        );
        assert_eq!(titles(hwnd), ["Rebalance", "Soup", "still being written"]);

        // Enter files to a folder; Ctrl+Z puts it back in the inbox and the list.
        select_titled(hwnd, "Rebalance");
        enter(hwnd);
        assert!(notebook.join(r"Payments\Incidents\Rebalance.md").exists());
        assert!(!notebook.join(r"inbox\Rebalance.md").exists());
        assert_eq!(titles(hwnd), ["Soup", "still being written"]);
        undo_last(hwnd);
        assert!(notebook.join(r"inbox\Rebalance.md").exists());
        assert!(!notebook.join(r"Payments\Incidents\Rebalance.md").exists());
        assert_eq!(
            temp.store()
                .get(&rebalance.capture_id)
                .unwrap()
                .note
                .unwrap()
                .path,
            PathBuf::from(r"inbox\Rebalance.md")
        );
        assert_eq!(titles(hwnd), ["Rebalance", "Soup", "still being written"]);

        // H: the picker, narrowed by typing, files where you pick.
        select_titled(hwnd, "Soup");
        pick_home(hwnd);
        assert!(super::super::home_picker::find().is_some());
        super::super::home_picker::type_text("incidents folder");
        let picker = super::super::home_picker::find().unwrap();
        unsafe { SendMessageW(picker, WM_COMMAND, IDOK as usize, 0) };
        assert!(notebook.join(r"Payments\Incidents\Soup.md").exists());
        assert_eq!(titles(hwnd), ["Rebalance", "still being written"]);

        // I keeps it in the inbox and off the list.
        select_titled(hwnd, "Rebalance");
        keep_selected(hwnd);
        assert!(
            std::fs::read_to_string(notebook.join(r"inbox\Rebalance.md"))
                .unwrap()
                .contains("status: kept\n")
        );
        assert_eq!(titles(hwnd), ["still being written"]);

        // Enter on a capture with no note opens the folder.
        select_titled(hwnd, "still being written");
        enter(hwnd);
        assert_eq!(seen.borrow().folders, [notebook.join("raw")]);
        assert!(temp.store().get(&queued.capture_id).is_some());
        temp.store().remove(&queued.capture_id);

        // A topic note that can't be written leaves the note in the list, with the reason.
        inbox_note(&temp, "Another lag", Some("[[Payments incidents]]"));
        reload(hwnd);
        // Another program holds the topic note open, sharing nothing.
        use std::os::windows::fs::OpenOptionsExt;
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(notebook.join(r"Payments\Payments incidents.md"))
            .unwrap();
        select_titled(hwnd, "Another lag");
        enter(hwnd);
        drop(held);
        assert!(
            preview(hwnd).starts_with(
                "Couldn't file this note: Couldn't add the note to Payments incidents"
            ),
            "{}",
            preview(hwnd)
        );
        assert!(titles(hwnd).contains(&"Another lag".to_owned()));
        assert!(notebook.join(r"inbox\Another lag.md").exists());

        unsafe { DestroyWindow(hwnd) };
        indexes.stop();
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
                source_url: None,
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
            indexes: None,
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
        // Its link's page goes with it.
        let row = state(hwnd).unwrap().rows[0].clone();
        let page = crate::capture::sidecar::write(
            &notebook,
            &row.raw,
            &crate::capture::sidecar::Sidecar {
                capture_id: row.capture_id.clone(),
                url: "https://example.com/".to_owned(),
                fetched: "2026-10-19T10:31".to_owned(),
                page: Err("failed (403)".to_owned()),
            },
            None,
        )
        .unwrap();
        recycle_selected(hwnd);
        assert_eq!(seen.borrow().recycled.len(), 4);
        assert!(seen.borrow().recycled.contains(&notebook.join(page)));
        assert!(state(hwnd).unwrap().rows.is_empty());
        assert!(Store::new(queue).list().is_empty());

        unsafe { DestroyWindow(hwnd) };
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn a_finding_is_filed_put_back_placed_again_and_recycled_alone() {
        use crate::runner::process::tests::Temp;
        let temp = Temp::new("review-finding");
        let notebook = temp.notebook();
        std::fs::create_dir_all(notebook.join(r"Payments\Incidents")).unwrap();
        std::fs::write(
            notebook.join(r"Payments\Incidents\Earlier.md"),
            "# Earlier\n",
        )
        .unwrap();
        std::fs::create_dir_all(notebook.join("inbox")).unwrap();
        let path = PathBuf::from(r"inbox\Lag.md");
        std::fs::write(
            notebook.join(&path),
            "---\ntype: finding\nauthor: opencode\nsuggested_home: \"Payments/Incidents/\"\nstatus: inbox\n---\n\n# Lag\n",
        )
        .unwrap();
        let mut job = Job::finding(
            "FINDING1",
            notebook.clone(),
            crate::note::write::Written {
                path: path.clone(),
                stamp: crate::note::write::Stamp::of(&notebook.join(&path)).unwrap(),
            },
            "2026-10-23T14:05".to_owned(),
        );
        job.state = State::Done;
        let store = temp.store();
        store.put(&job).unwrap();

        let seen = Rc::new(RefCell::new(Seen::default()));
        let hwnd = open(Open {
            queue: temp.0.join("queue"),
            settings: None,
            notebook: Some(notebook.clone()),
            indexes: Some(Indexes::new(None)),
            actions: Box::new(Watch(seen.clone())),
        })
        .unwrap();
        assert_eq!(titles(hwnd), ["Lag"]);
        select_titled(hwnd, "Lag");
        assert_eq!(state(hwnd).unwrap().rows[0].status(), "opencode");
        assert!(footer_text(state(hwnd).unwrap()).contains("R place again"));

        // Enter files it in its suggested folder; its job follows it, and Ctrl+Z puts it back.
        enter(hwnd);
        assert!(notebook.join(r"Payments\Incidents\Lag.md").exists());
        assert!(titles(hwnd).is_empty());
        assert_eq!(
            store.get("FINDING1").unwrap().note.unwrap().path,
            PathBuf::from(r"Payments\Incidents\Lag.md")
        );
        undo_last(hwnd);
        assert!(notebook.join(&path).exists());
        assert_eq!(titles(hwnd), ["Lag"]);

        // Del recycles it, and its job stays to say so; Ctrl+Z brings both back.
        select_titled(hwnd, "Lag");
        discard_selected(hwnd);
        assert!(!notebook.join(&path).exists());
        assert!(titles(hwnd).is_empty());
        let discarded = store.get("FINDING1").unwrap();
        assert_eq!(discarded.state, State::Discarded);
        assert_eq!(discarded.note.unwrap().path, path);
        undo_last(hwnd);
        assert!(notebook.join(&path).exists());
        assert_eq!(titles(hwnd), ["Lag"]);

        // R places it again without asking, even after an edit: its words aren't replaced.
        std::fs::write(
            notebook.join(&path),
            "---\ntype: finding\nauthor: opencode\nstatus: inbox\n---\n\n# Lag\n\nMore.\n",
        )
        .unwrap();
        select_titled(hwnd, "Lag");
        regenerate_selected(hwnd);
        assert!(seen.borrow().asked.is_empty());
        assert_eq!(store.get("FINDING1").unwrap().state, State::Queued);

        // Shift+Del recycles the note alone.
        select_titled(hwnd, "Lag");
        let before = seen.borrow().recycled.len();
        recycle_selected(hwnd);
        assert_eq!(seen.borrow().recycled[before..], [notebook.join(&path)]);
        assert!(store.get("FINDING1").is_none());
        assert!(notebook.join(r"Payments\Incidents\Earlier.md").exists());

        unsafe { DestroyWindow(hwnd) };
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
            indexes: None,
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
