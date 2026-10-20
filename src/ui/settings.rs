//! The Settings window: the notebook, Local or Cloud, the local server and model, the cloud
//! provider (the Claude API, an OpenAI-compatible API, or the user's own Claude Code or Copilot
//! CLI) with its model and API key, the tokens used and the monthly cap, two switches, the
//! hotkeys, and the theme. Built from standard controls painted in the theme (`controls`), under FastPad's title
//! strip (`frame`), so the keyboard and screen readers work through Windows; choosing a theme
//! shows it at once, and Cancel puts the saved one back. Tab, Enter (Save) and Esc (Cancel) come from
//! `IsDialogMessageW` in the app's message loop. Finding servers, listing models, the vision
//! check and Test run on threads of their own and report back with `WM_SETTINGS_RESULT`. Local's
//! and Cloud's fields share rows, as the providers' do: only the chosen mode's and provider's are
//! shown, and the rows below move up and the window shrinks to fit them.
//! The rest stays in `fastcapture.ini` (Open settings file).

use super::controls::{self, Look, Segment, Surface};
use super::frame::{self, Buttons, Frame};
use super::paint;
use super::settings_form::Form;
use super::theme::metrics::TITLE_STRIP;
use super::theme::ramp::{self, TextStyle};
use super::theme::{self, ThemePreference, scale};
use crate::config::settings::{CloudBackend, Mode, Settings, SettingsFile};
use crate::model::claude::{self, Claude};
use crate::model::claude_code::{self, ClaudeCode};
use crate::model::copilot::{self, Copilot};
use crate::model::local::{self, Found};
use crate::model::openai::{self, OpenAi};
use crate::model::request::{CaptureFacts, NoteRequest};
use crate::model::{Backend, prompt};
use crate::runner::usage::{self, Ledger, thousands};
use std::path::{Path, PathBuf};
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{DeleteObject, HDC, HFONT, InvalidateRect, SetTextColor};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{BST_CHECKED, BST_UNCHECKED};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_AUTORADIOBUTTON, BS_DEFPUSHBUTTON, BS_PUSHBUTTON,
    CB_ADDSTRING, CB_ERR, CB_GETCURSEL, CB_GETLBTEXT, CB_GETLBTEXTLEN, CB_RESETCONTENT,
    CB_SETCURSEL, CBN_KILLFOCUS, CBN_SELCHANGE, CBS_DROPDOWN, CBS_DROPDOWNLIST, CW_USEDEFAULT,
    CreateWindowExW, DefWindowProcW, DestroyWindow, ES_AUTOHSCROLL, ES_PASSWORD, FindWindowW,
    GWLP_USERDATA, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, IDC_ARROW, IDCANCEL,
    IDOK, IsDialogMessageW, LoadCursorW, MSG, PostMessageW, RegisterClassW, SW_HIDE, SW_SHOW,
    SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, WM_APP, WM_CLOSE, WM_COMMAND,
    WM_CTLCOLORBTN, WM_CTLCOLOREDIT, WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_DPICHANGED,
    WM_ERASEBKGND, WM_NCDESTROY, WM_PAINT, WM_SETFONT, WNDCLASSW, WS_CAPTION, WS_CHILD,
    WS_CLIPCHILDREN, WS_EX_CLIENTEDGE, WS_GROUP, WS_MINIMIZEBOX, WS_SYSMENU, WS_TABSTOP,
    WS_VISIBLE, WS_VSCROLL,
};

/// A background job finished: a boxed `Event` in lparam.
const WM_SETTINGS_RESULT: u32 = WM_APP + 0x80;

const ID_NOTEBOOK: i32 = 101;
const ID_BROWSE: i32 = 102;
const ID_LOCAL: i32 = 103;
const ID_CLOUD: i32 = 104;
const ID_ENDPOINT: i32 = 105;
const ID_FIND: i32 = 106;
const ID_LOCAL_MODEL: i32 = 107;
const ID_VISION: i32 = 108;
const ID_CLOUD_MODEL: i32 = 110;
const ID_KEY: i32 = 111;
const ID_KEY_STATUS: i32 = 112;
const ID_TEST: i32 = 113;
const ID_TEST_STATUS: i32 = 114;
const ID_SIGN_IN: i32 = 115;
const ID_SOURCE: i32 = 116;
const ID_OPEN_FILE: i32 = 117;
const ID_PROBLEM: i32 = 118;
const ID_PROVIDER: i32 = 119;
const ID_ADDRESS: i32 = 120;
const ID_OPENAI_MODEL: i32 = 121;
const ID_LIST_MODELS: i32 = 122;
const ID_USAGE: i32 = 123;
const ID_CAP: i32 = 124;
const ID_CLAUDE_CODE_MODEL: i32 = 125;
const ID_CLAUDE_CODE_STATUS: i32 = 126;
const ID_COPILOT_MODEL: i32 = 127;
const ID_COPILOT_STATUS: i32 = 128;
const ID_THEME_SYSTEM: i32 = 129;
const ID_THEME_LIGHT: i32 = 130;
const ID_THEME_DARK: i32 = 131;
const ID_CAPTURE_HOTKEY: i32 = 132;
const ID_CLIPBOARD_HOTKEY: i32 = 133;

const WIDTH: i32 = 560;
/// The title strip, the Theme row, then the form.
const THEME_ROW: i32 = 40;
/// The window with Cloud's rows, the tallest.
const HEIGHT: i32 = TITLE_STRIP + THEME_ROW + 600;
/// The theme choice's buttons, in order.
const THEMES: [(i32, ThemePreference); 3] = [
    (ID_THEME_SYSTEM, ThemePreference::System),
    (ID_THEME_LIGHT, ThemePreference::Light),
    (ID_THEME_DARK, ThemePreference::Dark),
];

/// The provider combo's items, in `CloudBackend` order.
const PROVIDERS: [(CloudBackend, &str); 4] = [
    (CloudBackend::Claude, "Claude API (Anthropic)"),
    (CloudBackend::OpenAi, "OpenAI-compatible API"),
    (
        CloudBackend::ClaudeCode,
        "Claude Code (your Claude plan, experimental)",
    ),
    (
        CloudBackend::Copilot,
        "GitHub Copilot (your Copilot plan, experimental)",
    ),
];

/// What a background job found.
enum Event {
    Servers(Vec<Found>),
    Models(String, Result<Vec<String>, String>),
    /// The OpenAI-compatible API's models.
    CloudModels(String, Result<Vec<String>, String>),
    Vision(String, String, Option<bool>),
    Tested(Result<String, String>),
    /// Which Claude Code is used, and whether it is signed in.
    ClaudeCode(String),
    /// Which Copilot CLI is used, who is signed in, and the models the plan offers.
    Copilot(copilot::Status),
}

/// What the window opens with.
pub struct Open {
    pub settings: PathBuf,
    /// The Credential Manager names the Claude and OpenAI-compatible keys are saved under.
    pub claude_key: String,
    pub openai_key: String,
    /// The usage ledger, for the tokens used.
    pub usage: Option<PathBuf>,
    /// The settings were saved: apply them now.
    pub saved: Box<dyn Fn()>,
    /// Open `fastcapture.ini` itself.
    pub open_file: Box<dyn Fn()>,
}

struct Controls {
    /// System, Light and Dark, in `THEMES` order.
    themes: [HWND; 3],
    notebook: HWND,
    local: HWND,
    cloud: HWND,
    endpoint: HWND,
    local_model: HWND,
    vision: HWND,
    provider: HWND,
    cloud_model: HWND,
    openai_address: HWND,
    claude_address: HWND,
    openai_model: HWND,
    claude_code_model: HWND,
    claude_code_status: HWND,
    copilot_model: HWND,
    copilot_status: HWND,
    key: HWND,
    key_status: HWND,
    test_status: HWND,
    usage: HWND,
    cap: HWND,
    sign_in: HWND,
    source: HWND,
    capture_hotkey: HWND,
    clipboard_hotkey: HWND,
    problem: HWND,
    /// Status and hint lines, in muted text.
    muted: Vec<HWND>,
    /// Everything that only applies to Local mode, and to Cloud mode.
    local_group: Vec<HWND>,
    cloud_group: Vec<HWND>,
    /// Shown only for one provider.
    claude_only: Vec<HWND>,
    openai_only: Vec<HWND>,
    claude_code_only: Vec<HWND>,
    copilot_only: Vec<HWND>,
    /// The address and key rows, which Claude Code and Copilot have no use for.
    api_only: Vec<HWND>,
    /// Every control and where it sits at 96 DPI with Cloud chosen, to place them again at
    /// another DPI or mode.
    placed: Vec<(HWND, [i32; 4])>,
    /// Where in `placed` the rows below the mode's fields start: Test and everything after.
    below: usize,
}

struct SettingsWindow {
    open: Open,
    settings: Settings,
    claude_key_saved: bool,
    openai_key_saved: bool,
    font: HFONT,
    controls: Controls,
    /// The Claude models listed in the combo, in order.
    claude_models: Vec<String>,
    frame: Frame,
}

