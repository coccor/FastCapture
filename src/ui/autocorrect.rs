//! Which typed word autocorrect may fix, and how the fix keeps its case. Pure: the capture box
//! asks Windows' spell checker (`platform::spell`) for the replacement.

/// Characters that end a typed word: the box checks the word before them.
pub fn is_boundary(ch: u32) -> bool {
    matches!(
        char::from_u32(ch),
        Some(' ' | '\r' | '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '"' | '\'')
    )
}

/// Characters that make the letters after them part of a token (a version, a path, an email, a
/// tag) rather than a word.
fn joins_token(ch: char) -> bool {
    ch.is_ascii_digit() || matches!(ch, '_' | '/' | '\\' | '@' | '.' | ':' | '-' | '#')
}

/// The word that ends at the caret, given the text before it: letters and apostrophes, two or
/// more, not part of a token or a web address.
pub fn word_before(before: &str) -> Option<&str> {
    let start = before
        .char_indices()
        .rev()
        .take_while(|&(_, ch)| ch.is_alphabetic() || ch == '\'')
        .last()
        .map_or(before.len(), |(index, _)| index);
    let word = before[start..].trim_start_matches('\'');
    let start = before.len() - word.len();
    if word.chars().count() < 2 || !word.chars().any(char::is_alphabetic) {
        return None;
    }
    if before[..start].chars().next_back().is_some_and(joins_token) {
        return None;
    }
    let token = before
        .rfind(char::is_whitespace)
        .map_or(before, |space| &before[space + 1..])
        .to_ascii_lowercase();
    if token.contains("://") || token.starts_with("www.") {
        return None;
    }
    Some(word)
}

/// `replacement` in the case `original` was typed in: all capitals, a leading capital, or as is.
pub fn match_case(original: &str, replacement: &str) -> String {
    let letters = || original.chars().filter(|ch| ch.is_alphabetic());
    if letters().count() > 1 && letters().all(char::is_uppercase) {
        return replacement.to_uppercase();
    }
    if original.chars().next().is_some_and(char::is_uppercase) {
        let mut chars = replacement.chars();
        return chars
            .next()
            .map(|first| first.to_uppercase().chain(chars).collect())
            .unwrap_or_default();
    }
    replacement.to_owned()
}

/// A fix to make: the word at UTF-16 offset `start` is replaced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Correction {
    pub start: usize,
    pub original: String,
    pub replacement: String,
}

/// The fix for the word before the caret, when `lookup` (given the word as typed) has one. The
/// word goes as typed so Windows' own rules apply: it leaves words in all capitals alone, as
/// acronyms ("CNA" is not "can").
pub fn correction(before: &str, lookup: impl FnOnce(&str) -> Option<String>) -> Option<Correction> {
    let word = word_before(before)?;
    let replacement = match_case(word, &lookup(word)?);
    (replacement != word).then(|| Correction {
        start: before[..before.len() - word.len()].encode_utf16().count(),
        original: word.to_owned(),
        replacement,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_word_before_the_caret_is_a_run_of_letters_and_apostrophes() {
        assert_eq!(word_before("I think teh"), Some("teh"));
        assert_eq!(word_before("(teh"), Some("teh"));
        assert_eq!(word_before("dont"), Some("dont"));
        assert_eq!(word_before("I"), None, "one letter");
        assert_eq!(word_before(""), None);
        assert_eq!(word_before("teh "), None, "the caret is after a space");
    }

    #[test]
    fn tokens_paths_urls_and_tags_are_not_words() {
        for before in [
            "v2teh",
            "my_teh",
            r"C:\teh",
            "src/teh",
            "me@teh",
            "file.teh",
            "a:teh",
            "re-teh",
            "#teh",
            "https://example.com/teh",
            "www.example.com/teh",
        ] {
            assert_eq!(word_before(before), None, "{before}");
        }
    }

    #[test]
    fn a_fix_keeps_the_typed_case() {
        assert_eq!(match_case("teh", "the"), "the");
        assert_eq!(match_case("Teh", "the"), "The");
        assert_eq!(match_case("TEH", "the"), "THE");
    }

    #[test]
    fn a_correction_names_the_word_by_its_utf16_offset() {
        let fix = correction("café 😀 Teh", |w| (w == "Teh").then(|| "the".into())).unwrap();
        assert_eq!(fix.start, "café 😀 ".encode_utf16().count());
        assert_eq!(
            (fix.original.as_str(), fix.replacement.as_str()),
            ("Teh", "The")
        );
        assert!(correction("the", |_| Some("the".into())).is_none());
        assert!(correction("qwzxv", |_| None).is_none());
    }

    #[test]
    fn boundaries_are_space_enter_and_closing_punctuation() {
        for ch in " \r.,;:!?)]\"'".chars() {
            assert!(is_boundary(ch as u32), "{ch:?}");
        }
        for ch in "a1-_/\t\n".chars() {
            assert!(!is_boundary(ch as u32), "{ch:?}");
        }
    }
}
