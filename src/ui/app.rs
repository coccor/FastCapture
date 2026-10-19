//! The tray app: a hidden window that owns the tray icon and the hotkeys, a
//! thread answering `fastcapture add` over the pipe, a watch on the settings folder, a
//! start-up pass over the queue, and the worker that turns captures into notes. Everything
//! that touches windows runs on this thread.

use crate::add::{self, Context};
use crate::capture::Capture;
use crate::capture::file::FileError;
use crate::capture::submit::submit;
use crate::config::settings::{Settings, SettingsFile};
use crate::ipc::pipe;
use crate::platform::tray::{self, MenuItem, Tray};
use crate::platform::window::SendHwnd;
use crate::platform::{hotkey, instance, shell, startup};
use crate::queue::{State, Store};
use crate::runner::from_settings::FromSettings;
use crate::runner::{Status, Waker, Worker};
use crate::ui::settings as settings_window;
use crate::ui::{capture_box, review};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, MSG,
    PostMessageW, PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SendMessageW,
    TranslateMessage, WM_APP, WM_DESTROY, WM_HOTKEY, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
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
const MENU_PAUSE: u32 = 7;
const MENU_SETTINGS_FILE: u32 = 8;

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
    worker: Option<Worker>,
    /// The last error pause reported, so the notice shows once.
    reported_pause: Option<String>,
}

/// Wakes the worker from any thread of this process, after a capture is filed.
static WAKER: OnceLock<Waker> = OnceLock::new();

/// Tells the worker a job may be waiting.
fn wake_worker() {
    if let Some(waker) = WAKER.get() {
        waker.wake();
    }
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
        taskbar_created,
        worker: None,
        reported_pause: None,
        context,
    };
    APP.with(|slot| *slot.borrow_mut() = Some(app));
    with_app(App::apply_settings);
    start_worker(window);
    // Until a model is chosen, captures are kept but no notes are written: say so once.
    with_app(|app| {
        if app.settings.mode.is_none() {
            app.tray.notify(
                "Choose how notes are written",
                "Open Settings from the tray menu: a model on this PC, or Claude.",
                false,
            );
        }
    });

    let stop = Arc::new(AtomicBool::new(false));
    start_pipe(window, stop.clone());
    start_settings_watch(window);
    start_orphan_scan(window);

    let mut message: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
        // Tab, Enter and Esc in the review and Settings windows.
        if review::dialog_message(&message) || settings_window::dialog_message(&message) {
            continue;
        }
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
            wake_worker();
            unsafe { PostMessageW(window.get(), WM_APP_QUEUE, 0, 0) };
            reply
        });
    });
}

