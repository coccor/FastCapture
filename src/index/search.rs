//! Finding the notes a capture relates to: BM25 over each field of the index, with a note's
//! title, aliases and tags weighing more than its text. Words, not meaning: a note that says the
//! same thing in other words isn't found.

use super::notes::{FIELDS, Field, IndexedNote, NoteIndex};
use super::words;
use std::collections::HashMap;

const K1: f32 = 1.2;
const B: f32 = 0.75;
/// Candidates for one capture.
pub const CANDIDATES: usize = 30;
/// The notebook's tags the model is shown.
pub const TOP_TAGS: usize = 50;

fn weight(field: Field) -> f32 {
    match field {
        Field::Title | Field::Aliases => 3.0,
        Field::Tags => 2.0,
        Field::Headings => 1.5,
        Field::Text => 1.0,
    }
}

/// One note in a word's postings: which note, how often, in which field.
#[derive(Clone, Copy)]
struct Posting {
    note: u32,
    count: u16,
    field: Field,
}

/// An index ready to search: its notes in path order and, per word, the notes that use it.
pub struct Searcher {
    index: NoteIndex,
    keys: Vec<String>,
    /// By word id.
    postings: Vec<Vec<Posting>>,
    /// By word id, how many notes use the word.
    note_counts: Vec<u32>,
    /// Per note, words per field.
    lengths: Vec<[u32; FIELDS.len()]>,
    /// Words per field, averaged over the notes.
    average: [f32; FIELDS.len()],
}

#[derive(Clone, Copy, Debug)]
pub struct Hit<'a> {
    pub note: &'a IndexedNote,
    pub score: f32,
}

/// A tag, as most often spelled, and how many notes use it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TagCount {
    pub name: String,
    pub notes: usize,
}

impl Searcher {
    pub fn new(index: NoteIndex) -> Self {
        let mut keys: Vec<String> = index.notes.keys().cloned().collect();
        keys.sort();
        let mut postings: Vec<Vec<Posting>> = vec![Vec::new(); index.vocabulary.len()];
        let mut note_counts = vec![0u32; index.vocabulary.len()];
        let mut lengths = Vec::with_capacity(keys.len());
        let mut totals = [0f32; FIELDS.len()];
        for (number, key) in keys.iter().enumerate() {
            let note = &index.notes[key];
            let mut length = [0u32; FIELDS.len()];
            let mut last_word = None;
            for count in &note.words {
                postings[count.word as usize].push(Posting {
                    note: number as u32,
                    count: count.count,
                    field: count.field,
                });
                // Words are sorted by id, so each word's fields come together.
                if last_word != Some(count.word) {
                    note_counts[count.word as usize] += 1;
                    last_word = Some(count.word);
                }
                length[count.field as usize] += u32::from(count.count);
            }
            for (total, length) in totals.iter_mut().zip(length) {
                *total += length as f32;
            }
            lengths.push(length);
        }
        let notes = keys.len().max(1) as f32;
        let average = totals.map(|total| (total / notes).max(1.0));
        Self {
            index,
            keys,
            postings,
            note_counts,
            lengths,
            average,
        }
    }

    pub fn index(&self) -> &NoteIndex {
        &self.index
    }

    pub fn into_index(self) -> NoteIndex {
        self.index
    }

    /// The `limit` notes that best match `query`, best first; notes that match none of its
    /// words aren't returned.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit<'_>> {
        let mut ids: Vec<u32> = Vec::new();
        words::words(query, |word| {
            if let Some(id) = self.index.vocabulary.get(word) {
                ids.push(id);
            }
        });
        ids.sort_unstable();
        ids.dedup();

