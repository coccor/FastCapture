//! The tray app (spec §5.1, §12): a hidden window that owns the tray icon and the hotkeys, a
//! thread answering `fastcapture add` over the pipe, a watch on the settings folder, and a
//! start-up pass over the queue. Everything that touches windows runs on this thread.

use crate::add::{self, Context};
use crate::capture::file::FileError;
use crate::capture::submit::submit;
use crate::config::settings::{Settings, SettingsFile};
use crate::ipc::pipe;
use crate::platform::tray::{self, MenuItem, Tray};
use crate::platform::window::SendHwnd;
use crate::platform::{hotkey, instance, shell, startup};
use crate::queue::{State, Store};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, MSG,
    PostMessageW, PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, TranslateMessage,
    WM_APP, WM_DESTROY, WM_HOTKEY, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};

const WM_APP_TRAY: u32 = WM_APP + 1;
/// The queue changed: refresh the tooltip.
const WM_APP_QUEUE: u32 = WM_APP + 2;
/// `fastcapture.ini` changed: read it again.
const WM_APP_SETTINGS: u32 = WM_APP + 3;

const HOTKEY_CAPTURE: i32 = 1;
const HOTKEY_CLIPBOARD: i32 = 2;

const MENU_CAPTURE: u32 = 1;
const MENU_CLIPBOARD: u32 = 2;
const MENU_REVIEW: u32 = 3;
const MENU_SETTINGS: u32 = 4;
const MENU_FOLDER: u32 = 5;
const MENU_QUIT: u32 = 6;

struct App {
    window: HWND,
    tray: Tray,
    context: Context,
    settings: Settings,
    notebook: Option<PathBuf>,
    /// The hotkey texts whose registration failure was already reported, so a settings save
    /// doesn't repeat the notice.
    reported: Vec<String>,
    taskbar_created: u32,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app, unless it isn't there (being torn down) or already in use (a nested
/// message while a menu is open): those messages are dropped.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|app| {
        app.try_borrow_mut()
            .ok()
            .and_then(|mut app| app.as_mut().map(f))
    })
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Runs the tray app until Quit and returns the exit code. A second start in the same session
/// exits at once.
pub fn run() -> i32 {
    let Some(_instance) = instance::claim(instance::TRAY) else {
        crate::log::line("tray: already running in this session");
        return 0;
    };
    crate::log::line(&format!(
        "FastCapture {} started",
        env!("CARGO_PKG_VERSION")
    ));
    let Some(window) = create_window() else {
        crate::log::line("tray: could not create the window");
        return 1;
    };
    let context = Context::from_environment();
    let taskbar_created = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
    let app = App {
        window,
        tray: Tray::add(window, WM_APP_TRAY, "FastCapture"),
        settings: Settings::default(),
        notebook: None,
        reported: Vec::new(),
        context,
        taskbar_created,
    };
    APP.with(|slot| *slot.borrow_mut() = Some(app));
    with_app(App::apply_settings);

    let stop = Arc::new(AtomicBool::new(false));
    start_pipe(window, stop.clone());
    start_settings_watch(window);
    start_queue_recovery(window);

    let mut message: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    stop.store(true, Ordering::SeqCst);
    pipe::wake();
    crate::log::line("FastCapture stopped");
    0
}

fn create_window() -> Option<HWND> {
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = wide("FastCaptureTray");
    let mut description: WNDCLASSEXW = unsafe { std::mem::zeroed() };
    description.cbSize = size_of::<WNDCLASSEXW>() as u32;
    description.lpfnWndProc = Some(window_proc);
    description.hInstance = instance;
    description.lpszClassName = class.as_ptr();
    if unsafe { RegisterClassExW(&description) } == 0 {
        return None;
    }
    // A hidden top-level window rather than a message-only one: only top-level windows get the
    // `TaskbarCreated` broadcast that says the tray icon must be added again.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            wide("FastCapture").as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    (!window.is_null()).then_some(window)
}

/// Answers `fastcapture add` on its own thread, then tells the window the queue changed.
fn start_pipe(window: HWND, stop: Arc<AtomicBool>) {
    let window = SendHwnd::new(window);
    std::thread::spawn(move || {
        let mut server = match pipe::Server::create() {
            Ok(server) => server,
            Err(error) => {
                crate::log::line(&format!("pipe: {error}"));
                return;
            }
        };
        let context = Context::from_environment();
        server.serve(&stop, |request| {
            let reply = add::handle(request, &context);
            crate::log::line(match reply {
                crate::ipc::Reply::Filed(_) => "pipe: filed a capture from fastcapture add",
                crate::ipc::Reply::Failed { .. } => "pipe: refused a capture from fastcapture add",
            });
            unsafe { PostMessageW(window.get(), WM_APP_QUEUE, 0, 0) };
            reply
        });
    });
}