impl SettingsWindow {
    /// Where the provider's key is saved; Claude Code and Copilot have none.
    fn key_target(&self, backend: CloudBackend) -> Option<&str> {
        match backend {
            CloudBackend::Claude => Some(&self.open.claude_key),
            CloudBackend::OpenAi => Some(&self.open.openai_key),
            CloudBackend::ClaudeCode | CloudBackend::Copilot => None,
        }
    }

    fn key_saved(&self, backend: CloudBackend) -> bool {
        match backend {
            CloudBackend::Claude => self.claude_key_saved,
            CloudBackend::OpenAi => self.openai_key_saved,
            CloudBackend::ClaudeCode | CloudBackend::Copilot => false,
        }
    }

    /// The provider the combo shows.
    fn provider(&self) -> CloudBackend {
        let index = unsafe { SendMessageW(self.controls.provider, CB_GETCURSEL, 0, 0) };
        usize::try_from(index)
            .ok()
            .and_then(|index| PROVIDERS.get(index))
            .map_or(CloudBackend::Claude, |(backend, _)| *backend)
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn class_name() -> &'static [u16] {
    static NAME: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| wide("FastCaptureSettings"))
}

fn register_class() -> bool {
    static REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *REGISTERED.get_or_init(|| {
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let class = WNDCLASSW {
            lpfnWndProc: Some(settings_proc),
            hInstance: instance,
            hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
            hIcon: unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::LoadIconW(instance, 1 as _)
            },
            lpszClassName: class_name().as_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        unsafe { RegisterClassW(&class) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS }
    })
}

/// The open Settings window, if any.
pub fn find() -> Option<HWND> {
    let hwnd = unsafe { FindWindowW(class_name().as_ptr(), std::ptr::null()) };
    (!hwnd.is_null()).then_some(hwnd)
}

/// Lets the Settings window handle Tab, Enter and Esc. Call from the message loop.
pub fn dialog_message(message: &MSG) -> bool {
    match find() {
        Some(hwnd) => unsafe { IsDialogMessageW(hwnd, message) != 0 },
        None => false,
    }
}

fn state(hwnd: HWND) -> Option<&'static mut SettingsWindow> {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut SettingsWindow;
    (!pointer.is_null()).then(|| unsafe { &mut *pointer })
}

fn text_of(control: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(control) }.max(0) as usize;
    let mut buffer = vec![0u16; length + 1];
    let copied = unsafe { GetWindowTextW(control, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
}

fn set_text(control: HWND, text: &str) {
    unsafe { SetWindowTextW(control, wide(text).as_ptr()) };
}

fn checked(control: HWND) -> bool {
    let state = unsafe { SendMessageW(control, BM_GETCHECK, 0, 0) };
    state == BST_CHECKED as isize
}

fn set_checked(control: HWND, on: bool) {
    let state = if on { BST_CHECKED } else { BST_UNCHECKED };
    unsafe { SendMessageW(control, BM_SETCHECK, state as usize, 0) };
}

fn combo_fill(combo: HWND, items: &[String]) {
    unsafe { SendMessageW(combo, CB_RESETCONTENT, 0, 0) };
    for item in items {
        let text = wide(item);
        unsafe { SendMessageW(combo, CB_ADDSTRING, 0, text.as_ptr() as isize) };
    }
}

fn combo_selected_text(combo: HWND) -> Option<String> {
    let index = unsafe { SendMessageW(combo, CB_GETCURSEL, 0, 0) };
    if index == CB_ERR as isize {
        return None;
    }
    let length = unsafe { SendMessageW(combo, CB_GETLBTEXTLEN, index as usize, 0) };
    if length < 0 {
        return None;
    }
    let mut buffer = vec![0u16; length as usize + 1];
    unsafe {
        SendMessageW(
            combo,
            CB_GETLBTEXT,
            index as usize,
            buffer.as_mut_ptr() as isize,
        )
    };
    Some(String::from_utf16_lossy(&buffer[..length as usize]))
}

/// The text of an editable combo: its edit text, or the item just chosen (during
/// `CBN_SELCHANGE` the edit text is still the old one).
fn combo_text(combo: HWND) -> String {
    let typed = text_of(combo);
    if typed.is_empty() {
        combo_selected_text(combo).unwrap_or_default()
    } else {
        typed
    }
}

/// Opens the Settings window, or brings the open one forward.
pub fn open(options: Open) -> Option<HWND> {
    if let Some(hwnd) = find() {
        unsafe {
            ShowWindow(hwnd, SW_SHOWNORMAL);
            SetForegroundWindow(hwnd);
        }
        return Some(hwnd);
    }
    if !register_class() {
        return None;
    }
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let style = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name().as_ptr(),
            wide("FastCapture Settings").as_ptr(),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WIDTH,
            HEIGHT,
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
    // The frame gives up the native caption and border: the client area is the whole window.
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            scale(WIDTH, dpi),
            scale(HEIGHT, dpi),
            windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE | SWP_NOZORDER,
        )
    };
    let font = ramp::font(TextStyle::Body, dpi);
    let (settings, _) = SettingsFile::load(&options.settings).settings();
    let claude_key_saved = crate::secrets::load(&options.claude_key).is_some();
    let openai_key_saved = crate::secrets::load(&options.openai_key).is_some();
    let controls = create_controls(hwnd, dpi, font);
    let mut claude_models: Vec<String> = claude::MODELS
        .iter()
        .map(|(id, _)| (*id).to_owned())
        .collect();
    if !claude_models.contains(&settings.claude_model) {
        claude_models.push(settings.claude_model.clone());
    }
    let window = Box::new(SettingsWindow {
        open: options,
        settings,
        claude_key_saved,
        openai_key_saved,
        font,
        controls,
        claude_models,
        frame: Frame::new(
            "FastCapture Settings",
            Buttons {
                minimize: true,
                maximize: false,
            },
            false,
            dpi,
        ),
    });
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(window) as isize) };
    frame::attach(hwnd, &theme::current());
    load(hwnd);
    unsafe {
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
    }
    // Look for local servers straight away when Local is chosen and none is set.
    if let Some(window) = state(hwnd)
        && window.settings.mode != Some(Mode::Cloud)
        && window.settings.local_endpoint.is_none()
    {
        find_servers(hwnd);
    }
    Some(hwnd)
}

