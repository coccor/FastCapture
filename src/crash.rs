//! What happens when the app panics: the panic is written to the log, with its place and the
//! version, before the process stops.

/// Makes every panic write a line to the log and then stop the process.
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
        crate::log::line(&line(message, location, &thread, false));
        std::process::abort();
    }));
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