fn start_settings_watch(window: HWND) {
    let Some(folder) = crate::paths::roaming() else {
        return;
    };
    if let Err(error) = std::fs::create_dir_all(&folder) {
        crate::log::line(&format!("settings: {}: {error}", folder.display()));
        return;
    }
    let window = SendHwnd::new(window);
    if let Err(error) = crate::platform::watch::watch_folder(&folder, move || unsafe {
        PostMessageW(window.get(), WM_APP_SETTINGS, 0, 0);
    }) {
        crate::log::line(&format!("settings: not watched: {error}"));
    }
}

/// After a restart: interrupted jobs are queued again, and raw sources with no job get one.
fn start_queue_recovery(window: HWND) {
    let Some(queue) = with_app(|app| app.context.queue.clone()).flatten() else {
        return;
    };
    let notebook = with_app(|app| app.notebook.clone()).flatten();
    let window = SendHwnd::new(window);
    std::thread::spawn(move || {
        let store = Store::new(queue);
        let recovered = store.recover();
        let added = notebook
            .map(|root| {
                store
                    .enqueue_orphans(&crate::notebook::Conventions::load(&root))
                    .len()
            })
            .unwrap_or(0);
        if recovered + added > 0 {
            crate::log::line(&format!(
                "queue: {recovered} interrupted, {added} raw sources without a job"
            ));
        }
        unsafe { PostMessageW(window.get(), WM_APP_QUEUE, 0, 0) };
    });
}

impl App {
    /// Reads `fastcapture.ini` and applies it: the notebook, hotkeys and the sign-in entry.
    fn apply_settings(&mut self) {
        let (settings, warnings) = self
            .context
            .settings
            .as_deref()
            .map(|path| SettingsFile::load(path).settings())
            .unwrap_or_default();
        for warning in &warnings {
            crate::log::line(&format!(
                "fastcapture.ini line {}: {}",
                warning.line, warning.message
            ));
        }
        self.notebook = settings
            .notebook
            .clone()
            .or_else(|| self.context.notebook());

        for id in [HOTKEY_CAPTURE, HOTKEY_CLIPBOARD] {
            hotkey::unregister(self.window, id);
        }
        let mut failed = Vec::new();
        for (id, key, stroke) in [
            (HOTKEY_CAPTURE, "capture_hotkey", settings.capture_hotkey),
            (
                HOTKEY_CLIPBOARD,
                "clipboard_hotkey",
                settings.clipboard_hotkey,
            ),
        ] {
            if let Some(stroke) = stroke
                && let Err(error) = hotkey::register(self.window, id, stroke)
            {
                crate::log::line(&format!("hotkey {stroke}: {error}"));
                failed.push((key, stroke.to_string()));
            }
        }
        for (key, text) in failed {
            if !self.reported.contains(&text) {
                self.tray.notify(
                    "Hotkey not available",
                    &format!("{text} is in use by another app. Change {key} in fastcapture.ini (tray menu › Settings)."),
                    true,
                );
                self.reported.push(text);
            }
        }

        let wanted = settings
            .start_at_sign_in
            .then(startup::this_command)
            .and_then(Result::ok);
        if startup::get(startup::VALUE) != wanted
            && let Err(error) = startup::set(startup::VALUE, wanted.as_deref())
        {
            crate::log::line(&format!("start at sign-in: {error}"));
        }

        self.settings = settings;
        self.refresh_tip();
    }

    fn queue_counts(&self) -> (usize, usize) {
        let Some(queue) = &self.context.queue else {
            return (0, 0);
        };
        let jobs = Store::new(queue.clone()).list();
        let waiting = jobs
            .iter()
            .filter(|job| matches!(job.state, State::Queued | State::Working))
            .count();
        let failed = jobs.iter().filter(|job| job.state == State::Failed).count();
        (waiting, failed)
    }

    fn refresh_tip(&mut self) {
        let (waiting, failed) = self.queue_counts();
        let mut tip = match &self.notebook {
            Some(notebook) => format!(
                "FastCapture → {}",
                notebook.file_name().unwrap_or_default().to_string_lossy()
            ),
            None => "FastCapture: no notebook set".to_owned(),
        };
        tip.push_str(&match waiting {
            0 => "\nNothing waiting".to_owned(),
            1 => "\n1 capture queued".to_owned(),
            n => format!("\n{n} captures queued"),
        });
        if failed > 0 {
            tip.push_str(&format!(", {failed} failed"));
        }
        self.tray.set_tip(&tip);
    }

    /// Capture clipboard: files it as it is, then says what happened.
    fn capture_clipboard(&mut self, source_window: Option<String>) {
        let result =
            super::clipboard_capture::from_clipboard(self.window).and_then(|mut capture| {
                capture.source_window = source_window;
                let root = self.notebook.clone().ok_or_else(|| {
                    "No notebook is set. Open Settings from the tray menu.".to_owned()
                })?;
                submit(&root, &capture, self.context.queue.as_deref(), false).map_err(|error| {
                    match error {
                        FileError::TooLarge { .. } => {
                            format!("{error}. Use the capture box to copy it anyway.")
                        }
                        FileError::Failed(message) => message,
                    }
                })
            });
        match result {
            Ok(raw) => {
                let name = raw.file_stem().unwrap_or_default().to_string_lossy();
                self.tray.notify("Captured from clipboard", &name, false);
            }
            Err(message) => self.tray.notify("Nothing captured", &message, true),
        }
        self.refresh_tip();
    }

