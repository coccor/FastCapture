//! Filing a capture and queuing it: what `fastcapture add`, the clipboard
//! hotkey and the capture box all do once a capture is ready.

use super::file::{self, FileError};
use super::{Capture, raw, ulid};
use crate::notebook::Conventions;
use crate::queue::{Job, Store};
use std::path::{Path, PathBuf};

/// Writes `capture` into the notebook at `root` and queues a job for it in `queue`. Returns the
/// raw source's absolute path. A job that can't be written is only logged: the orphan scan finds
/// the raw source again.
pub fn submit(
    root: &Path,
    capture: &Capture,
    queue: Option<&Path>,
    allow_large: bool,
) -> Result<PathBuf, FileError> {
    let conventions = Conventions::load(root);
    for warning in &conventions.warnings {
        crate::log::line(&format!(
            "{} line {}: {}",
            warning.file.display(),
            warning.line,
            warning.message
        ));
    }
    let capture_id = ulid::new();
    let (date, time) = crate::platform::clock::local_now();
    let filed = file::file_capture(&file::Request {
        conventions: &conventions,
        capture,
        capture_id: &capture_id,
        date,
        time,
        allow_large,
    })?;
    let job = Job::queued(
        &capture_id,
        root.to_path_buf(),
        filed.raw.clone(),
        raw::captured_stamp(date, time),
    );
    if let Some(queue) = queue
        && let Err(error) = Store::new(queue.to_path_buf()).put(&job)
    {
        crate::log::line(&format!("queue: {error}"));
    }
    Ok(root.join(filed.raw))
}
