//! The soak run: starts a release fastcapture.exe against a fake local model, captures into it
//! every two minutes and watches its handles, GDI and USER objects and private bytes, to show
//! that none of them grows over days of use.
//!
//! `cargo run --release --example soak -- target\release\fastcapture.exe <hours>`
//!
//! FastCapture must be quit first: one tray app runs per Windows session. The exe gets settings,
//! a queue and a notebook of its own in a scratch folder, with `APPDATA` and `LOCALAPPDATA`
//! pointing there; nothing of the user's FastCapture is read or written. While it starts it
//! syncs the "start at sign-in" entry with its own settings, so the entry is put back as it was
//! once it has started, and again at the end.
//!
//! Every minute a line `elapsed_s,handles,gdi,user,private_bytes` goes to `soak.csv` in the
//! scratch folder. At the end each counter's change per hour is printed, measured after the
//! first hour (or the first third of a shorter run), with `flat` or `GROWING`. When the run ends
//! or is interrupted with Ctrl+C, the exe is closed the way Quit closes it, so its tray icon
//! goes too.

use fastcapture::extract::pdf::build::{self as pdf_build, Page};
use fastcapture::ipc::pipe::{self, SendError};
use fastcapture::ipc::protocol::Request;
use fastcapture::net::test_server::{Scripted, TestServer};
use fastcapture::platform::job::{CREATE_NO_WINDOW, Job};
use fastcapture::platform::png::{Pixels, encode_png};
use fastcapture::platform::startup;
use std::io::Write;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM};
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::Threading::{
    GR_GDIOBJECTS, GR_USEROBJECTS, GetGuiResources, GetProcessHandleCount,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
};

const SAMPLE_EVERY: Duration = Duration::from_secs(60);
const CAPTURE_EVERY: Duration = Duration::from_secs(120);
/// How long the exe is given to close after it is asked to.
const CLOSE_WITHIN: Duration = Duration::from_secs(15);
/// Above these changes an hour, a counter is growing.
const HANDLES_LIMIT: f64 = 0.0;
const GDI_LIMIT: f64 = 0.0;
const PRIVATE_BYTES_LIMIT: f64 = 1024.0 * 1024.0;

/// The note the fake model writes for every capture.
fn model_reply() -> Scripted {
    let note = serde_json::json!({
        "title": "Soak capture",
        "summary": "A capture made by the soak run.",
        "body": "The soak run captures text, a picture and a PDF in turn, all day long.",
        "key_points": ["It runs for days"],
        "tasks": [],
        "tags": ["soak"],
        "links": [],
        "home": null
    });
    let body = serde_json::json!({
        "id": "soak",
        "object": "chat.completion",
        "model": "fake",
        "choices": [{
            "index": 0,
            "finish_reason": "stop",
            "message": {"role": "assistant", "content": note.to_string()}
        }],
        "usage": {"prompt_tokens": 100, "completion_tokens": 50, "total_tokens": 150}
    });
    Scripted::json(200, &body.to_string())
}

