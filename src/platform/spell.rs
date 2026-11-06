//! Windows' spell checker (`ISpellChecker`, Windows 8 and later) for English: the sure fixes
//! of its autocorrect list, such as "teh" to "the".

use std::cell::OnceCell;
use windows::Win32::Globalization::{
    CORRECTIVE_ACTION_REPLACE, ISpellChecker, ISpellCheckerFactory, SpellCheckerFactory,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::core::{HSTRING, w};

const LANGUAGE: &str = "en-US";

/// This thread's English spell checker. Not `Send`: COM objects stay on their thread.
pub struct Speller {
    checker: ISpellChecker,
}

impl Speller {
    fn create() -> Result<Self, String> {
        // The checker needs COM on this thread. Already initialized (S_FALSE, or another
        // apartment) is fine; COM stays initialized while the thread lives.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        unsafe {
            let factory: ISpellCheckerFactory =
                CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(|error| format!("no spell checker factory: {error}"))?;
            let supported = factory
                .IsSupported(w!("en-US"))
                .map_err(|error| format!("{LANGUAGE} support unknown: {error}"))?;
            if !supported.as_bool() {
                return Err(format!("{LANGUAGE} is not installed"));
            }
            let checker = factory
                .CreateSpellChecker(w!("en-US"))
                .map_err(|error| format!("no {LANGUAGE} checker: {error}"))?;
            Ok(Self { checker })
        }
    }

    /// The sure replacement for `word`, if Windows has one: its autocorrect list, not a guess
    /// among suggestions.
    pub fn replacement(&self, word: &str) -> Option<String> {
        unsafe {
            let errors = self.checker.Check(&HSTRING::from(word)).ok()?;
            let mut error = None;
            if errors.Next(&mut error).is_err() {
                return None;
            }
            let error = error?;
            let whole = error.StartIndex().ok()? == 0
                && error.Length().ok()? as usize == word.encode_utf16().count();
            if !whole || error.CorrectiveAction().ok()? != CORRECTIVE_ACTION_REPLACE {
                return None;
            }
            let text = error.Replacement().ok()?;
            if text.is_null() {
                return None;
            }
            let replacement = text.to_string().ok();
            CoTaskMemFree(Some(text.0 as *const _));
            replacement
        }
    }
}

thread_local! {
    static ENGLISH: OnceCell<Option<Speller>> = const { OnceCell::new() };
}

/// Runs `f` with this thread's English speller, made the first time. `None` when Windows has
/// none: the reason is logged once and later calls don't try again.
pub fn with_english<R>(f: impl FnOnce(&Speller) -> R) -> Option<R> {
    ENGLISH.with(|cell| {
        cell.get_or_init(|| {
            Speller::create()
                .map_err(|error| crate::log::line(&format!("spell: {error}")))
                .ok()
        })
        .as_ref()
        .map(f)
    })
}

/// Reports a test that returned early because this Windows lacks what it needs. libtest hides a
/// passing test's stderr, so the name is also appended to the file named by `FASTCAPTURE_SKIPPED`
/// when that is set, which is what CI counts.
#[cfg(test)]
pub(crate) fn skipped(test: &str, why: &str) {
    let file = std::env::var_os("FASTCAPTURE_SKIPPED").map(std::path::PathBuf::from);
    record_skip(test, why, file.as_deref());
}

#[cfg(test)]
fn record_skip(test: &str, why: &str, file: Option<&std::path::Path>) {
    use std::io::Write;
    eprintln!("skipped ({why}): {test}");
    if let Some(file) = file
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
    {
        let _ = writeln!(f, "{test}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_autocorrects_teh_to_the() {
        let Some(fix) = with_english(|s| s.replacement("teh")) else {
            skipped("windows_autocorrects_teh_to_the", "no en-US spell checker");
            return;
        };
        // Windows Server (CI's runners) has the checker but not its autocorrect list: nothing
        // is ever a sure fix there, and autocorrect does nothing.
        let Some(fix) = fix else {
            skipped("windows_autocorrects_teh_to_the", "no autocorrect list");
            return;
        };
        assert_eq!(fix, "the");
    }

    #[test]
    fn correct_words_and_mere_misspellings_are_left_alone() {
        let Some((right, unsure)) =
            with_english(|s| (s.replacement("the"), s.replacement("qwzxv")))
        else {
            skipped(
                "correct_words_and_mere_misspellings_are_left_alone",
                "no en-US spell checker",
            );
            return;
        };
        assert_eq!(right, None);
        assert_eq!(unsure, None, "GET_SUGGESTIONS is not a sure fix");
    }

    #[test]
    fn a_skip_is_appended_to_the_named_file() {
        let file = std::env::temp_dir().join(format!("fc-skipped-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&file);
        record_skip("one", "why", Some(&file));
        record_skip("two", "why", Some(&file));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\ntwo\n");
        let _ = std::fs::remove_file(&file);
        record_skip("three", "why", None);
        assert!(!file.exists());
    }
}