fn create_controls(hwnd: HWND, dpi: u32, font: HFONT) -> Controls {
    let s = |value: i32| scale(value, dpi);
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let placed = std::cell::RefCell::new(Vec::new());
    // Rows are placed below the title strip and the Theme row.
    let make =
        |class: &str, text: &str, style: u32, ex: u32, x: i32, y: i32, w: i32, h: i32, id: i32| {
            let y = y + TITLE_STRIP + THEME_ROW;
            let control = unsafe {
                CreateWindowExW(
                    ex,
                    wide(class).as_ptr(),
                    wide(text).as_ptr(),
                    WS_CHILD | WS_VISIBLE | style,
                    s(x),
                    s(y),
                    s(w),
                    s(h),
                    hwnd,
                    id as isize as _,
                    instance,
                    std::ptr::null(),
                )
            };
            unsafe { SendMessageW(control, WM_SETFONT, font as usize, 1) };
            paint(control, class, style);
            placed.borrow_mut().push((control, [x, y, w, h]));
            control
        };
    let label = |text: &str, y: i32| make("STATIC", text, 0, 0, 16, y + 4, 92, 20, -1);
    let (field_x, field_w, button_x, button_w) = (112, 320, 440, 104);
    let edit = WS_TABSTOP | ES_AUTOHSCROLL as u32;

    let theme_label = make("STATIC", "Theme", 0, 0, 16, 20 - THEME_ROW, 92, 20, -1);
    let _ = theme_label;
    let themes = [
        (
            "System",
            WS_TABSTOP | WS_GROUP,
            ID_THEME_SYSTEM,
            Segment::First,
        ),
        ("Light", 0, ID_THEME_LIGHT, Segment::Middle),
        ("Dark", 0, ID_THEME_DARK, Segment::Last),
    ]
    .iter()
    .enumerate()
    .map(|(n, (text, flags, id, segment))| {
        let button = make(
            "BUTTON",
            text,
            flags | BS_AUTORADIOBUTTON as u32,
            0,
            field_x + 104 * n as i32,
            16 - THEME_ROW,
            104,
            26,
            *id,
        );
        controls::paint_as(button, Look::Segment(*segment), Surface::Content);
        button
    })
    .collect::<Vec<_>>();

    label("Notebook", 16);
    let notebook = make(
        "EDIT",
        "",
        edit,
        WS_EX_CLIENTEDGE,
        field_x,
        16,
        field_w,
        26,
        ID_NOTEBOOK,
    );
    make(
        "BUTTON",
        "Browse…",
        WS_TABSTOP | BS_PUSHBUTTON as u32,
        0,
        button_x,
        15,
        button_w,
        26,
        ID_BROWSE,
    );

    label("Notes by", 56);
    let local = make(
        "BUTTON",
        "A model on this PC (Local)",
        WS_TABSTOP | WS_GROUP | BS_AUTORADIOBUTTON as u32,
        0,
        field_x,
        56,
        216,
        26,
        ID_LOCAL,
    );
    controls::paint_as(local, Look::Segment(Segment::First), Surface::Content);
    let cloud = make(
        "BUTTON",
        "A cloud API (Cloud)",
        BS_AUTORADIOBUTTON as u32,
        0,
        field_x + 216,
        56,
        216,
        26,
        ID_CLOUD,
    );
    controls::paint_as(cloud, Look::Segment(Segment::Last), Surface::Content);

    let server_label = label("Server", 96);
    let endpoint = make(
        "COMBOBOX",
        "",
        WS_TABSTOP | WS_GROUP | WS_VSCROLL | CBS_DROPDOWN as u32,
        0,
        field_x,
        96,
        field_w,
        200,
        ID_ENDPOINT,
    );
    let find = make(
        "BUTTON",
        "Find servers",
        WS_TABSTOP | BS_PUSHBUTTON as u32,
        0,
        button_x,
        95,
        button_w,
        26,
        ID_FIND,
    );
    let model_label = label("Model", 132);
    let local_model = make(
        "COMBOBOX",
        "",
        WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWN as u32,
        0,
        field_x,
        132,
        field_w,
        200,
        ID_LOCAL_MODEL,
    );
    let vision = make(
        "STATIC",
        "",
        0,
        0,
        field_x,
        160,
        field_w + button_w,
        34,
        ID_VISION,
    );

    let provider_label = label("Provider", 96);
    let provider = make(
        "COMBOBOX",
        "",
        WS_TABSTOP | WS_GROUP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
        0,
        field_x,
        96,
        field_w,
        200,
        ID_PROVIDER,
    );
    let address_label = label("Address", 132);
    let claude_address = make(
        "EDIT",
        claude::API_BASE,
        ES_AUTOHSCROLL as u32,
        WS_EX_CLIENTEDGE,
        field_x,
        132,
        field_w,
        26,
        -1,
    );
    let openai_address = make(
        "EDIT",
        "",
        edit,
        WS_EX_CLIENTEDGE,
        field_x,
        132,
        field_w,
        26,
        ID_ADDRESS,
    );
    let cloud_label = label("Model", 168);
    let cloud_model = make(
        "COMBOBOX",
        "",
        WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
        0,
        field_x,
        168,
        field_w,
        200,
        ID_CLOUD_MODEL,
    );
    let openai_model = make(
        "COMBOBOX",
        "",
        WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWN as u32,
        0,
        field_x,
        168,
        field_w,
        200,
        ID_OPENAI_MODEL,
    );
    let list_models = make(
        "BUTTON",
        "List models",
        WS_TABSTOP | BS_PUSHBUTTON as u32,
        0,
        button_x,
        167,
        button_w,
        26,
        ID_LIST_MODELS,
    );
    let claude_code_label = label("Claude Code", 132);
    let claude_code_status = make(
        "STATIC",
        "",
        0,
        0,
        field_x,
        128,
        field_w + button_w,
        34,
        ID_CLAUDE_CODE_STATUS,
    );
    let claude_code_model = make(
        "COMBOBOX",
        "",
        WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWN as u32,
        0,
        field_x,
        168,
        field_w,
        200,
        ID_CLAUDE_CODE_MODEL,
    );
    let copilot_label = label("Copilot", 132);
    let copilot_status = make(
        "STATIC",
        "",
        0,
        0,
        field_x,
        128,
        field_w + button_w,
        34,
        ID_COPILOT_STATUS,
    );
    let copilot_model = make(
        "COMBOBOX",
        "",
        WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWN as u32,
        0,
        field_x,
        168,
        field_w,
        200,
        ID_COPILOT_MODEL,
    );
    let key_label = label("API key", 204);
    let key = make(
        "EDIT",
        "",
        edit | ES_PASSWORD as u32,
        WS_EX_CLIENTEDGE,
        field_x,
        204,
        field_w,
        26,
        ID_KEY,
    );
    let key_status = make(
        "STATIC",
        "",
        0,
        0,
        field_x,
        234,
        field_w + button_w,
        20,
        ID_KEY_STATUS,
    );

    let usage = make("STATIC", "", 0, 0, 16, 262, WIDTH - 32, 20, ID_USAGE);
    let cap_label = label("Token cap", 290);
    let cap = make(
        "EDIT",
        "",
        edit,
        WS_EX_CLIENTEDGE,
        field_x,
        290,
        96,
        26,
        ID_CAP,
    );
    let cap_hint = make(
        "STATIC",
        "tokens a month, like 2M or 500k; empty for none",
        0,
        0,
        field_x + 106,
        294,
        field_w + button_w - 106,
        20,
        -1,
    );

    let below = placed.borrow().len();
    make(
        "BUTTON",
        "Test",
        WS_TABSTOP | BS_PUSHBUTTON as u32,
        0,
        field_x,
        330,
        96,
        26,
        ID_TEST,
    );
    let test_status = make(
        "STATIC",
        "",
        0,
        0,
        field_x + 106,
        334,
        field_w + button_w - 106,
        40,
        ID_TEST_STATUS,
    );

    let sign_in = make(
        "BUTTON",
        "Start FastCapture when I sign in",
        WS_TABSTOP | BS_AUTOCHECKBOX as u32,
        0,
        field_x,
        382,
        360,
        28,
        ID_SIGN_IN,
    );
    let source = make(
        "BUTTON",
        "Record the title of the window I capture from",
        WS_TABSTOP | BS_AUTOCHECKBOX as u32,
        0,
        field_x,
        414,
        360,
        28,
        ID_SOURCE,
    );

    label("Capture key", 450);
    let capture_hotkey = make(
        "EDIT",
        "",
        edit,
        WS_EX_CLIENTEDGE,
        field_x,
        450,
        160,
        26,
        ID_CAPTURE_HOTKEY,
    );
    let capture_hint = make(
        "STATIC",
        "opens the capture box, like Ctrl+Alt+C",
        0,
        0,
        field_x + 170,
        454,
        field_w + button_w - 170,
        20,
        -1,
    );
    label("Clipboard key", 486);
    let clipboard_hotkey = make(
        "EDIT",
        "",
        edit,
        WS_EX_CLIENTEDGE,
        field_x,
        486,
        160,
        26,
        ID_CLIPBOARD_HOTKEY,
    );
    let clipboard_hint = make(
        "STATIC",
        "captures the clipboard; empty for none",
        0,
        0,
        field_x + 170,
        490,
        field_w + button_w - 170,
        20,
        -1,
    );

    let problem = make("STATIC", "", 0, 0, 16, 522, WIDTH - 32, 20, ID_PROBLEM);
    make(
        "BUTTON",
        "Open settings file",
        WS_TABSTOP | BS_PUSHBUTTON as u32,
        0,
        16,
        552,
        140,
        28,
        ID_OPEN_FILE,
    );
    make(
        "BUTTON",
        "Save",
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        WIDTH - 16 - 2 * 96 - 8,
        552,
        96,
        28,
        IDOK,
    );
    make(
        "BUTTON",
        "Cancel",
        WS_TABSTOP | BS_PUSHBUTTON as u32,
        0,
        WIDTH - 16 - 96,
        552,
        96,
        28,
        IDCANCEL,
    );

    Controls {
        themes: [themes[0], themes[1], themes[2]],
        notebook,
        local,
        cloud,
        endpoint,
        local_model,
        vision,
        provider,
        cloud_model,
        openai_address,
        claude_address,
        openai_model,
        claude_code_model,
        claude_code_status,
        copilot_model,
        copilot_status,
        key,
        key_status,
        test_status,
        usage,
        cap,
        sign_in,
        source,
        capture_hotkey,
        clipboard_hotkey,
        problem,
        muted: vec![
            vision,
            claude_code_status,
            copilot_status,
            key_status,
            test_status,
            usage,
            cap_hint,
            capture_hint,
            clipboard_hint,
        ],
        local_group: vec![
            server_label,
            endpoint,
            find,
            model_label,
            local_model,
            vision,
        ],
        cloud_group: vec![
            provider_label,
            provider,
            address_label,
            claude_address,
            openai_address,
            cloud_label,
            cloud_model,
            openai_model,
            list_models,
            claude_code_label,
            claude_code_status,
            claude_code_model,
            copilot_label,
            copilot_status,
            copilot_model,
            key_label,
            key,
            key_status,
            usage,
            cap_label,
            cap,
            cap_hint,
        ],
        claude_only: vec![claude_address, cloud_model],
        openai_only: vec![openai_address, openai_model, list_models],
        claude_code_only: vec![claude_code_label, claude_code_status, claude_code_model],
        copilot_only: vec![copilot_label, copilot_status, copilot_model],
        api_only: vec![address_label, key_label, key, key_status],
        placed: placed.take(),
        below,
    }
}