/// What must be undone however the run ends.
struct Running {
    child: Child,
    job: Option<Job>,
    /// The "start at sign-in" entry as it was before the run.
    sign_in: Option<String>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(exe), Some(hours)) = (
        args.first().map(PathBuf::from),
        args.get(1).and_then(|hours| hours.parse::<f64>().ok()),
    ) else {
        eprintln!("usage: soak <fastcapture.exe> <hours>");
        std::process::exit(2);
    };
    let exe = std::path::absolute(&exe).unwrap_or(exe);
    if !exe.is_file() {
        eprintln!("soak: no such file: {}", exe.display());
        std::process::exit(2);
    }
    let scratch = std::env::temp_dir().join(format!("fastcapture-soak-{}", std::process::id()));
    let appdata = scratch.join("appdata");
    let localappdata = scratch.join("localappdata");
    let notebook = scratch.join("notebook");

    // An attachment that doesn't exist: a running tray app refuses it, and files nothing.
    let probe = Request::Add {
        text: String::new(),
        attachments: vec![scratch.join("no-such-file.png")],
        notebook: None,
    };
    if !matches!(pipe::send(&probe), Err(SendError::NotRunning)) {
        eprintln!(
            "soak: Quit FastCapture first. One tray app runs per Windows session, and the soak \
             run starts its own."
        );
        std::process::exit(2);
    }

    let server = TestServer::answering(model_reply());
    for dir in [
        appdata.join("FastCapture"),
        localappdata.clone(),
        notebook.clone(),
    ] {
        std::fs::create_dir_all(&dir).expect("the scratch folder can be made");
    }
    let settings = [
        "version=1".to_owned(),
        "mode=local".to_owned(),
        format!("local_endpoint={}", server.url()),
        "local_model=fake".to_owned(),
        format!("notebook={}", notebook.display()),
        "start_at_sign_in=false".to_owned(),
        String::new(),
    ]
    .join("\r\n");
    std::fs::write(appdata.join(r"FastCapture\fastcapture.ini"), settings)
        .expect("the settings can be written");
    let inputs = Inputs::write(&scratch);
    let environment = [
        ("APPDATA", appdata.clone()),
        ("LOCALAPPDATA", localappdata.clone()),
    ];

    let sign_in = startup::get(startup::VALUE);
    let child = Command::new(&exe)
        .envs(environment.clone())
        .spawn()
        .expect("fastcapture.exe starts");
    // Should this process die without cleaning up, closing the job ends the exe too.
    let job = Job::new().filter(|job| job.add(&child));
    let process = child.as_raw_handle() as HANDLE;
    *RUNNING.lock().unwrap() = Some(Running {
        child,
        job,
        sign_in,
    });
    unsafe { SetConsoleCtrlHandler(Some(on_ctrl), 1) };

    let csv_path = scratch.join("soak.csv");
    println!("soak: {} for {hours} h", exe.display());
    println!("soak: scratch folder {}", scratch.display());
    println!("soak: writing {}", csv_path.display());
    wait_until_started(&localappdata, Duration::from_secs(30));
    put_sign_in_back();

    let mut csv = std::fs::File::create(&csv_path).expect("soak.csv can be written");
    let _ = writeln!(csv, "elapsed_s,handles,gdi,user,private_bytes");
    let length = Duration::from_secs_f64(hours * 3600.0);
    let began = Instant::now();
    let mut samples: Vec<Sample> = Vec::new();
    let mut next_sample = Duration::ZERO;
    let mut next_capture = SAMPLE_EVERY / 2;
    let mut captures = 0usize;
    let mut failed_captures = 0usize;
    loop {
        let elapsed = began.elapsed();
        if let Some(code) = exited() {
            eprintln!("soak: fastcapture.exe ended early, with exit code {code:?}");
            finish();
            std::process::exit(1);
        }
        if elapsed >= next_sample {
            let sample = Sample::read(process, elapsed);
            let _ = writeln!(
                csv,
                "{},{},{},{},{}",
                elapsed.as_secs(),
                sample.handles,
                sample.gdi,
                sample.user,
                sample.private_bytes
            );
            let _ = csv.flush();
            samples.push(sample);
            next_sample += SAMPLE_EVERY;
        }
        if elapsed >= length {
            break;
        }
        if elapsed >= next_capture {
            if !capture(&exe, &environment, &inputs, captures) {
                failed_captures += 1;
            }
            captures += 1;
            next_capture += CAPTURE_EVERY;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    finish();
    let notes = count_files(&notebook, "md");
    println!(
        "soak: {captures} captures ({failed_captures} failed), {} model requests answered, {notes} \
         Markdown files in the notebook",
        server.requests().len()
    );
    let verdict = report(&samples);
    std::process::exit(if verdict { 0 } else { 1 });
}

/// The files `add` attaches, written once.
struct Inputs {
    png: PathBuf,
    pdf: PathBuf,
}

impl Inputs {
    fn write(scratch: &Path) -> Self {
        let inputs = scratch.join("inputs");
        std::fs::create_dir_all(&inputs).expect("the inputs folder can be made");
        let (width, height) = (320u32, 200u32);
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                bgra.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 128, 255]);
            }
        }
        let png = encode_png(&Pixels {
            width,
            height,
            bgra,
        })
        .expect("the picture encodes");
        let pdf = pdf_build::pdf(
            &[
                Page::Text(&[
                    "A page of text the soak run attaches, long enough to read as text.",
                    "It has a second line, so the page has a little more on it.",
                ]),
                Page::Picture,
            ],
            false,
        );
        let this = Self {
            png: inputs.join("picture.png"),
            pdf: inputs.join("document.pdf"),
        };
        std::fs::write(&this.png, png).expect("the picture can be written");
        std::fs::write(&this.pdf, pdf).expect("the PDF can be written");
        this
    }
}