    fn open_folder(&self) {
        if let Some(notebook) = &self.notebook {
            let raw = crate::notebook::Conventions::load(notebook);
            let folder = notebook.join(raw.raw_folder());
            let target = if folder.is_dir() {
                folder
            } else {
                notebook.clone()
            };
            if let Err(error) = shell::open(&target) {
                crate::log::line(&format!("open folder: {error}"));
            }
        }
    }

    fn open_settings(&self) {
        if let Some(path) = &self.context.settings {
            if !path.exists() {
                let _ = SettingsFile::load(path).save(path);
            }
            if let Err(error) = shell::open(path) {
                crate::log::line(&format!("open settings: {error}"));
            }
        }
    }
}

fn show_menu(window: HWND, at: POINT) {
    let Some(waiting) = with_app(|app| app.queue_counts().0) else {
        return;
    };
    let clipboard_text = with_app(|app| {
        app.settings
            .clipboard_hotkey
            .map_or("Capture clipboard".to_owned(), |key| {
                format!("Capture clipboard\t{key}")
            })
    })
    .unwrap_or_default();
    let capture_text = with_app(|app| {
        app.settings
            .capture_hotkey
            .map_or("Capture…".to_owned(), |key| format!("Capture…\t{key}"))
    })
    .unwrap_or_default();
    let review_text = format!("Review ({waiting})");
    // The app isn't borrowed while the menu is open: the menu runs a message loop of its own.
    let chosen = tray::menu(
        window,
        at,
        &[
            // The capture box (Part 7) and the review window (Part 8) come next.
            MenuItem::Command {
                id: MENU_CAPTURE,
                text: &capture_text,
                enabled: false,
            },
            MenuItem::Command {
                id: MENU_CLIPBOARD,
                text: &clipboard_text,
                enabled: true,
            },
            MenuItem::Command {
                id: MENU_REVIEW,
                text: &review_text,
                enabled: false,
            },
            MenuItem::Separator,
            MenuItem::Command {
                id: MENU_SETTINGS,
                text: "Settings",
                enabled: true,
            },
            MenuItem::Command {
                id: MENU_FOLDER,
                text: "Open notebook folder",
                enabled: true,
            },
            MenuItem::Separator,
            MenuItem::Command {
                id: MENU_QUIT,
                text: "Quit FastCapture",
                enabled: true,
            },
        ],
    );
    match chosen {
        MENU_CLIPBOARD => {
            // The window in front is the taskbar here, so no source is recorded.
            with_app(|app| app.capture_clipboard(None));
        }
        MENU_SETTINGS => {
            with_app(|app| app.open_settings());
        }
        MENU_FOLDER => {
            with_app(|app| app.open_folder());
        }
        MENU_QUIT => unsafe {
            DestroyWindow(window);
        },
        _ => {}
    }
}

extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_HOTKEY => {
            match wparam as i32 {
                HOTKEY_CLIPBOARD => {
                    let source = with_app(|app| app.settings.capture_source)
                        .unwrap_or(false)
                        .then(crate::platform::window::foreground_title)
                        .flatten();
                    with_app(|app| app.capture_clipboard(source));
                }
                HOTKEY_CAPTURE => crate::log::line("capture box: arrives in M1 Part 7"),
                _ => {}
            }
            0
        }
        WM_APP_TRAY => {
            let event = (lparam as u32) & 0xFFFF;
            match event {
                tray::NIN_SELECT | tray::NIN_KEYSELECT => {
                    // The review window (Part 8) opens here; until then, the raw folder.
                    with_app(|app| app.open_folder());
                }
                tray::WM_CONTEXTMENU => {
                    let at = POINT {
                        x: (wparam & 0xFFFF) as i16 as i32,
                        y: ((wparam >> 16) & 0xFFFF) as i16 as i32,
                    };
                    show_menu(window, at);
                }
                _ => {}
            }
            0
        }
        WM_APP_QUEUE => {
            with_app(App::refresh_tip);
            0
        }
        WM_APP_SETTINGS => {
            with_app(App::apply_settings);
            0
        }
        WM_DESTROY => {
            // The tray icon goes while its window still exists, so no ghost icon stays behind.
            APP.with(|slot| slot.try_borrow_mut().map(|mut app| app.take()).ok());
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => {
            if with_app(|app| app.taskbar_created == message && message != 0) == Some(true) {
                with_app(|app| app.tray.show());
                return 0;
            }
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
    }
}
