//! FastCapture: a Windows tray app that captures text, screenshots, links and files into a
//! notebook as raw sources, and turns them into Markdown notes
//! (`docs/specs/2026-10-18-fastcapture-design.md`).

pub mod capture;
pub mod error;
pub mod log;
pub mod notebook;
pub mod paths;
pub mod platform;

pub use error::{Error, Result};

/// Runs the app and returns the process exit code. M1 Part 1: start, log, and exit.
pub fn run() -> i32 {
    log::line(&format!(
        "FastCapture {} started",
        env!("CARGO_PKG_VERSION")
    ));
    0
}
