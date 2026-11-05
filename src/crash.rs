//! What happens when the app panics: the panic is written to the log, with its place and the
//! version, before the process stops.

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};

thread_local! {
    /// How many `guarded` calls this thread is inside.
    static GUARDED: Cell<u32> = const { Cell::new(0) };
}

/// Lowers the count when dropped, so it is lowered even when the guarded code panics.
struct Leave;

impl Drop for Leave {
    fn drop(&mut self) {
        GUARDED.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Runs `f`, turning a panic in it into an error. Code that parses files from outside, and may
/// panic on a malformed one, runs here so the damage stays with that one file.
pub fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    GUARDED.with(|depth| depth.set(depth.get() + 1));
    let _leave = Leave;
    catch_unwind(AssertUnwindSafe(f)).map_err(|_| "it couldn't be read".to_owned())
}

/// Whether this thread is inside `guarded` now.
pub(crate) fn is_guarded() -> bool {
    GUARDED.with(|depth| depth.get() > 0)
}

/// A panic outside guarded code is fatal: the process can't be trusted to carry on.
fn should_abort(guarded: bool) -> bool {
    !guarded
}

/// Makes every panic write a line to the log, and stops the process unless the panic is
/// inside guarded code, where it is caught.
pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
        let location = info
            .location()
            .map(|place| (place.file(), place.line(), place.column()));
        let current = std::thread::current();
        let thread = current
            .name()
            .map_or_else(|| format!("{:?}", current.id()), str::to_owned);
        let recovered = is_guarded();
        crate::log::line(&line(
            logged_message(message, recovered),
            location,
            &thread,
            recovered,
        ));
        if should_abort(recovered) {
            std::process::abort();
        }
    }));
}

/// The message to log. A recovered panic comes from code reading a file from outside, and its
/// message can quote that file's text, which the log must not hold; only a fatal panic keeps it.
fn logged_message(message: Option<&str>, recovered: bool) -> Option<&str> {
    if recovered { None } else { message }
}

/// The log line for a panic. `recovered` says the panic was caught rather than fatal.
fn line(
    message: Option<&str>,
    location: Option<(&str, u32, u32)>,
    thread: &str,
    recovered: bool,
) -> String {
    let kind = if recovered {
        "panic (recovered)"
    } else {
        "panic"
    };
    let message = message.unwrap_or("(no message)");
    let place = location.map_or_else(
        || "(unknown)".to_owned(),
        |(file, line, column)| format!("{file}:{line}:{column}"),
    );
    format!(
        "{kind}: {message} at {place} (FastCapture {}, thread {thread})",
        env!("CARGO_PKG_VERSION")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guarded_panic_is_an_error_and_the_guard_is_left() {
        assert_eq!(guarded(|| 7), Ok(7));
        let result: Result<(), String> = guarded(|| panic!("bad pdf"));
        assert_eq!(result, Err("it couldn't be read".to_owned()));
        assert!(!is_guarded(), "the guard ends with the call, panic or not");
        let nested = guarded(|| guarded(is_guarded).unwrap() && is_guarded());
        assert_eq!(nested, Ok(true));
    }

    #[test]
    fn a_recovered_panic_keeps_its_message_out_of_the_log() {
        assert_eq!(logged_message(Some("inside `secret`"), true), None);
        assert_eq!(logged_message(Some("boom"), false), Some("boom"));
    }

    #[test]
    fn the_hook_aborts_unless_guarded() {
        assert!(should_abort(false));
        assert!(!should_abort(true));
    }

    #[test]
    fn the_line_names_the_panic_where_and_which_version() {
        let version = env!("CARGO_PKG_VERSION");
        assert_eq!(
            line(
                Some("index out of range"),
                Some(("src\\extract\\pdf.rs", 12, 5)),
                "worker",
                false
            ),
            format!(
                "panic: index out of range at src\\extract\\pdf.rs:12:5 (FastCapture {version}, thread worker)"
            )
        );
        assert_eq!(
            line(None, None, "main", true),
            format!(
                "panic (recovered): (no message) at (unknown) (FastCapture {version}, thread main)"
            )
        );
    }
}