/// Follows the window to a monitor with another scale: the size Windows suggests, the fonts,
/// the strip and every control at the new DPI.
fn change_dpi(hwnd: HWND, dpi: u32, suggested: &RECT) {
    let Some(window) = state(hwnd) else {
        return;
    };
    window.frame.set_dpi(dpi);
    let font = ramp::font(TextStyle::Body, dpi);
    for &(control, _) in &window.controls.placed {
        unsafe { SendMessageW(control, WM_SETFONT, font as usize, 0) };
    }
    unsafe { DeleteObject(std::mem::replace(&mut window.font, font)) };
    unsafe {
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
    place(hwnd, dpi);
}

/// How far the rows below the mode's fields move up: none under Cloud's rows, which are the
/// tallest; Local's are shorter, and with no mode chosen there are none.
fn lift(mode: Option<Mode>) -> i32 {
    match mode {
        Some(Mode::Cloud) => 0,
        Some(Mode::Local) => 122,
        None => 220,
    }
}

/// Places every control for the chosen mode at `dpi`, and fits the window's height to them.
fn place(hwnd: HWND, dpi: u32) {
    let Some(window) = state(hwnd) else {
        return;
    };
    let controls = &window.controls;
    let lift = lift(if checked(controls.local) {
        Some(Mode::Local)
    } else if checked(controls.cloud) {
        Some(Mode::Cloud)
    } else {
        None
    });
    for (index, &(control, [x, y, w, h])) in controls.placed.iter().enumerate() {
        let y = if index >= controls.below { y - lift } else { y };
        unsafe {
            SetWindowPos(
                control,
                std::ptr::null_mut(),
                scale(x, dpi),
                scale(y, dpi),
                scale(w, dpi),
                scale(h, dpi),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            scale(WIDTH, dpi),
            scale(HEIGHT - lift, dpi),
            windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        InvalidateRect(hwnd, std::ptr::null(), 1);
    }
}

/// Paints a control in the theme by its kind; radio buttons are painted where they are made,
/// since each knows its place in its choice.
fn paint(control: HWND, class: &str, style: u32) {
    let look = match class {
        "BUTTON" => match style & 0xF {
            kind if kind == BS_AUTOCHECKBOX as u32 => Some(Look::Toggle),
            kind if kind == BS_AUTORADIOBUTTON as u32 => None,
            _ => Some(Look::Button),
        },
        "COMBOBOX" => Some(Look::Combo),
        "EDIT" => Some(Look::Field),
        _ => None,
    };
    if let Some(look) = look {
        controls::paint_as(control, look, Surface::Content);
    }
}

/// Fills the controls from the settings.
fn load(hwnd: HWND) {
    let Some(window) = state(hwnd) else {
        return;
    };
    let form = Form::from_settings(&window.settings, window.key_saved(window.settings.backend));
    let controls = &window.controls;
    set_text(controls.notebook, &form.notebook);
    set_checked(controls.local, form.mode == Some(Mode::Local));
    set_checked(controls.cloud, form.mode == Some(Mode::Cloud));
    for (button, (_, theme)) in controls.themes.iter().zip(THEMES) {
        set_checked(*button, form.theme == theme);
    }
    set_text(controls.endpoint, &form.local_endpoint);
    set_text(controls.local_model, &form.local_model);
    let labels: Vec<String> = window
        .claude_models
        .iter()
        .map(|id| {
            claude::MODELS
                .iter()
                .find(|(known, _)| known == id)
                .map_or_else(|| id.clone(), |(_, name)| format!("{name}  ({id})"))
        })
        .collect();
    combo_fill(controls.cloud_model, &labels);
    let index = window
        .claude_models
        .iter()
        .position(|id| *id == form.claude_model)
        .unwrap_or(0);
    unsafe { SendMessageW(controls.cloud_model, CB_SETCURSEL, index, 0) };
    let providers: Vec<String> = PROVIDERS
        .iter()
        .map(|(_, name)| (*name).to_owned())
        .collect();
    combo_fill(controls.provider, &providers);
    let provider = PROVIDERS
        .iter()
        .position(|(backend, _)| *backend == form.backend)
        .unwrap_or(0);
    unsafe { SendMessageW(controls.provider, CB_SETCURSEL, provider, 0) };
    set_text(controls.openai_address, &form.openai_endpoint);
    set_text(controls.openai_model, &form.openai_model);
    let aliases: Vec<String> = claude_code::MODELS
        .iter()
        .map(|(alias, _)| (*alias).to_owned())
        .collect();
    combo_fill(controls.claude_code_model, &aliases);
    set_text(controls.claude_code_model, &form.claude_code_model);
    combo_fill(controls.copilot_model, &[copilot::DEFAULT_MODEL.to_owned()]);
    set_text(controls.copilot_model, &form.copilot_model);
    set_text(controls.cap, &form.token_cap);
    set_text(
        controls.usage,
        &usage_text(window.open.usage.as_deref(), window.settings.token_cap),
    );
    set_checked(controls.sign_in, form.start_at_sign_in);
    set_checked(controls.source, form.capture_source);
    set_text(controls.capture_hotkey, &form.capture_hotkey);
    set_text(controls.clipboard_hotkey, &form.clipboard_hotkey);
    if !form.local_endpoint.is_empty() {
        list_models(hwnd, form.local_endpoint.clone());
    }
    enable_for_mode(hwnd);
}

/// The form as the controls show it.
fn read_form(window: &SettingsWindow) -> Form {
    let controls = &window.controls;
    let mode = if checked(controls.local) {
        Some(Mode::Local)
    } else if checked(controls.cloud) {
        Some(Mode::Cloud)
    } else {
        None
    };
    let index = unsafe { SendMessageW(controls.cloud_model, CB_GETCURSEL, 0, 0) };
    let claude_model = usize::try_from(index)
        .ok()
        .and_then(|index| window.claude_models.get(index).cloned())
        .unwrap_or_else(|| window.settings.claude_model.clone());
    Form {
        notebook: text_of(controls.notebook).trim().to_owned(),
        mode,
        backend: window.provider(),
        local_endpoint: combo_text(controls.endpoint).trim().to_owned(),
        local_model: combo_text(controls.local_model).trim().to_owned(),
        claude_model,
        openai_endpoint: text_of(controls.openai_address).trim().to_owned(),
        openai_model: combo_text(controls.openai_model).trim().to_owned(),
        claude_code_model: combo_text(controls.claude_code_model).trim().to_owned(),
        copilot_model: combo_text(controls.copilot_model).trim().to_owned(),
        new_key: text_of(controls.key).trim().to_owned(),
        key_saved: window.key_saved(window.provider()),
        token_cap: text_of(controls.cap).trim().to_owned(),
        capture_hotkey: text_of(controls.capture_hotkey).trim().to_owned(),
        clipboard_hotkey: text_of(controls.clipboard_hotkey).trim().to_owned(),
        start_at_sign_in: checked(controls.sign_in),
        capture_source: checked(controls.source),
        theme: controls
            .themes
            .iter()
            .zip(THEMES)
            .find(|(button, _)| checked(**button))
            .map_or(ThemePreference::System, |(_, (_, theme))| theme),
    }
}

fn enable_for_mode(hwnd: HWND) {
    let Some(window) = state(hwnd) else {
        return;
    };
    let local = checked(window.controls.local);
    let cloud = checked(window.controls.cloud);
    // Anthropic's address is shown, not edited.
    unsafe { EnableWindow(window.controls.claude_address, 0) };
    let provider = window.provider();
    let show = |controls: &[HWND], on: bool| {
        for control in controls {
            unsafe { ShowWindow(*control, if on { SW_SHOW } else { SW_HIDE }) };
        }
    };
    show(&window.controls.local_group, local);
    show(&window.controls.cloud_group, cloud);
    if cloud {
        show(
            &window.controls.claude_only,
            provider == CloudBackend::Claude,
        );
        show(
            &window.controls.openai_only,
            provider == CloudBackend::OpenAi,
        );
        show(
            &window.controls.claude_code_only,
            provider == CloudBackend::ClaudeCode,
        );
        show(
            &window.controls.copilot_only,
            provider == CloudBackend::Copilot,
        );
        show(
            &window.controls.api_only,
            !matches!(provider, CloudBackend::ClaudeCode | CloudBackend::Copilot),
        );
    }
    place(hwnd, unsafe { GetDpiForWindow(hwnd) }.max(96));
    let Some(window) = state(hwnd) else {
        return;
    };
    if provider == CloudBackend::ClaudeCode
        && text_of(window.controls.claude_code_status).is_empty()
    {
        check_claude_code(hwnd);
    }
    if provider == CloudBackend::Copilot && text_of(window.controls.copilot_status).is_empty() {
        check_copilot(hwnd);
    }
    set_text(
        window.controls.key_status,
        if window.key_saved(provider) {
            "A key is saved in Windows Credential Manager. Type a new one to replace it."
        } else {
            "Saved in Windows Credential Manager, never in a file."
        },
    );
}

/// Finds Claude Code and asks whether it is signed in, off the UI thread.
fn check_claude_code(hwnd: HWND) {
    let Some(window) = state(hwnd) else {
        return;
    };
    set_text(
        window.controls.claude_code_status,
        "Looking for Claude Code…",
    );
    let path = window.settings.claude_code_path.clone();
    in_background(hwnd, move || {
        Event::ClaudeCode(claude_code::status(path.as_deref()))
    });
}

/// Finds the Copilot CLI, and asks who is signed in and which models the plan offers, off the
/// UI thread.
fn check_copilot(hwnd: HWND) {
    let Some(window) = state(hwnd) else {
        return;
    };
    set_text(
        window.controls.copilot_status,
        "Looking for the Copilot CLI…",
    );
    let path = window.settings.copilot_path.clone();
    in_background(hwnd, move || {
        Event::Copilot(copilot::status(path.as_deref()))
    });
}

/// "Tokens used: 12,400 today, 210,000 this month…", from the ledger at `ledger`.
fn usage_text(ledger: Option<&Path>, cap: Option<u64>) -> String {
    let Some(ledger) = ledger.map(Ledger::load) else {
        return String::new();
    };
    let day = usage::today();
    let month = ledger.month(&day).total();
    let mut text = format!(
        "Tokens used: {} today, {} this month, as the cloud providers reported them.",
        thousands(ledger.day(&day).total()),
        thousands(month)
    );
    if cap.is_some_and(|cap| month >= cap) {
        text.push_str(" The cap is reached.");
    }
    text
}

/// Runs `job` on a thread and posts its event back to the window.
fn in_background(hwnd: HWND, job: impl FnOnce() -> Event + Send + 'static) {
    let target = hwnd as usize;
    std::thread::spawn(move || {
        let event = Box::into_raw(Box::new(job()));
        if unsafe { PostMessageW(target as HWND, WM_SETTINGS_RESULT, 0, event as isize) } == 0 {
            drop(unsafe { Box::from_raw(event) });
        }
    });
}

fn find_servers(hwnd: HWND) {
    if let Some(window) = state(hwnd) {
        set_text(
            window.controls.vision,
            "Looking for model servers on this PC…",
        );
    }
    in_background(hwnd, || Event::Servers(local::find_servers()));
}

fn list_models(hwnd: HWND, endpoint: String) {
    in_background(hwnd, move || {
        let models = openai::list_models(&endpoint, None, Duration::from_secs(5))
            .map_err(|error| error.to_string());
        Event::Models(endpoint, models)
    });
}

/// The OpenAI-compatible API's models, with the typed or saved key.
fn list_cloud_models(hwnd: HWND) {
    let Some(window) = state(hwnd) else {
        return;
    };
    let endpoint = text_of(window.controls.openai_address).trim().to_owned();
    if !crate::net::http::parse_url(&endpoint).is_ok_and(|target| target.secure) {
        set_text(
            window.controls.test_status,
            "Enter the API's https:// address first.",
        );
        return;
    }
    let typed = text_of(window.controls.key).trim().to_owned();
    let key = if typed.is_empty() {
        crate::secrets::load(&window.open.openai_key)
    } else {
        Some(typed)
    };
    set_text(
        window.controls.test_status,
        "Asking the API for its models…",
    );
    in_background(hwnd, move || {
        let models = openai::list_models(&endpoint, key.as_deref(), Duration::from_secs(15))
            .map_err(|error| error.to_string());
        Event::CloudModels(endpoint, models)
    });
}

fn check_vision(hwnd: HWND) {
    let Some(window) = state(hwnd) else {
        return;
    };
    let endpoint = combo_text(window.controls.endpoint);
    let model = combo_text(window.controls.local_model);
    if endpoint.is_empty() || model.is_empty() || !local::is_on_this_pc(&endpoint) {
        return;
    }
    set_text(
        window.controls.vision,
        "Checking whether this model reads images…",
    );
    in_background(hwnd, move || {
        let reads = local::reads_images(&endpoint, &model);
        Event::Vision(endpoint, model, reads)
    });
}

/// A tiny request that writes a throwaway note, to prove the settings work.
fn test_request() -> NoteRequest {
    let user_text = prompt::user_turn(&prompt::Turn {
        today: "2026-01-01",
        notebook: "Settings test",
        your_note: "Testing FastCapture's settings: can notes be written?",
        ..prompt::Turn::default()
    });
    NoteRequest {
        system: prompt::RULES,
        user_text,
        images: Vec::new(),
        capture: CaptureFacts {
            notebook: PathBuf::new(),
            capture_id: String::new(),
            captured: String::new(),
            raw: PathBuf::new(),
            kinds: Vec::new(),
            attachments: Vec::new(),
            fallback_title: "Settings test".to_owned(),
            links: Vec::new(),
            truncated: Vec::new(),
            candidates: Vec::new(),
            home_notes: Vec::new(),
            home_folders: Vec::new(),
        },
    }
}

fn test(hwnd: HWND) {
    let Some(window) = state(hwnd) else {
        return;
    };
    let mut form = read_form(window);
    // Test only checks the model settings, whatever the notebook.
    if form.notebook.is_empty() {
        form.notebook = r"C:\".to_owned();
    }
    if let Some(problem) = form.problem() {
        set_text(window.controls.test_status, &problem);
        return;
    }
    let saved_key = window
        .key_saved(form.backend)
        .then(|| {
            window
                .key_target(form.backend)
                .and_then(crate::secrets::load)
        })
        .flatten();
    let claude_code_path = window.settings.claude_code_path.clone();
    let copilot_path = window.settings.copilot_path.clone();
    set_text(
        window.controls.test_status,
        "Asking the model for a test note…",
    );
    in_background(hwnd, move || {
        let backend: Result<Box<dyn Backend>, String> = match form.mode {
            Some(Mode::Local) => OpenAi::local(&form.local_endpoint, &form.local_model)
                .map(|backend| Box::new(backend) as Box<dyn Backend>)
                .map_err(|error| error.to_string()),
            _ => {
                let key = if form.new_key.is_empty() {
                    saved_key.unwrap_or_default()
                } else {
                    form.new_key.clone()
                };
                match form.backend {
                    CloudBackend::Claude => Claude::new(&form.claude_model, key)
                        .map(|backend| Box::new(backend) as Box<dyn Backend>),
                    CloudBackend::OpenAi => {
                        OpenAi::cloud(&form.openai_endpoint, &form.openai_model, key)
                            .map(|backend| Box::new(backend) as Box<dyn Backend>)
                    }
                    CloudBackend::ClaudeCode => {
                        ClaudeCode::new(claude_code_path.as_deref(), &form.claude_code_model)
                            .map(|backend| Box::new(backend) as Box<dyn Backend>)
                    }
                    CloudBackend::Copilot => {
                        Copilot::new(copilot_path.as_deref(), &form.copilot_model)
                            .map(|backend| Box::new(backend) as Box<dyn Backend>)
                    }
                }
                .map_err(|error| error.to_string())
            }
        };
        Event::Tested(backend.and_then(|backend| {
            backend
                .complete(&test_request())
                .map(|note| format!("It works: the model wrote “{}”.", note.title.trim()))
                .map_err(|error| error.to_string())
        }))
    });
}

#[cfg(test)]
thread_local! {
    /// What tests answer to the first-use screen for Cloud mode.
    static CONSENT: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

fn confirm_cloud(hwnd: HWND, form: &Form) -> bool {
    #[cfg(test)]
    {
        let _ = (hwnd, form);
        CONSENT.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        let (title, text) = form.consent();
        let question = super::popup::Question {
            title: &title,
            text: &text,
            buttons: &["Yes, send them", "No"],
            default: 1,
            cancel: 1,
        };
        super::popup::ask(hwnd, &question) == 0
    }
}

/// Save: checks the form, asks the first time Cloud is chosen, saves the key and the file, and
/// closes. A problem is shown above the buttons instead.
fn save(hwnd: HWND) {
    let Some(window) = state(hwnd) else {
        return;
    };
    let form = read_form(window);
    if let Some(problem) = form.problem() {
        set_text(window.controls.problem, &problem);
        return;
    }
    let consent = form.needs_cloud_consent(&window.settings);
    if consent && !confirm_cloud(hwnd, &form) {
        set_text(
            window.controls.problem,
            "Cloud mode wasn't confirmed, so nothing was saved.",
        );
        return;
    }
    let Some(window) = state(hwnd) else {
        return;
    };
    if !form.new_key.is_empty()
        && let Some(target) = window.key_target(form.backend)
        && let Err(error) = crate::secrets::save(target, &form.new_key)
    {
        set_text(
            window.controls.problem,
            &format!("The key couldn't be saved: {error}"),
        );
        return;
    }
    let mut file = SettingsFile::load(&window.open.settings);
    form.apply(&mut file, consent);
    if let Err(error) = file.save(&window.open.settings) {
        set_text(
            window.controls.problem,
            &format!("The settings couldn't be saved: {error}"),
        );
        return;
    }
    (window.open.saved)();
    unsafe { DestroyWindow(hwnd) };
}

/// The folder picker for the notebook.
fn browse(hwnd: HWND) {
    let Some(window) = state(hwnd) else {
        return;
    };
    if let Some(folder) = crate::platform::shell::pick_folder(hwnd, "Choose the notebook folder") {
        set_text(window.controls.notebook, &folder.display().to_string());
    }
}

fn on_event(hwnd: HWND, event: Event) {
    let Some(window) = state(hwnd) else {
        return;
    };
    let controls = &window.controls;
    match event {
        Event::Servers(found) => {
            let endpoints: Vec<String> =
                found.iter().map(|server| server.endpoint.clone()).collect();
            let current = combo_text(controls.endpoint);
            combo_fill(controls.endpoint, &endpoints);
            let message = if found.is_empty() {
                "No model server found on this PC. Start Ollama, LM Studio or Foundry Local, then Find servers.".to_owned()
            } else {
                format!(
                    "Found {}.",
                    found
                        .iter()
                        .map(|server| server.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            set_text(controls.vision, &message);
            if current.is_empty()
                && let Some(first) = found.first()
            {
                set_text(controls.endpoint, &first.endpoint);
                combo_fill(controls.local_model, &first.models);
                if let Some(model) = first.models.first() {
                    set_text(controls.local_model, model);
                    check_vision(hwnd);
                }
            } else {
                set_text(controls.endpoint, &current);
            }
        }
        Event::Models(endpoint, models) => {
            if combo_text(controls.endpoint).trim() != endpoint {
                return;
            }
            match models {
                Ok(models) => {
                    let current = combo_text(controls.local_model);
                    combo_fill(controls.local_model, &models);
                    set_text(controls.local_model, &current);
                    if models.is_empty() {
                        set_text(
                            controls.vision,
                            "The server lists no models. Download one with it first.",
                        );
                    } else {
                        check_vision(hwnd);
                    }
                }
                Err(error) => set_text(
                    controls.vision,
                    &format!("No answer from {endpoint}: {error}"),
                ),
            }
        }
        Event::CloudModels(endpoint, models) => {
            if text_of(controls.openai_address).trim() != endpoint {
                return;
            }
            match models {
                Ok(models) => {
                    let current = combo_text(controls.openai_model);
                    combo_fill(controls.openai_model, &models);
                    set_text(controls.openai_model, &current);
                    set_text(
                        controls.test_status,
                        &match models.len() {
                            0 => "The API lists no models; type the model's name.".to_owned(),
                            1 => "The API lists 1 model.".to_owned(),
                            count => format!("The API lists {count} models."),
                        },
                    );
                }
                Err(error) => set_text(controls.test_status, &error),
            }
        }
        Event::Vision(endpoint, model, reads) => {
            if combo_text(controls.endpoint).trim() != endpoint
                || combo_text(controls.local_model).trim() != model
            {
                return;
            }
            set_text(
                controls.vision,
                match reads {
                    Some(true) => "This model reads images: screenshots are described in notes.",
                    Some(false) => {
                        "This model doesn't read images: screenshots are attached to notes unread."
                    }
                    None => "Couldn't tell whether this model reads images.",
                },
            );
        }
        Event::ClaudeCode(status) => set_text(controls.claude_code_status, &status),
        Event::Copilot(status) => {
            set_text(controls.copilot_status, &status.line);
            if !status.models.is_empty() {
                let typed = combo_text(controls.copilot_model);
                combo_fill(controls.copilot_model, &status.models);
                set_text(controls.copilot_model, &typed);
            }
        }
        Event::Tested(result) => set_text(
            controls.test_status,
            &match result {
                Ok(message) => message,
                Err(error) => error,
            },
        ),
    }
}

unsafe extern "system" fn settings_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } == 0 {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    if let Some(window) = state(hwnd)
        && let Some(answer) = window.frame.handle(hwnd, message, wparam, lparam)
    {
        return answer;
    }
    match message {
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as i32;
            let code = ((wparam >> 16) & 0xFFFF) as u32;
            match (id, code) {
                (IDOK, _) => save(hwnd),
                (IDCANCEL, _) => unsafe {
                    DestroyWindow(hwnd);
                },
                (ID_BROWSE, _) => browse(hwnd),
                // The theme is shown at once; it is saved with the rest.
                (ID_THEME_SYSTEM | ID_THEME_LIGHT | ID_THEME_DARK, _) => {
                    if let Some((_, chosen)) = THEMES.iter().find(|(theme, _)| *theme == id)
                        && theme::set_preference(*chosen)
                    {
                        theme::tell_windows();
                    }
                }
                (ID_LOCAL | ID_CLOUD, _) => {
                    enable_for_mode(hwnd);
                    if let Some(window) = state(hwnd) {
                        set_text(window.controls.problem, "");
                    }
                }
                (ID_FIND, _) => find_servers(hwnd),
                (ID_ENDPOINT, CBN_SELCHANGE | CBN_KILLFOCUS) => {
                    if let Some(window) = state(hwnd) {
                        let endpoint = combo_text(window.controls.endpoint).trim().to_owned();
                        if !endpoint.is_empty() {
                            list_models(hwnd, endpoint);
                        }
                    }
                }
                (ID_LOCAL_MODEL, CBN_SELCHANGE | CBN_KILLFOCUS) => check_vision(hwnd),
                (ID_PROVIDER, CBN_SELCHANGE) => {
                    // A key typed for one provider isn't saved as the other's.
                    if let Some(window) = state(hwnd) {
                        set_text(window.controls.key, "");
                        set_text(window.controls.problem, "");
                    }
                    enable_for_mode(hwnd);
                }
                (ID_LIST_MODELS, _) => list_cloud_models(hwnd),
                (ID_TEST, _) => test(hwnd),
                (ID_OPEN_FILE, _) => {
                    if let Some(window) = state(hwnd) {
                        (window.open.open_file)();
                    }
                }
                _ => {}
            }
            0
        }
        // Fields and labels in the theme; problems in the error color, statuses muted.
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
            let answer = controls::colors(message, wparam, lparam, Surface::Content).unwrap_or(0);
            if message == WM_CTLCOLORSTATIC
                && let Some(window) = state(hwnd)
            {
                let palette = theme::current();
                let child = lparam as HWND;
                let color = if child == window.controls.problem {
                    Some(palette.error_foreground)
                } else if window.controls.muted.contains(&child) {
                    Some(palette.muted_foreground)
                } else {
                    None
                };
                if let Some(color) = color {
                    unsafe { SetTextColor(wparam as HDC, color) };
                }
            }
            answer
        }
        WM_DPICHANGED => {
            let dpi = u32::from((wparam & 0xFFFF) as u16).max(96);
            change_dpi(hwnd, dpi, unsafe { &*(lparam as *const RECT) });
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let palette = theme::current();
            if let Some(window) = state(hwnd) {
                paint::buffered(hwnd, |dc, area| {
                    paint::fill(dc, area, palette.content_background);
                    window.frame.paint(dc, area.right, &palette, false);
                });
            }
            0
        }
        WM_SETTINGS_RESULT => {
            if lparam != 0 {
                let event = unsafe { Box::from_raw(lparam as *mut Event) };
                on_event(hwnd, *event);
            }
            0
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_NCDESTROY => {
            let pointer =
                unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut SettingsWindow;
            if !pointer.is_null() {
                let window = unsafe { Box::from_raw(pointer) };
                unsafe { DeleteObject(window.font) };
                // A theme chosen but not saved was only shown: the saved one comes back.
                let (saved, _) = SettingsFile::load(&window.open.settings).settings();
                if theme::set_preference(saved.theme) {
                    theme::tell_windows();
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        _ if message == theme::changed_message() => {
            frame::apply_theme(hwnd, &theme::current());
            controls::retheme(hwnd);
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;

    /// A scratch folder, and test credential names for the Claude and OpenAI-compatible keys.
    struct Temp(PathBuf, String, String);

    impl Temp {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "fastcapture-settings-window-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let key = |backend: &str| {
                crate::secrets::target(&format!(
                    "test-window-{name}-{backend}-{}",
                    std::process::id()
                ))
            };
            Self(dir, key("claude"), key("openai"))
        }

        fn open(&self, saved: Rc<Cell<bool>>) -> HWND {
            open(Open {
                settings: self.0.join("fastcapture.ini"),
                claude_key: self.1.clone(),
                openai_key: self.2.clone(),
                usage: Some(self.0.join(usage::FILE)),
                saved: Box::new(move || saved.set(true)),
                open_file: Box::new(|| {}),
            })
            .unwrap()
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            crate::secrets::delete(&self.1);
            crate::secrets::delete(&self.2);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn command(hwnd: HWND, id: i32) {
        unsafe { SendMessageW(hwnd, WM_COMMAND, id as usize, 0) };
    }

    fn notify(hwnd: HWND, id: i32, code: u32) {
        unsafe { SendMessageW(hwnd, WM_COMMAND, ((code as usize) << 16) | id as usize, 0) };
    }

    fn visible(control: HWND) -> bool {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(control) != 0 }
    }

    /// Handles the window's messages until `done`, for up to ten seconds.
    fn pump_until(done: impl Fn() -> bool) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, PM_REMOVE, PeekMessageW,
        };
        let until = std::time::Instant::now() + Duration::from_secs(10);
        let mut message: MSG = unsafe { std::mem::zeroed() };
        while std::time::Instant::now() < until {
            while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0
            {
                unsafe { DispatchMessageW(&message) };
            }
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        done()
    }

    #[test]
    fn copilot_shows_who_is_signed_in_lists_the_plan_s_models_and_is_saved_without_a_key() {
        let temp = Temp::new("copilot");
        let fakes = crate::model::claude_code::cli::tests::Temp::new("settings-window-copilot");
        let fake = crate::model::copilot::cli::tests::fake(
            &fakes,
            &[
                crate::model::copilot::cli::tests::connects(),
                r#"auth.getStatus	"result":{"isAuthenticated":true,"login":"someone"}"#.to_owned(),
                r#"models.list	"result":{"models":[{"id":"auto"},{"id":"gpt-5.4"}]}"#.to_owned(),
                r#"account.getQuota	"result":{"quotaSnapshots":{"chat":{"isUnlimitedEntitlement":false,"entitlementRequests":200,"usedRequests":3}}}"#.to_owned(),
            ],
        );
        std::fs::write(
            temp.0.join("fastcapture.ini"),
            format!(
                "notebook={}\r\nmode=cloud\r\ncopilot_path={}\r\n",
                temp.0.display(),
                fake.display()
            ),
        )
        .unwrap();
        let saved = Rc::new(Cell::new(false));
        let hwnd = temp.open(saved.clone());
        let window = state(hwnd).unwrap();
        unsafe { SendMessageW(window.controls.provider, CB_SETCURSEL, 3, 0) };
        notify(hwnd, ID_PROVIDER, CBN_SELCHANGE);
        let window = state(hwnd).unwrap();
        assert!(visible(window.controls.copilot_model));
        assert!(!visible(window.controls.key) && !visible(window.controls.claude_code_model));
        assert_eq!(text_of(window.controls.copilot_model), "auto");
        let status = window.controls.copilot_status;
        assert!(pump_until(
            || text_of(status).starts_with("Copilot CLI 1.0.100")
        ));
        assert_eq!(
            text_of(status),
            format!(
                "Copilot CLI 1.0.100, signed in as someone; 197 of 200 chat requests left this month. At {}.",
                fake.display()
            )
        );
        let count = unsafe {
            SendMessageW(
                window.controls.copilot_model,
                windows_sys::Win32::UI::WindowsAndMessaging::CB_GETCOUNT,
                0,
                0,
            )
        };
        assert_eq!(count, 2, "the plan's models");
        assert_eq!(
            text_of(window.controls.copilot_model),
            "auto",
            "the choice is kept"
        );

        set_text(window.controls.copilot_model, "gpt-5.4");
        command(hwnd, IDOK);
        assert!(saved.get());
        let text = std::fs::read_to_string(temp.0.join("fastcapture.ini")).unwrap();
        let (settings, _) = crate::config::settings::parse(&text);
        assert_eq!(settings.backend, CloudBackend::Copilot);
        assert_eq!(settings.copilot_model, "gpt-5.4");
        assert!(settings.copilot_accepted && !settings.cloud_accepted);
        assert_eq!(settings.copilot_path, Some(fake.clone()), "kept as it was");
        assert_eq!(crate::secrets::load(&temp.1), None);
        assert_eq!(crate::secrets::load(&temp.2), None);
    }

    #[test]
    fn claude_code_shows_what_was_found_and_is_saved_without_a_key() {
        let temp = Temp::new("claude-code");
        let fakes = crate::model::claude_code::cli::tests::Temp::new("settings-window");
        let fake = fakes.script(
            "claude.cmd",
            &[
                r#"if "%~1"=="--version" (echo 2.1.303 ^(Claude Code^)& exit /b 0)"#,
                r#"if "%~1"=="auth" (echo {"loggedIn": false}& exit /b 1)"#,
            ],
        );
        std::fs::write(
            temp.0.join("fastcapture.ini"),
            format!(
                "notebook={}\r\nmode=cloud\r\nclaude_code_path={}\r\n",
                temp.0.display(),
                fake.display()
            ),
        )
        .unwrap();
        let saved = Rc::new(Cell::new(false));
        let hwnd = temp.open(saved.clone());
        let window = state(hwnd).unwrap();
        unsafe { SendMessageW(window.controls.provider, CB_SETCURSEL, 2, 0) };
        notify(hwnd, ID_PROVIDER, CBN_SELCHANGE);
        let window = state(hwnd).unwrap();
        assert!(visible(window.controls.claude_code_model));
        assert!(!visible(window.controls.key) && !visible(window.controls.cloud_model));
        let status = window.controls.claude_code_status;
        assert!(pump_until(
            || text_of(status).starts_with("Claude Code 2.1.303")
        ));
        assert_eq!(
            text_of(status),
            format!(
                "Claude Code 2.1.303 at {}, not signed in: run claude in a terminal, then /login.",
                fake.display()
            )
        );

        set_text(window.controls.claude_code_model, "haiku");
        command(hwnd, IDOK);
        assert!(saved.get());
        let text = std::fs::read_to_string(temp.0.join("fastcapture.ini")).unwrap();
        let (settings, _) = crate::config::settings::parse(&text);
        assert_eq!(settings.backend, CloudBackend::ClaudeCode);
        assert_eq!(settings.claude_code_model, "haiku");
        assert!(settings.claude_code_accepted && !settings.cloud_accepted);
        assert_eq!(
            settings.claude_code_path,
            Some(fake.clone()),
            "kept as it was"
        );
        assert_eq!(crate::secrets::load(&temp.1), None);
        assert_eq!(crate::secrets::load(&temp.2), None);
    }

    #[test]
    fn an_openai_compatible_api_is_saved_with_its_own_key_and_the_tokens_used_are_shown() {
        let temp = Temp::new("openai");
        std::fs::write(
            temp.0.join("fastcapture.ini"),
            format!(
                "notebook={}\r\nmode=cloud\r\ncloud_accepted=true\r\ntoken_cap=1k\r\n",
                temp.0.display()
            ),
        )
        .unwrap();
        usage::record(
            &temp.0.join(usage::FILE),
            &usage::today(),
            "cloud/claude-haiku-5-5",
            crate::model::Usage {
                input: 1200,
                cache_read: 300,
                ..Default::default()
            },
        )
        .unwrap();
        let saved = Rc::new(Cell::new(false));
        let hwnd = temp.open(saved.clone());
        let window = state(hwnd).unwrap();
        assert_eq!(
            text_of(window.controls.usage),
            "Tokens used: 1,500 today, 1,500 this month, as the cloud providers reported them. The cap is reached."
        );
        assert_eq!(text_of(window.controls.cap), "1k");
        assert!(visible(window.controls.cloud_model) && !visible(window.controls.openai_address));

        unsafe { SendMessageW(window.controls.provider, CB_SETCURSEL, 1, 0) };
        set_text(window.controls.key, "typed for Claude");
        notify(hwnd, ID_PROVIDER, CBN_SELCHANGE);
        let window = state(hwnd).unwrap();
        assert!(!visible(window.controls.cloud_model) && visible(window.controls.openai_address));
        assert_eq!(
            text_of(window.controls.key),
            "",
            "the other provider's key is cleared"
        );
        command(hwnd, IDOK);
        assert!(
            text_of(state(hwnd).unwrap().controls.problem).starts_with("Enter the API's address")
        );

        let window = state(hwnd).unwrap();
        set_text(
            window.controls.openai_address,
            "https://api.example.test/v1",
        );
        set_text(window.controls.openai_model, "gpt-test");
        set_text(window.controls.key, "sk-openai-window-test");
        set_text(window.controls.cap, "2M");
        command(hwnd, IDOK);
        assert!(
            saved.get(),
            "{}",
            text_of(state(hwnd).map_or(hwnd, |w| w.controls.problem))
        );

        let text = std::fs::read_to_string(temp.0.join("fastcapture.ini")).unwrap();
        assert!(!text.contains("sk-openai"), "{text}");
        let (settings, _) = crate::config::settings::parse(&text);
        assert_eq!(settings.backend, CloudBackend::OpenAi);
        assert_eq!(
            settings.openai_endpoint.as_deref(),
            Some("https://api.example.test/v1")
        );
        assert_eq!(settings.openai_model.as_deref(), Some("gpt-test"));
        assert_eq!(
            settings.openai_accepted.as_deref(),
            Some("api.example.test")
        );
        assert_eq!(settings.token_cap, Some(2_000_000));
        assert_eq!(
            crate::secrets::load(&temp.2).as_deref(),
            Some("sk-openai-window-test")
        );
        assert_eq!(crate::secrets::load(&temp.1), None, "no Claude key saved");
    }

    #[test]
    fn cloud_settings_are_saved_with_the_key_in_credential_manager() {
        let temp = Temp::new("cloud");
        std::fs::write(
            temp.0.join("fastcapture.ini"),
            format!(
                "notebook={}\r\nlocal_endpoint=http://localhost:11434\r\n",
                temp.0.display()
            ),
        )
        .unwrap();
        let saved = Rc::new(Cell::new(false));
        let hwnd = temp.open(saved.clone());
        let window = state(hwnd).unwrap();
        assert_eq!(
            text_of(window.controls.notebook),
            temp.0.display().to_string()
        );
        assert!(!checked(window.controls.local) && !checked(window.controls.cloud));

        // No mode yet: Save says so and keeps the window.
        command(hwnd, IDOK);
        assert_eq!(
            text_of(state(hwnd).unwrap().controls.problem),
            "Choose Local or Cloud."
        );
        assert!(!saved.get());

        let window = state(hwnd).unwrap();
        set_checked(window.controls.cloud, true);
        command(hwnd, ID_CLOUD);
        command(hwnd, IDOK);
        assert_eq!(
            text_of(state(hwnd).unwrap().controls.problem),
            "Enter your Claude API key."
        );

        let window = state(hwnd).unwrap();
        set_text(window.controls.key, "sk-ant-window-test");
        unsafe { SendMessageW(window.controls.cloud_model, CB_SETCURSEL, 1, 0) };
        CONSENT.with(|consent| consent.set(false));
        command(hwnd, IDOK);
        assert!(!saved.get(), "declined the first-use screen");
        CONSENT.with(|consent| consent.set(true));
        command(hwnd, IDOK);
        assert!(saved.get());
        assert_eq!(unsafe { IsWindow(hwnd) }, 0, "closed");

        let text = std::fs::read_to_string(temp.0.join("fastcapture.ini")).unwrap();
        assert!(!text.contains("sk-ant"), "{text}");
        let (settings, _) = crate::config::settings::parse(&text);
        assert_eq!(settings.mode, Some(Mode::Cloud));
        assert_eq!(settings.claude_model, claude::MODELS[1].0);
        assert!(settings.cloud_accepted);
        assert_eq!(
            crate::secrets::load(&temp.1).as_deref(),
            Some("sk-ant-window-test")
        );
    }

    #[test]
    fn a_theme_is_shown_when_chosen_put_back_on_cancel_and_kept_on_save() {
        use crate::ui::theme::{DARK, LIGHT};
        theme::set_system_for_test(theme::System::default());
        theme::set_preference(ThemePreference::System);
        let temp = Temp::new("theme");
        let saved = Rc::new(Cell::new(false));
        let hwnd = temp.open(saved.clone());
        let window = state(hwnd).unwrap();
        assert!(checked(window.controls.themes[0]), "System");
        set_checked(window.controls.themes[0], false);
        set_checked(window.controls.themes[2], true);
        command(hwnd, ID_THEME_DARK);
        assert_eq!(theme::current(), DARK, "shown at once");
        command(hwnd, IDCANCEL);
        assert_eq!(theme::current(), LIGHT, "put back");

        std::fs::write(
            temp.0.join("fastcapture.ini"),
            format!(
                "notebook={}\r\nmode=cloud\r\nbackend=claude_code\r\nclaude_code_accepted=true\r\n",
                temp.0.display()
            ),
        )
        .unwrap();
        let hwnd = temp.open(saved.clone());
        let window = state(hwnd).unwrap();
        set_checked(window.controls.themes[0], false);
        set_checked(window.controls.themes[2], true);
        command(hwnd, ID_THEME_DARK);
        unsafe { SendMessageW(window.controls.provider, CB_SETCURSEL, 2, 0) };
        notify(hwnd, ID_PROVIDER, CBN_SELCHANGE);
        command(hwnd, IDOK);
        assert!(saved.get());
        let text = std::fs::read_to_string(temp.0.join("fastcapture.ini")).unwrap();
        assert_eq!(
            crate::config::settings::parse(&text).0.theme,
            ThemePreference::Dark
        );
        assert_eq!(theme::current(), DARK, "kept");
        theme::set_preference(ThemePreference::System);
    }

    #[test]
    fn hotkeys_are_shown_checked_and_saved() {
        let temp = Temp::new("hotkeys");
        std::fs::write(
            temp.0.join("fastcapture.ini"),
            format!(
                "notebook={}\r\nmode=local\r\nlocal_endpoint=http://localhost:11434\r\n\
                 local_model=gemma3\r\n",
                temp.0.display()
            ),
        )
        .unwrap();
        let saved = Rc::new(Cell::new(false));
        let hwnd = temp.open(saved.clone());
        let window = state(hwnd).unwrap();
        assert_eq!(text_of(window.controls.capture_hotkey), "Ctrl+Alt+C");
        assert_eq!(text_of(window.controls.clipboard_hotkey), "");

        set_text(window.controls.capture_hotkey, "Ctrl+Alt");
        command(hwnd, IDOK);
        assert!(
            text_of(state(hwnd).unwrap().controls.problem)
                .starts_with("\"Ctrl+Alt\" isn't a capture hotkey.")
        );
        assert!(!saved.get());

        let window = state(hwnd).unwrap();
        set_text(window.controls.capture_hotkey, "ctrl+shift+space");
        set_text(window.controls.clipboard_hotkey, "Ctrl+Shift+V");
        command(hwnd, IDOK);
        assert!(saved.get());
        let text = std::fs::read_to_string(temp.0.join("fastcapture.ini")).unwrap();
        let settings = crate::config::settings::parse(&text).0;
        assert_eq!(
            settings.capture_hotkey.unwrap().to_string(),
            "Ctrl+Shift+Space"
        );
        assert_eq!(
            settings.clipboard_hotkey.unwrap().to_string(),
            "Ctrl+Shift+V"
        );
    }

    #[test]
    fn only_the_chosen_mode_s_fields_are_shown_and_the_window_fits_them() {
        let temp = Temp::new("modes");
        let hwnd = temp.open(Rc::new(Cell::new(false)));
        let height = || {
            let mut place: RECT = unsafe { std::mem::zeroed() };
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut place) };
            place.bottom - place.top
        };
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let window = state(hwnd).unwrap();
        let (endpoint, provider, cap, sign_in) = (
            window.controls.endpoint,
            window.controls.provider,
            window.controls.cap,
            window.controls.sign_in,
        );

        set_checked(window.controls.local, false);
        set_checked(window.controls.cloud, false);
        command(hwnd, ID_LOCAL);
        assert!(!visible(endpoint) && !visible(provider) && !visible(cap));
        assert_eq!(height(), scale(HEIGHT - lift(None), dpi));

        set_checked(state(hwnd).unwrap().controls.local, true);
        command(hwnd, ID_LOCAL);
        assert!(visible(endpoint) && !visible(provider) && !visible(cap));
        assert!(!visible(state(hwnd).unwrap().controls.claude_address));
        assert_eq!(height(), scale(HEIGHT - lift(Some(Mode::Local)), dpi));
        let local_sign_in = place_of(hwnd, sign_in).top;

        let window = state(hwnd).unwrap();
        set_checked(window.controls.local, false);
        set_checked(window.controls.cloud, true);
        command(hwnd, ID_CLOUD);
        assert!(!visible(endpoint) && visible(provider) && visible(cap));
        assert_eq!(height(), scale(HEIGHT, dpi));
        assert_eq!(
            place_of(hwnd, sign_in).top - local_sign_in,
            scale(382 + TITLE_STRIP + THEME_ROW, dpi)
                - scale(382 - lift(Some(Mode::Local)) + TITLE_STRIP + THEME_ROW, dpi),
            "the rows below moved down"
        );
        command(hwnd, IDCANCEL);
    }

    /// Where `control` sits in its window's client area.
    fn place_of(hwnd: HWND, control: HWND) -> RECT {
        use windows_sys::Win32::Graphics::Gdi::MapWindowPoints;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
        let mut place: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetWindowRect(control, &mut place);
            MapWindowPoints(std::ptr::null_mut(), hwnd, &mut place as *mut RECT as _, 2);
        }
        place
    }

    #[test]
    fn moved_to_a_monitor_at_another_scale_it_is_laid_out_again() {
        let temp = Temp::new("dpi");
        let hwnd = temp.open(Rc::new(Cell::new(false)));
        let suggested = RECT {
            left: 40,
            top: 30,
            right: 40 + scale(WIDTH, 144),
            bottom: 30 + scale(HEIGHT, 144),
        };
        unsafe {
            SendMessageW(
                hwnd,
                WM_DPICHANGED,
                (144 << 16) | 144,
                &suggested as *const RECT as isize,
            )
        };
        let window = state(hwnd).unwrap();
        assert_eq!(window.frame.height(), scale(TITLE_STRIP, 144));
        let notebook = place_of(hwnd, window.controls.notebook);
        assert_eq!(
            (notebook.left, notebook.top),
            (scale(112, 144), scale(16 + TITLE_STRIP + THEME_ROW, 144))
        );
        assert_eq!(notebook.right - notebook.left, scale(320, 144));
        let mut place: RECT = unsafe { std::mem::zeroed() };
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut place) };
        assert_eq!(place.right - place.left, scale(WIDTH, 144));
        command(hwnd, IDCANCEL);
    }

    #[test]
    fn local_mode_refuses_a_server_elsewhere_and_cancel_saves_nothing() {
        let temp = Temp::new("local");
        let saved = Rc::new(Cell::new(false));
        let hwnd = temp.open(saved.clone());
        let window = state(hwnd).unwrap();
        set_text(window.controls.notebook, &temp.0.display().to_string());
        set_checked(window.controls.local, true);
        command(hwnd, ID_LOCAL);
        set_text(window.controls.endpoint, "http://192.168.1.5:11434");
        set_text(window.controls.local_model, "gemma3");
        command(hwnd, IDOK);
        assert!(text_of(state(hwnd).unwrap().controls.problem).contains("isn't on this PC"));
        command(hwnd, IDCANCEL);
        assert!(!saved.get());
        assert!(!temp.0.join("fastcapture.ini").exists());
    }
}
