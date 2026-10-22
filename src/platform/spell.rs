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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_autocorrects_teh_to_the() {
        let Some(fix) = with_english(|s| s.replacement("teh")) else {
            eprintln!("skipped: no en-US spell checker");
            return;
        };
        assert_eq!(fix.as_deref(), Some("the"));
    }

    #[test]
    fn correct_words_and_mere_misspellings_are_left_alone() {
        let Some((right, unsure)) =
            with_english(|s| (s.replacement("the"), s.replacement("qwzxv")))
        else {
            eprintln!("skipped: no en-US spell checker");
            return;
        };
        assert_eq!(right, None);
        assert_eq!(unsure, None, "GET_SUGGESTIONS is not a sure fix");
    }
}
