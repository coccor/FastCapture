//! The text "Copy diagnostics" puts on the clipboard for a bug report: versions, settings by
//! name, counts and the log's tail. Never keys, endpoints, paths or anything captured.

/// What the report says about this PC and this run.
pub struct Facts {
    pub version: &'static str,
    /// The Windows build, when it could be read.
    pub build: Option<u32>,
    pub arch: &'static str,
    /// `local`, `cloud` or `none`.
    pub mode: &'static str,
    pub backend: &'static str,
    pub model: String,
    /// `missing`, `network`, `onedrive` or `local`.
    pub notebook_kind: &'static str,
    /// The notes indexed, when the index is ready.
    pub notes: Option<usize>,
    /// Jobs queued, working, failed and done.
    pub queue: [usize; 4],
    /// Why the queue is paused, when it is.
    pub paused: Option<String>,
}

/// How many log lines the report ends with.
pub const LOG_LINES: usize = 100;

/// The report for `facts`, ending with `log_tail`.
pub fn report(facts: &Facts, log_tail: &str) -> String {
    let build = facts
        .build
        .map_or_else(|| "?".to_owned(), |build| build.to_string());
    let notes = facts
        .notes
        .map_or_else(|| "?".to_owned(), |notes| notes.to_string());
    let [queued, working, failed, done] = facts.queue;
    let paused = facts.paused.as_deref().unwrap_or("no");
    format!(
        "FastCapture {}, Windows {build} {}\n\
         Mode: {}, backend: {}, model: {}\n\
         Notebook: {}, notes indexed: {notes}\n\
         Queue: {queued} queued, {working} working, {failed} failed, {done} done\n\
         Paused: {paused}\n\
         \n\
         Last {LOG_LINES} log lines:\n\
         {log_tail}\n",
        facts.version, facts.arch, facts.mode, facts.backend, facts.model, facts.notebook_kind,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            version: "0.1.3",
            build: Some(26100),
            arch: "x86_64",
            mode: "cloud",
            backend: "openai",
            model: "openai/gpt-5-mini".to_owned(),
            notebook_kind: "onedrive",
            notes: Some(412),
            queue: [3, 1, 2, 40],
            paused: None,
        }
    }

    #[test]
    fn the_report_has_each_line_in_order() {
        let text = report(&facts(), "l1\nl2");
        assert_eq!(
            text,
            "FastCapture 0.1.3, Windows 26100 x86_64\n\
             Mode: cloud, backend: openai, model: openai/gpt-5-mini\n\
             Notebook: onedrive, notes indexed: 412\n\
             Queue: 3 queued, 1 working, 2 failed, 40 done\n\
             Paused: no\n\
             \n\
             Last 100 log lines:\n\
             l1\nl2\n"
        );
    }

    #[test]
    fn unknown_facts_and_a_pause_reason_are_said() {
        let mut facts = facts();
        facts.build = None;
        facts.notes = None;
        facts.paused = Some("by you".to_owned());
        let text = report(&facts, "");
        assert!(text.contains("Windows ? x86_64"));
        assert!(text.contains("notes indexed: ?"));
        assert!(text.contains("Paused: by you\n"));
    }

    #[test]
    fn a_url_in_a_pause_reason_is_reduced_to_its_host() {
        let reason = crate::log::scrubbed("paused: https://example.com/private/key?x=1 refused");
        let mut facts = facts();
        facts.paused = Some(reason);
        let text = report(&facts, "");
        assert!(text.contains("example.com"));
        assert!(!text.contains("private"));
    }

    #[test]
    fn the_report_holds_no_path_or_url_beyond_the_model_name() {
        let text = report(&facts(), "");
        assert!(!text.contains('\\'));
        assert!(!text.contains(":/"));
        assert!(!text.replace("openai/gpt-5-mini", "").contains('/'));
    }
}
