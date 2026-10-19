//! FastCapture: a Windows tray app that captures text, screenshots, links and files into a
//! notebook as raw sources, and turns them into Markdown notes.

pub mod add;
pub mod capture;
pub mod config;
pub mod error;
pub mod extract;
pub mod icon;
pub mod index;
pub mod ipc;
pub mod launch;
pub mod log;
pub mod model;
pub mod net;
pub mod note;
pub mod notebook;
pub mod paths;
pub mod platform;
pub mod queue;
pub mod runner;
pub mod secrets;
pub mod ui;

pub use error::{Error, Result};

use launch::Launch;
use std::io::Write;

/// Runs the app with the process's arguments and returns its exit code.
pub fn run() -> i32 {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_default();
    let launch = match launch::parse(&args, &cwd) {
        Ok(launch) => launch,
        Err(message) => {
            platform::console::attach();
            let _ = writeln!(std::io::stderr(), "fastcapture: {message}");
            return 2;
        }
    };
    match launch {
        Launch::Add(args) => add::run(args),
        Launch::Settings => open_settings(),
        Launch::Version => print(&format!("FastCapture {}", env!("CARGO_PKG_VERSION"))),
        Launch::Help => print(launch::USAGE),
        Launch::Tray => ui::app::run(),
    }
}

fn print(text: &str) -> i32 {
    platform::console::attach();
    let _ = writeln!(std::io::stdout(), "{text}");
    0
}

/// Opens `fastcapture.ini` in its default program, writing the starter file first when there
/// is none.
fn open_settings() -> i32 {
    let Some(path) = config::settings::settings_path() else {
        return 1;
    };
    if !path.exists() {
        let file = config::settings::SettingsFile::load(&path);
        if let Err(error) = file.save(&path) {
            log::line(&format!("settings: {error}"));
            return 1;
        }
    }
    match platform::shell::open(&path) {
        Ok(()) => 0,
        Err(error) => {
            log::line(&format!("settings: {error}"));
            1
        }
    }
}