        let total = self.keys.len() as f32;
        let mut scores: HashMap<u32, f32> = HashMap::new();
        for id in ids {
            let postings = &self.postings[id as usize];
            if postings.is_empty() {
                continue;
            }
            let notes = self.note_counts[id as usize] as f32;
            let idf = (1.0 + (total - notes + 0.5) / (notes + 0.5)).ln();
            for posting in postings {
                let field = posting.field as usize;
                let count = f32::from(posting.count);
                let length = self.lengths[posting.note as usize][field] as f32;
                let norm = 1.0 - B + B * length / self.average[field];
                let score = idf * weight(posting.field) * count * (K1 + 1.0) / (count + K1 * norm);
                *scores.entry(posting.note).or_default() += score;
            }
        }
        let mut ranked: Vec<(u32, f32)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        ranked.truncate(limit);
        ranked
            .into_iter()
            .map(|(number, score)| Hit {
                note: &self.index.notes[&self.keys[number as usize]],
                score,
            })
            .collect()
    }

    /// The `limit` tags the most notes use, most used first, then by name; each spelled as most
    /// notes spell it.
    pub fn top_tags(&self, limit: usize) -> Vec<TagCount> {
        let mut tags: HashMap<String, HashMap<&str, usize>> = HashMap::new();
        for note in self.index.notes.values() {
            for tag in &note.tags {
                *tags
                    .entry(tag.to_lowercase())
                    .or_default()
                    .entry(tag.as_str())
                    .or_default() += 1;
            }
        }
        let mut counted: Vec<TagCount> = tags
            .into_values()
            .map(|spellings| {
                let notes = spellings.values().sum();
                let name = spellings
                    .into_iter()
                    .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0)))
                    .map(|(name, _)| name.to_owned())
                    .unwrap_or_default();
                TagCount { name, notes }
            })
            .collect();
        counted.sort_by(|a, b| {
            b.notes
                .cmp(&a.notes)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        counted.truncate(limit);
        counted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::notes::{Vocabulary, path_key, read_note};
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    fn searcher(notes: &[(&str, &str)]) -> Searcher {
        let mut index = NoteIndex::default();
        let mut vocabulary = Vocabulary::default();
        for (path, text) in notes {
            let note = read_note(Path::new(path), 0, 0, text, &mut vocabulary);
            index.notes.insert(path_key(Path::new(path)), note);
        }
        index.vocabulary = vocabulary;
        Searcher::new(index)
    }

    fn paths(hits: &[Hit]) -> Vec<PathBuf> {
        hits.iter().map(|hit| hit.note.path.clone()).collect()
    }

    #[test]
    fn a_title_match_beats_a_text_match() {
        let searcher = searcher(&[
            (
                "mentions.md",
                "# Weekly notes\n\nWe talked about consumer lag briefly.",
            ),
            (
                "Consumer lag.md",
                "# Consumer lag\n\nHow to read the LAG column.",
            ),
            ("other.md", "# Lunch\n\nSoup."),
        ]);
        let hits = searcher.search("Why is consumer lag growing?", 10);
        assert_eq!(
            paths(&hits),
            [
                PathBuf::from("Consumer lag.md"),
                PathBuf::from("mentions.md")
            ]
        );
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn a_rare_word_beats_a_common_one() {
        let mut owned: Vec<(String, String)> = (0..20)
            .map(|number| (format!("p{number}.md"), "payments payments".to_owned()))
            .collect();
        owned.push(("both.md".to_owned(), "payments ledger".to_owned()));
        owned.push(("ledger.md".to_owned(), "ledger".to_owned()));
        let borrowed: Vec<(&str, &str)> = owned
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
            .collect();
        let searcher = searcher(&borrowed);
        let hits = searcher.search("payments ledger", 3);
        // One `ledger` outweighs any number of `payments`, which nearly every note has.
        let mut first = paths(&hits[..2]);
        first.sort();
        assert_eq!(
            first,
            [PathBuf::from("both.md"), PathBuf::from("ledger.md")]
        );
    }

    #[test]
    fn aliases_and_tags_are_found_and_unknown_words_find_nothing() {
        let searcher = searcher(&[
            ("a.md", "---\naliases: [Kafka lag]\n---\nText."),
            ("b.md", "---\ntags: [observability]\n---\nText."),
        ]);
        assert_eq!(paths(&searcher.search("kafka", 5)), [PathBuf::from("a.md")]);
        assert_eq!(
            paths(&searcher.search("Observability", 5)),
            [PathBuf::from("b.md")]
        );
        assert!(searcher.search("zebra the and", 5).is_empty());
        assert!(searcher.search("", 5).is_empty());
    }

    #[test]
    fn ties_are_broken_by_path_and_the_limit_holds() {
        let searcher = searcher(&[("c.md", "kafka"), ("a.md", "kafka"), ("b.md", "kafka")]);
        assert_eq!(
            paths(&searcher.search("kafka", 2)),
            [PathBuf::from("a.md"), PathBuf::from("b.md")]
        );
    }

    #[test]
    fn top_tags_count_notes_and_keep_the_common_spelling() {
        let searcher = searcher(&[
            ("a.md", "#Kafka #ops"),
            ("b.md", "#kafka #ops #kafka"),
            ("c.md", "#kafka"),
            ("d.md", "#zebra #Alpha"),
        ]);
        let tags = searcher.top_tags(3);
        let summary: Vec<(&str, usize)> = tags
            .iter()
            .map(|tag| (tag.name.as_str(), tag.notes))
            .collect();
        assert_eq!(summary, [("kafka", 3), ("ops", 2), ("Alpha", 1)]);
    }

    #[test]
    fn five_thousand_notes_are_searched_quickly() {
        const TOPICS: [&str; 12] = [
            "kafka", "ledger", "payments", "consumer", "invoice", "deploy", "budget", "garden",
            "recipe", "travel", "lag", "incident",
        ];
        let mut index = NoteIndex::default();
        let mut vocabulary = Vocabulary::default();
        for number in 0..5_000usize {
            let mut text = format!("# Note {number} {}\n\n", TOPICS[number % TOPICS.len()]);
            for word in 0..120usize {
                text.push_str(TOPICS[(number * 7 + word * 3) % TOPICS.len()]);
                text.push_str(&format!(" w{} ", (number * 31 + word * 17) % 3_000));
            }
            let path = PathBuf::from(format!("n{number}.md"));
            let note = read_note(&path, 0, 0, &text, &mut vocabulary);
            index.notes.insert(path_key(&path), note);
        }
        index.vocabulary = vocabulary;
        let searcher = Searcher::new(index);
        let query = "Consumer lag on the payments ledger after the deploy, incident w42 w1999 \
                     kafka budget"
            .repeat(20);
        let start = Instant::now();
        let hits = searcher.search(&query, CANDIDATES);
        let elapsed = start.elapsed();
        assert_eq!(hits.len(), CANDIDATES);
        let limit = if cfg!(debug_assertions) { 200 } else { 20 };
        assert!(elapsed.as_millis() < limit, "{elapsed:?}");
    }
}