/// Runs `fastcapture add` with text, a picture or a PDF, by turn; false when it failed.
fn capture(exe: &Path, environment: &[(&str, PathBuf)], inputs: &Inputs, turn: usize) -> bool {
    let mut command = Command::new(exe);
    command
        .arg("add")
        .envs(environment.iter().cloned())
        .creation_flags(CREATE_NO_WINDOW);
    match turn % 3 {
        0 => command.arg(format!(
            "Soak capture {turn}: some text to turn into a note."
        )),
        1 => command.arg("--attach").arg(&inputs.png),
        _ => command.arg("--attach").arg(&inputs.pdf),
    };
    match command.output() {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            eprintln!(
                "soak: add failed ({:?}): {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
            false
        }
        Err(error) => {
            eprintln!("soak: add didn't start: {error}");
            false
        }
    }
}

/// A counter's name, how it is read from a sample, and the change an hour it may not pass.
type Counter = (&'static str, fn(&Sample) -> f64, Option<f64>);

struct Sample {
    elapsed: Duration,
    handles: u32,
    gdi: u32,
    user: u32,
    private_bytes: usize,
}

impl Sample {
    fn read(process: HANDLE, elapsed: Duration) -> Self {
        let mut handles = 0;
        unsafe { GetProcessHandleCount(process, &mut handles) };
        let gdi = unsafe { GetGuiResources(process, GR_GDIOBJECTS) };
        let user = unsafe { GetGuiResources(process, GR_USEROBJECTS) };
        let mut memory: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
        memory.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        unsafe {
            GetProcessMemoryInfo(
                process,
                (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
                memory.cb,
            )
        };
        Self {
            elapsed,
            handles,
            gdi,
            user,
            private_bytes: memory.PrivateUsage,
        }
    }
}

/// Prints each counter's change per hour, and whether any grows; true when all are flat.
///
/// The change is the median of the last third of the samples after the warm-up less the median
/// of the first third, over the hours between them, so a counter that goes up and down with the
/// work of the moment but comes back is flat.
fn report(samples: &[Sample]) -> bool {
    let total = samples.last().map_or(Duration::ZERO, |last| last.elapsed);
    let warm_up = Duration::from_secs(3600).min(total / 3);
    let after: Vec<&Sample> = samples.iter().filter(|s| s.elapsed >= warm_up).collect();
    if after.len() < 3 {
        println!("soak: too few samples to tell; run it for longer");
        return false;
    }
    let third = after.len() / 3;
    let (first, last) = (&after[..third], &after[after.len() - third..]);
    let middle = |part: &[&Sample]| part[part.len() / 2].elapsed.as_secs_f64();
    let hours = (middle(last) - middle(first)) / 3600.0;
    let median = |part: &[&Sample], value: fn(&Sample) -> f64| {
        let mut values: Vec<f64> = part.iter().map(|s| value(s)).collect();
        values.sort_by(f64::total_cmp);
        values[values.len() / 2]
    };
    println!(
        "soak: {} samples over {:.2} h; changes an hour after the first {} min:",
        samples.len(),
        total.as_secs_f64() / 3600.0,
        warm_up.as_secs() / 60
    );
    let counters: [Counter; 4] = [
        ("handles", |s| f64::from(s.handles), Some(HANDLES_LIMIT)),
        ("gdi", |s| f64::from(s.gdi), Some(GDI_LIMIT)),
        ("user", |s| f64::from(s.user), None),
        (
            "private_bytes",
            |s| s.private_bytes as f64,
            Some(PRIVATE_BYTES_LIMIT),
        ),
    ];
    let mut flat = true;
    for (name, value, limit) in counters {
        let change = (median(last, value) - median(first, value)) / hours;
        let growing = limit.is_some_and(|limit| change > limit);
        flat &= !growing;
        let judged = match limit {
            Some(limit) => format!(
                "(limit {limit:.0}) {}",
                if growing { "GROWING" } else { "flat" }
            ),
            None => "(not judged)".to_owned(),
        };
        println!(
            "  {name:<14} {:>12.0} -> {:>12.0}  {change:>+14.1} an hour  {judged}",
            median(first, value),
            median(last, value)
        );
    }
    println!("soak: {}", if flat { "flat" } else { "GROWING" });
    flat
}

/// Waits until the exe logs that it started, so it has synced its settings.
fn wait_until_started(localappdata: &Path, within: Duration) {
    let log = localappdata.join(r"FastCapture\log");
    let began = Instant::now();
    while began.elapsed() < within {
        let started = std::fs::read_dir(&log)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| {
                std::fs::read_to_string(entry.path()).is_ok_and(|text| text.contains(" started"))
            });
        if started {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    eprintln!("soak: fastcapture.exe didn't log that it started; going on");
}

/// Puts the "start at sign-in" entry back as it was before the run.
fn put_sign_in_back() {
    let Ok(running) = RUNNING.lock() else { return };
    let Some(running) = running.as_ref() else {
        return;
    };
    if startup::get(startup::VALUE) != running.sign_in
        && let Err(error) = startup::set(startup::VALUE, running.sign_in.as_deref())
    {
        eprintln!("soak: couldn't put the start at sign-in entry back: {error}");
    }
}

/// The exe's exit code when it has ended.
fn exited() -> Option<Option<i32>> {
    let mut running = RUNNING.lock().ok()?;
    let child = &mut running.as_mut()?.child;
    child.try_wait().ok().flatten().map(|status| status.code())
}

/// Closes the exe as Quit does (its window gets `WM_CLOSE`, so the tray icon goes with it), or
/// ends it when it doesn't close in time, then puts the sign-in entry back. Safe to call twice.
fn finish() {
    put_sign_in_back();
    let Some(mut running) = RUNNING.lock().ok().and_then(|mut running| running.take()) else {
        return;
    };
    if running.child.try_wait().ok().flatten().is_none() {
        close_windows_of(running.child.id());
        let began = Instant::now();
        while began.elapsed() < CLOSE_WITHIN {
            if running.child.try_wait().ok().flatten().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if running.child.try_wait().ok().flatten().is_none() {
            eprintln!("soak: fastcapture.exe didn't close; ending it");
            match &running.job {
                Some(job) => job.end(),
                None => {
                    let _ = running.child.kill();
                }
            }
            let _ = running.child.wait();
        }
    }
    // Once more, in case the exe synced its settings again before it closed.
    if startup::get(startup::VALUE) != running.sign_in {
        let _ = startup::set(startup::VALUE, running.sign_in.as_deref());
    }
}

/// Posts `WM_CLOSE` to the tray app's window in process `id`.
fn close_windows_of(id: u32) {
    unsafe extern "system" fn each(window: HWND, id: LPARAM) -> i32 {
        let mut owner = 0;
        unsafe { GetWindowThreadProcessId(window, &mut owner) };
        if owner == id as u32 {
            let mut class = [0u16; 64];
            let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
            if String::from_utf16_lossy(&class[..length.max(0) as usize]) == "FastCaptureTray" {
                unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
            }
        }
        1
    }
    unsafe { EnumWindows(Some(each), id as LPARAM) };
}

/// Ctrl+C, Ctrl+Break or the console closing: close the exe before this process goes.
unsafe extern "system" fn on_ctrl(_kind: u32) -> i32 {
    eprintln!("soak: interrupted; closing fastcapture.exe");
    finish();
    std::process::exit(130);
}

fn count_files(dir: &Path, extension: &str) -> usize {
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == extension) {
                count += 1;
            }
        }
    }
    count
}