/// Jobs left working by a crash are queued again, then the worker starts on the queue.
fn start_worker(window: HWND) {
    let Some((queue, settings)) =
        with_app(|app| (app.context.queue.clone(), app.context.settings.clone()))
    else {
        return;
    };
    let Some(queue) = queue else {
        return;
    };
    let recovered = Store::new(queue.clone()).recover();
    if recovered > 0 {
        crate::log::line(&format!("queue: {recovered} interrupted jobs queued again"));
    }
    let window = SendHwnd::new(window);
    let worker = Worker::start(
        queue,
        Box::new(FromSettings { settings }),
        Box::new(move || unsafe {
            PostMessageW(window.get(), WM_APP_QUEUE, 0, 0);
        }),
    );
    let _ = WAKER.set(worker.waker());
    with_app(|app| app.worker = Some(worker));
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

/// After a restart: raw sources with no job (written while the tray app was closed) get one.
fn start_orphan_scan(window: HWND) {
    let Some(queue) = with_app(|app| app.context.queue.clone()).flatten() else {
        return;
    };
    let notebook = with_app(|app| app.notebook.clone()).flatten();
    let window = SendHwnd::new(window);
    std::thread::spawn(move || {
        let added = notebook
            .map(|root| {
                Store::new(queue)
                    .enqueue_orphans(&crate::notebook::Conventions::load(&root))
                    .len()
            })
            .unwrap_or(0);
        if added > 0 {
            crate::log::line(&format!("queue: {added} raw sources without a job"));
            wake_worker();
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
                    &format!("{text} is in use by another app. Change {key} in the settings file (tray menu › Open settings file)."),
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
        if let Some(worker) = &self.worker {
            worker.waker().settings_changed();
        }
        self.refresh_tip();
    }

    /// Captures waiting, notes written, and failures.
    fn queue_counts(&self) -> (usize, usize, usize) {
        let Some(queue) = &self.context.queue else {
            return (0, 0, 0);
        };
        let jobs = Store::new(queue.clone()).list();
        let count =
            |wanted: fn(&crate::queue::Job) -> bool| jobs.iter().filter(|job| wanted(job)).count();
        (
            count(|job| matches!(job.state, State::Queued | State::Working)),
            count(|job| job.state == State::Done && job.note.is_some()),
            count(|job| job.state == State::Failed),
        )
    }

    fn status(&self) -> Status {
        self.worker.as_ref().map_or(Status::Idle, Worker::status)
    }

    fn refresh_tip(&mut self) {
        let (waiting, notes, failed) = self.queue_counts();
        let mut tip = match &self.notebook {
            Some(notebook) => format!(
                "FastCapture → {}",
                notebook.file_name().unwrap_or_default().to_string_lossy()
            ),
            None => "FastCapture: no notebook set".to_owned(),
        };
        let status = self.status();
        tip.push('\n');
        tip.push_str(&match &status {
            Status::Working => "Writing a note…".to_owned(),
            Status::Paused(None) => "Paused".to_owned(),
            Status::Paused(Some(reason)) | Status::Unready(reason) => reason.clone(),
            Status::Waiting { .. } => "Waiting to retry".to_owned(),
            Status::Idle if waiting == 0 => "Nothing waiting".to_owned(),
            Status::Idle => format!("{waiting} queued"),
        });
        if notes > 0 {
            tip.push_str(&format!(
                "\n{notes} note{} to review",
                if notes == 1 { "" } else { "s" }
            ));
        }
        if failed > 0 {
            tip.push_str(&format!(", {failed} failed"));
        }
        // An error that paused the queue is said once, out loud.
        match &status {
            Status::Paused(Some(reason)) if self.reported_pause.as_ref() != Some(reason) => {
                self.tray.notify("FastCapture paused", reason, true);
                self.reported_pause = Some(reason.clone());
            }
            Status::Paused(Some(_)) => {}
            _ => self.reported_pause = None,
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
                wake_worker();
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

    fn open_settings_file(&self) {
        open_settings_file(self.context.settings.as_deref());
    }
}

/// Opens `fastcapture.ini` in its editor, writing the starter file first.
fn open_settings_file(path: Option<&Path>) {
    if let Some(path) = path {
        if !path.exists() {
            let _ = SettingsFile::load(path).save(path);
        }
        if let Err(error) = shell::open(path) {
            crate::log::line(&format!("open settings: {error}"));
        }
    }
}

/// The Settings window; saving it applies the settings at once.
fn open_settings_window(window: HWND) {
    let Some(path) = with_app(|app| app.context.settings.clone()).flatten() else {
        return;
    };
    let target = SendHwnd::new(window);
    let file = path.clone();
    let opened = settings_window::open(settings_window::Open {
        settings: path,
        key_target: crate::secrets::target("claude"),
        saved: Box::new(move || unsafe {
            PostMessageW(target.get(), WM_APP_SETTINGS, 0, 0);
        }),
        open_file: Box::new(move || open_settings_file(Some(&file))),
    });
    if opened.is_none() {
        crate::log::line("settings: could not open the window");
    }
}

/// The tray icon's click and the Review menu item: opens the review window, or brings it forward.
fn open_review(window: HWND) {
    let Some((queue, settings, notebook)) = with_app(|app| {
        (
            app.context.queue.clone(),
            app.context.settings.clone(),
            app.notebook.clone(),
        )
    }) else {
        return;
    };
    let Some(queue) = queue else {
        return;
    };
    let window = SendHwnd::new(window);
    let opened = review::open(review::Open {
        queue,
        settings,
        notebook,
        actions: Box::new(review::ShellActions {
            // A capture queued again (R) needs the worker.
            on_change: Box::new(move || {
                wake_worker();
                unsafe { PostMessageW(window.get(), WM_APP_QUEUE, 0, 0) };
            }),
        }),
    });
    if opened.is_none() {
        crate::log::line("review: could not open");
    }
}

/// Files the capture box's captures and queues them.
struct BoxSink {
    window: SendHwnd,
    queue: Option<PathBuf>,
    notebooks: Vec<PathBuf>,
}

impl capture_box::Sink for BoxSink {
    fn notebooks(&self) -> Vec<PathBuf> {
        self.notebooks.clone()
    }

    fn file(&mut self, capture: &Capture, notebook: &Path) -> Result<(), String> {
        // Large files were asked about when they were attached.
        let filed = submit(notebook, capture, self.queue.as_deref(), true);
        if filed.is_ok() {
            wake_worker();
        }
        unsafe { PostMessageW(self.window.get(), WM_APP_QUEUE, 0, 0) };
        filed.map(|_| ()).map_err(|error| error.to_string())
    }
}

/// The capture hotkey or menu item: opens the box, or brings the open one forward.
fn open_capture_box(window: HWND, source_window: Option<String>) {
    if let Some(open) = capture_box::find() {
        unsafe { SendMessageW(open, capture_box::WM_CAPTURE_AGAIN, 0, 0) };
        return;
    }
    let Some((notebook, queue, notebooks)) = with_app(|app| {
        (
            app.notebook.clone(),
            app.context.queue.clone(),
            app.settings.recent_notebooks.clone(),
        )
    }) else {
        return;
    };
    let Some(notebook) = notebook else {
        with_app(|app| {
            app.tray.notify(
                "No notebook is set",
                "Choose one in Settings (tray menu › Settings…).",
                true,
            )
        });
        return;
    };
    let opened = capture_box::open(capture_box::Open {
        sink: Box::new(BoxSink {
            window: SendHwnd::new(window),
            queue,
            notebooks,
        }),
        notebook,
        draft_dir: crate::capture::draft::default_dir(),
        source_window,
    });
    if opened.is_none() {
        crate::log::line("capture box: could not open");
    }
}

fn show_menu(window: HWND, at: POINT) {
    let Some((waiting, notes, paused)) = with_app(|app| {
        let (waiting, notes, _) = app.queue_counts();
        (waiting, notes, matches!(app.status(), Status::Paused(_)))
    }) else {
        return;
    };
    let pause_text = if paused {
        "Resume processing"
    } else {
        "Pause processing"
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
    let review_text = format!("Review ({})", waiting + notes);
    // The app isn't borrowed while the menu is open: the menu runs a message loop of its own.
    let chosen = tray::menu(
        window,
        at,
        &[
            MenuItem::Command {
                id: MENU_CAPTURE,
                text: &capture_text,
                enabled: true,
            },
            MenuItem::Command {
                id: MENU_CLIPBOARD,
                text: &clipboard_text,
                enabled: true,
            },
            MenuItem::Command {
                id: MENU_REVIEW,
                text: &review_text,
                enabled: true,
            },
            MenuItem::Command {
                id: MENU_PAUSE,
                text: pause_text,
                enabled: true,
            },
            MenuItem::Separator,
            MenuItem::Command {
                id: MENU_SETTINGS,
                text: "Settings…",
                enabled: true,
            },
            MenuItem::Command {
                id: MENU_SETTINGS_FILE,
                text: "Open settings file",
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
        // The window in front is the taskbar here, so no source is recorded.
        MENU_CAPTURE => open_capture_box(window, None),
        MENU_REVIEW => open_review(window),
        MENU_PAUSE => {
            with_app(|app| {
                if let Some(worker) = &app.worker {
                    worker.set_paused(!paused);
                }
            });
        }
        MENU_CLIPBOARD => {
            with_app(|app| app.capture_clipboard(None));
        }
        MENU_SETTINGS => open_settings_window(window),
        MENU_SETTINGS_FILE => {
            with_app(|app| app.open_settings_file());
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
                HOTKEY_CAPTURE => {
                    let source = with_app(|app| app.settings.capture_source)
                        .unwrap_or(false)
                        .then(crate::platform::window::foreground_title)
                        .flatten();
                    open_capture_box(window, source);
                }
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
            if let Some(open) = review::find() {
                unsafe { PostMessageW(open, review::WM_REVIEW_REFRESH, 0, 0) };
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::clipboard::{self, tests::Backup};
    use crate::queue::store::raw_sources;
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;

    #[test]
    fn the_box_files_a_pasted_image_and_text_into_the_notebook_and_queue() {
        let temp = std::env::temp_dir().join(format!("fastcapture-app-box-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let notebook = temp.join("Notes");
        let queue = temp.join("queue");
        std::fs::create_dir_all(&notebook).unwrap();

        let _backup = Backup::take();
        let png_format: Vec<u16> = "PNG\0".encode_utf16().collect();
        let format = unsafe {
            windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW(png_format.as_ptr())
        };
        clipboard::set(std::ptr::null_mut(), format, b"\x89PNG\r\n\x1a\nimage").unwrap();

        let hwnd = capture_box::open(capture_box::Open {
            sink: Box::new(BoxSink {
                window: SendHwnd::new(std::ptr::null_mut()),
                queue: Some(queue.clone()),
                notebooks: Vec::new(),
            }),
            notebook: notebook.clone(),
            draft_dir: Some(temp.join("draft")),
            source_window: None,
        })
        .unwrap();
        capture_box::paste_for_test(hwnd);
        capture_box::set_text_for_test(hwnd, "lag spikes during ledger batch??");
        capture_box::run_for_test(hwnd, capture_box::Action::Submit);
        assert_eq!(unsafe { IsWindow(hwnd) }, 0, "the box closed");

        let raws = raw_sources(&notebook, Path::new("raw"));
        assert_eq!(raws.len(), 1);
        let text = std::fs::read_to_string(notebook.join(&raws[0].path)).unwrap();
        assert!(text.contains("kind: [text, screenshot]"), "{text}");
        assert!(text.contains("lag spikes during ledger batch??"), "{text}");
        let image = notebook.join("attachments").join(format!(
            "{}.png",
            raws[0].path.file_stem().unwrap().to_string_lossy()
        ));
        assert_eq!(std::fs::read(&image).unwrap(), b"\x89PNG\r\n\x1a\nimage");
        let jobs = Store::new(queue).list();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].capture_id, raws[0].capture_id);
        let _ = std::fs::remove_dir_all(&temp);
    }
}
