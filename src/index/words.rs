//! Words for the index and its queries: runs of letters and digits, lowercase, with accents
//! folded, so `Café` and `cafe` match. `snake_case` and `kebab-case` words count whole and as
//! their parts. Pure.

/// Words shorter than this (in characters) are left out.
const MIN_CHARS: usize = 2;
/// Longer runs are hashes, encoded data or noise, and are left out.
const MAX_CHARS: usize = 64;

/// Common English words, and the parts of web addresses, which say nothing about a note.
const STOP: [&str; 64] = [
    "about", "after", "all", "also", "an", "and", "any", "are", "as", "at", "be", "been", "but",
    "by", "can", "com", "could", "did", "do", "does", "for", "from", "had", "has", "have", "he",
    "her", "his", "how", "http", "https", "if", "in", "into", "is", "it", "its", "my", "no", "not",
    "of", "on", "or", "our", "she", "so", "than", "that", "the", "their", "them", "then", "there",
    "they", "this", "to", "was", "we", "were", "what", "when", "which", "with", "www",
];

/// Calls `emit` with each word of `text`, in order, repeats included.
pub fn words(text: &str, mut emit: impl FnMut(&str)) {
    let mut whole = String::new();
    // Where each part of `whole` ends, when it has connectors.
    let mut part_ends: Vec<(usize, usize)> = Vec::new();
    let mut part_start = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_alphanumeric() {
            fold(c, &mut whole);
            continue;
        }
        let joins = matches!(c, '_' | '-')
            && part_start < whole.len()
            && chars.peek().is_some_and(|next| next.is_alphanumeric());
        if joins {
            part_ends.push((part_start, whole.len()));
            whole.push(c);
            part_start = whole.len();
            continue;
        }
        flush(&mut whole, &mut part_ends, &mut emit);
        part_start = 0;
    }
    flush(&mut whole, &mut part_ends, &mut emit);
}

/// The words of `text`, for tests and queries.
pub fn word_list(text: &str) -> Vec<String> {
    let mut list = Vec::new();
    words(text, |word| list.push(word.to_owned()));
    list
}

/// Emits the word in `whole` and, when it has connectors, each of its parts, then clears both.
fn flush(whole: &mut String, part_ends: &mut Vec<(usize, usize)>, emit: &mut impl FnMut(&str)) {
    if whole.is_empty() {
        return;
    }
    if !part_ends.is_empty() {
        let last_start = part_ends.last().map_or(0, |&(_, end)| end + 1);
        part_ends.push((last_start, whole.len()));
        if keeps(whole) {
            emit(whole);
        }
        for &(start, end) in part_ends.iter() {
            let part = &whole[start..end];
            if keeps(part) {
                emit(part);
            }
        }
    } else if keeps(whole) {
        emit(whole);
    }
    whole.clear();
    part_ends.clear();
}

fn keeps(word: &str) -> bool {
    let chars = word.chars().count();
    (MIN_CHARS..=MAX_CHARS).contains(&chars) && STOP.binary_search(&word).is_err()
}

/// Pushes `c` lowercase, without its accent.
fn fold(c: char, out: &mut String) {
    for lower in c.to_lowercase() {
        let plain = match lower {
            'à'..='å' | 'ā' | 'ă' | 'ą' => 'a',
            'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
            'ď' | 'đ' | 'ð' => 'd',
            'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
            'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
            'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
            'ł' | 'ľ' | 'ĺ' | 'ļ' => 'l',
            'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
            'ò'..='ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
            'ŕ' | 'ř' => 'r',
            'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => 's',
            'ţ' | 'ť' | 'ț' => 't',
            'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
            'ý' | 'ÿ' => 'y',
            'ź' | 'ż' | 'ž' => 'z',
            'æ' => {
                out.push_str("ae");
                continue;
            }
            'œ' => {
                out.push_str("oe");
                continue;
            }
            'ß' => {
                out.push_str("ss");
                continue;
            }
            'þ' => {
                out.push_str("th");
                continue;
            }
            other => other,
        };
        out.push(plain);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stop_list_is_sorted_for_its_search() {
        assert!(STOP.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn words_are_lowercase_folded_and_filtered() {
        assert_eq!(
            word_list("The Café in Brașov, and Ærøskøbing! Straße 2026 x 7"),
            ["cafe", "brasov", "aeroskobing", "strasse", "2026"]
        );
    }

    #[test]
    fn snake_and_kebab_words_count_whole_and_in_parts() {
        assert_eq!(
            word_list("ledger-events max_poll_records"),
            [
                "ledger-events",
                "ledger",
                "events",
                "max_poll_records",
                "max",
                "poll",
                "records"
            ]
        );
        // A connector not between letters or digits only separates.
        assert_eq!(word_list("_lag_ -- re- -try"), ["lag", "re", "try"]);
        // Stop words and short parts are dropped from the parts, not from the whole.
        assert_eq!(
            word_list("state-of-the-art"),
            ["state-of-the-art", "state", "art"]
        );
    }

    #[test]
    fn web_addresses_lose_their_common_parts() {
        assert_eq!(
            word_list("https://www.dailyledger.example/tech/2026"),
            ["dailyledger", "example", "tech", "2026"]
        );
    }

    #[test]
    fn very_long_runs_are_left_out() {
        let hash = "a".repeat(MAX_CHARS + 1);
        assert_eq!(word_list(&format!("{hash} kafka")), ["kafka"]);
    }
}
