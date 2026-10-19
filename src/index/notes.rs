//! What the index knows about each note: its title, aliases, tags, type, headings and a snippet,
//! and how often each word appears in each of those fields. Kept by size and write time, so a
//! rebuild reads only the notes that changed, and cached on disk between runs.

use super::scan::{ScanEntry, is_note};
use super::words;
use crate::notebook::frontmatter::{self, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Notes larger than this are left out of the index.
pub const MAX_NOTE_BYTES: u64 = 2 * 1024 * 1024;
/// A note's snippet holds this many characters of its text at most.
pub const SNIPPET_CHARS: usize = 300;
const CACHE_VERSION: &str = "1";

/// The parts of a note searched separately, each with its own weight.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Field {
    Title,
    Aliases,
    Tags,
    Headings,
    Text,
}

pub const FIELDS: [Field; 5] = [
    Field::Title,
    Field::Aliases,
    Field::Tags,
    Field::Headings,
    Field::Text,
];

impl Field {
    fn from_index(index: u8) -> Option<Self> {
        FIELDS.get(usize::from(index)).copied()
    }
}

/// How often one word appears in one field of a note.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WordCount {
    /// The word's id in the index's `Vocabulary`.
    pub word: u32,
    pub field: Field,
    pub count: u16,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexedNote {
    /// Relative to the notebook.
    pub path: PathBuf,
    pub size: u64,
    /// FILETIME ticks, as the scan reports them.
    pub mtime: u64,
    /// The first `# ` heading, else the file name without its extension.
    pub title: String,
    pub aliases: Vec<String>,
    /// From the front matter and the text, without `#`, each once, as first spelled.
    pub tags: Vec<String>,
    /// The front matter's `type:`.
    pub kind: Option<String>,
    /// Every heading but the title.
    pub headings: Vec<String>,
    /// The start of the text, on one line.
    pub snippet: String,
    /// Sorted by word id, then field.
    pub words: Vec<WordCount>,
}

/// Every word the index has seen, each with a number, so notes keep numbers rather than strings.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Vocabulary {
    ids: HashMap<Box<str>, u32>,
    words: Vec<Box<str>>,
}

impl Vocabulary {
    /// The id of `word`, which is added when new.
    pub fn intern(&mut self, word: &str) -> u32 {
        if let Some(&id) = self.ids.get(word) {
            return id;
        }
        let id = self.words.len() as u32;
        self.words.push(word.into());
        self.ids.insert(word.into(), id);
        id
    }

    /// The id of `word`, when some note has used it.
    pub fn get(&self, word: &str) -> Option<u32> {
        self.ids.get(word).copied()
    }

    pub fn word(&self, id: u32) -> &str {
        &self.words[id as usize]
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
}

#[derive(Clone, Debug, Default)]
pub struct NoteIndex {
    /// By lowercase relative path.
    pub notes: HashMap<String, IndexedNote>,
    /// Words of notes since removed stay until the index is next read from its cache.
    pub vocabulary: Vocabulary,
}

/// The key `NoteIndex::notes` uses for `path`.
pub fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

impl NoteIndex {
    /// The index of `entries` (a scan of `notebook`), reusing `previous` notes whose size and
    /// write time still match and reading the rest. Also returns whether anything differs from
    /// `previous`.
    pub fn build(notebook: &Path, entries: &[ScanEntry], previous: NoteIndex) -> (NoteIndex, bool) {
        let NoteIndex {
            notes: mut known,
            mut vocabulary,
        } = previous;
        let before = known.len();
        let mut notes = HashMap::with_capacity(entries.len());
        let mut changed = false;
        for entry in entries {
            if !is_note(&entry.path) || entry.size > MAX_NOTE_BYTES {
                continue;
            }
            let key = path_key(&entry.path);
            if let Some(note) = known.remove(&key)
                && note.size == entry.size
                && note.mtime == entry.mtime
                && note.path == entry.path
            {
                notes.insert(key, note);
                continue;
            }
            changed = true;
            // A note that can't be read now is left out; the next build tries again.
            let Some(text) = std::fs::read(notebook.join(&entry.path))
                .ok()
                .and_then(|bytes| crate::extract::text::decode(&bytes))
            else {
                continue;
            };
            let note = read_note(&entry.path, entry.size, entry.mtime, &text, &mut vocabulary);
            notes.insert(key, note);
        }
        changed |= notes.len() != before;
        (NoteIndex { notes, vocabulary }, changed)
    }

    pub fn get(&self, relative: &Path) -> Option<&IndexedNote> {
        self.notes.get(&path_key(relative))
    }

    /// The cache file's text.
    pub fn encode(&self) -> String {
        let mut notes: Vec<&IndexedNote> = self.notes.values().collect();
        notes.sort_by(|a, b| a.path.cmp(&b.path));
        let mut output = format!("version={CACHE_VERSION}\r\n");
        for note in notes {
            output.push_str(&format!(
                "note={}|{}|{}\r\n",
                note.size,
                note.mtime,
                escape(&note.path.to_string_lossy())
            ));
            output.push_str(&format!("title={}\r\n", escape(&note.title)));
            for alias in &note.aliases {
                output.push_str(&format!("alias={}\r\n", escape(alias)));
            }
            for tag in &note.tags {
                output.push_str(&format!("tag={}\r\n", escape(tag)));
            }
            if let Some(kind) = &note.kind {
                output.push_str(&format!("type={}\r\n", escape(kind)));
            }
            for heading in &note.headings {
                output.push_str(&format!("heading={}\r\n", escape(heading)));
            }
            output.push_str(&format!("snippet={}\r\n", escape(&note.snippet)));
            for field in FIELDS {
                let counts: Vec<String> = note
                    .words
                    .iter()
                    .filter(|count| count.field == field)
                    .map(|count| format!("{}:{}", self.vocabulary.word(count.word), count.count))
                    .collect();
                if !counts.is_empty() {
                    output.push_str(&format!("words={}|{}\r\n", field as u8, counts.join(" ")));
                }
            }
        }
        output
    }

    /// The index a cache file holds, or `None` for a file of another version or with a line
    /// that doesn't parse: the whole cache is thrown away then, and every note is read again.
    pub fn parse(source: &str) -> Option<NoteIndex> {
        let mut lines = source.lines();
        if lines.next()? != format!("version={CACHE_VERSION}") {
            return None;
        }
        let mut index = NoteIndex::default();
        let mut current: Option<IndexedNote> = None;
        let finish = |note: IndexedNote, index: &mut NoteIndex| {
            let mut note = note;
            note.words.sort_by_key(|count| (count.word, count.field));
            index.notes.insert(path_key(&note.path), note);
        };
        for line in lines {
            if line.is_empty() {
                continue;
            }
            let (key, value) = line.split_once('=')?;
            if key == "note" {
                if let Some(note) = current.take() {
                    finish(note, &mut index);
                }
                let mut fields = value.splitn(3, '|');
                current = Some(IndexedNote {
                    size: fields.next()?.parse().ok()?,
                    mtime: fields.next()?.parse().ok()?,
                    path: PathBuf::from(unescape(fields.next()?)?),
                    ..IndexedNote::default()
                });
                continue;
            }
            let note = current.as_mut()?;
            match key {
                "title" => note.title = unescape(value)?,
                "alias" => note.aliases.push(unescape(value)?),
                "tag" => note.tags.push(unescape(value)?),
                "type" => note.kind = Some(unescape(value)?),
                "heading" => note.headings.push(unescape(value)?),
                "snippet" => note.snippet = unescape(value)?,
                "words" => {
                    let (field, counts) = value.split_once('|')?;
                    let field = Field::from_index(field.parse().ok()?)?;
                    for pair in counts.split(' ') {
                        let (word, count) = pair.rsplit_once(':')?;
                        note.words.push(WordCount {
                            word: index.vocabulary.intern(word),
                            field,
                            count: count.parse().ok()?,
                        });
                    }
                }
                _ => return None,
            }
        }
        if let Some(note) = current {
            finish(note, &mut index);
        }
        Some(index)
    }
}

/// What `text`, the note at `path`, says about itself, its words added to `vocabulary`.
pub fn read_note(
    path: &Path,
    size: u64,
    mtime: u64,
    text: &str,
    vocabulary: &mut Vocabulary,
) -> IndexedNote {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let front = frontmatter::parse(text);
    let mut note = IndexedNote {
        path: path.to_path_buf(),
        size,
        mtime,
        ..IndexedNote::default()
    };
    let mut tags = Vec::new();
    if let Some(front) = &front {
        note.aliases = front.aliases();
        tags.extend(front.tags().into_iter().filter(|tag| is_tag(tag)));
        note.kind = match front.get("type") {
            Some(Value::Text(kind)) if !kind.trim().is_empty() => Some(kind.trim().to_owned()),
            _ => None,
        };
    }
    let body = &text[front.as_ref().map_or(0, |front| front.body_start)..];

    let mut title = None;
    let mut prose = String::new();
    let mut in_fence: Option<&str> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some(fence) = in_fence {
            if trimmed.starts_with(fence) {
                in_fence = None;
            } else {
                prose.push_str(line);
                prose.push('\n');
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = Some(&trimmed[..3]);
            continue;
        }
        if let Some((level, heading)) = heading_of(trimmed) {
            if level == 1 && title.is_none() && note.headings.is_empty() && prose.trim().is_empty()
            {
                title = Some(heading.to_owned());
            } else {
                note.headings.push(heading.to_owned());
            }
            continue;
        }
        tags.extend(
            inline_tags(&without_code_spans(line))
                .into_iter()
                .map(|(_, tag)| tag.to_owned()),
        );
        prose.push_str(line);
        prose.push('\n');
    }
    note.title = title.unwrap_or_else(|| {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    for tag in tags {
        if !note
            .tags
            .iter()
            .any(|known| known.eq_ignore_ascii_case(&tag))
        {
            note.tags.push(tag);
        }
    }
    note.snippet = snippet(&prose);

    let mut counts: HashMap<(u32, Field), u16> = HashMap::new();
    let mut count = |field: Field, text: &str, vocabulary: &mut Vocabulary| {
        words::words(text, |word| {
            let slot = counts.entry((vocabulary.intern(word), field)).or_default();
            *slot = slot.saturating_add(1);
        });
    };
    count(Field::Title, &note.title, vocabulary);
    for alias in &note.aliases {
        count(Field::Aliases, alias, vocabulary);
    }
    for tag in &note.tags {
        count(Field::Tags, tag, vocabulary);
    }
    for heading in &note.headings {
        count(Field::Headings, heading, vocabulary);
    }
    count(Field::Text, &prose, vocabulary);
    note.words = counts
        .into_iter()
        .map(|((word, field), count)| WordCount { word, field, count })
        .collect();
    note.words.sort_by_key(|count| (count.word, count.field));
    note
}

/// An ATX heading's level and text, without closing `#`s.
fn heading_of(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|&byte| byte == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &line[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim_end();
    (!text.is_empty()).then_some((level, text))
}

/// `line` with its `` `code` `` spans blanked, so tags inside them don't count.
fn without_code_spans(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut inside = false;
    for c in line.chars() {
        if c == '`' {
            inside = !inside;
            output.push(' ');
        } else {
            output.push(if inside { ' ' } else { c });
        }
    }
    output
}

/// Up to `SNIPPET_CHARS` characters of `prose`, its whitespace collapsed to single spaces.
fn snippet(prose: &str) -> String {
    let mut output = String::new();
    for (count, word) in prose.split_whitespace().enumerate() {
        if count > 0 {
            output.push(' ');
        }
        output.push_str(word);
        if output.chars().count() >= SNIPPET_CHARS {
            break;
        }
    }
    match output.char_indices().nth(SNIPPET_CHARS) {
        Some((cut, _)) => output[..cut].to_owned(),
        None => output,
    }
}

// Tag rules copied from FastPad `src/library/facts.rs`.

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '/')
}

/// Whether `name` (without `#`) is a tag: tag characters only, at least one letter.
pub fn is_tag(name: &str) -> bool {
    !name.is_empty() && name.chars().all(is_tag_char) && name.chars().any(char::is_alphabetic)
}

/// The tags in a run of prose, with each `#`'s byte offset in it: `#` at the start of the run or
/// after whitespace or `(`, then tag characters with at least one letter. A trailing `/` isn't
/// part of the tag.
pub fn inline_tags(run: &str) -> Vec<(usize, &str)> {
    let mut tags = Vec::new();
    let mut previous: Option<char> = None;
    for (at, c) in run.char_indices() {
        let starts = c == '#' && previous.is_none_or(|p| p.is_whitespace() || p == '(');
        previous = Some(c);
        if !starts {
            continue;
        }
        let rest = &run[at + 1..];
        let length = rest
            .char_indices()
            .find(|(_, c)| !is_tag_char(*c))
            .map_or(rest.len(), |(end, _)| end);
        let name = rest[..length].trim_end_matches('/');
        if is_tag(name) {
            tags.push((at, name));
        }
    }
    tags
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\p")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

fn unescape(value: &str) -> Option<String> {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            output.push(c);
            continue;
        }
        output.push(match chars.next()? {
            '\\' => '\\',
            'p' => '|',
            'r' => '\r',
            'n' => '\n',
            _ => return None,
        });
    }
    Some(output)
}

/// `%LocalAppData%\FastCapture\index\<folder key>.cache`, given `local` (the app's local data
/// folder): the same file for every spelling of one notebook folder.
pub fn cache_file(local: &Path, notebook: &Path) -> PathBuf {
    let folder: PathBuf = std::path::absolute(notebook)
        .unwrap_or_else(|_| notebook.to_path_buf())
        .components()
        .collect();
    let key = fnv1a(folder.to_string_lossy().to_lowercase().as_bytes());
    local.join("index").join(format!("{key:016x}.cache"))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The index cached at `path`, or an empty one when there is none or it can't be read.
pub fn read_cache(path: &Path) -> NoteIndex {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| NoteIndex::parse(&text))
        .unwrap_or_default()
}

pub fn write_cache(path: &Path, index: &NoteIndex) -> crate::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| crate::Error::io(parent, error))?;
    }
    crate::platform::saver::save_atomic(path, index.encode().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::scan::tests::Scratch;
    use std::collections::BTreeMap;

    /// A note's word counts by field and word, for comparing across vocabularies.
    fn counts(index: &NoteIndex, path: &str) -> BTreeMap<(Field, String), u16> {
        index
            .get(Path::new(path))
            .unwrap()
            .words
            .iter()
            .map(|count| {
                (
                    (count.field, index.vocabulary.word(count.word).to_owned()),
                    count.count,
                )
            })
            .collect()
    }

    fn entry(scratch: &Scratch, name: &str, text: &str, mtime: u64) -> ScanEntry {
        let path = scratch.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, text).unwrap();
        ScanEntry {
            path: PathBuf::from(name),
            size: text.len() as u64,
            mtime,
        }
    }

    #[test]
    fn a_note_gives_its_fields() {
        let mut vocabulary = Vocabulary::default();
        let note = read_note(
            Path::new(r"Payments\Lag.md"),
            0,
            0,
            "\u{feff}---\ntype: topic\naliases: [Consumer lag]\ntags: [kafka, \"#ops\"]\n---\n\
             # Ledger lag\n\nLag on `#not-a-tag` the #Kafka group, see #payments/incidents.\n\n\
             ## Checking lag ##\n\n```\n# not a heading #nor-a-tag\n```\n",
            &mut vocabulary,
        );
        assert_eq!(note.title, "Ledger lag");
        assert_eq!(note.kind.as_deref(), Some("topic"));
        assert_eq!(note.aliases, ["Consumer lag"]);
        assert_eq!(note.tags, ["kafka", "ops", "payments/incidents"]);
        assert_eq!(note.headings, ["Checking lag"]);
        assert!(
            note.snippet
                .starts_with("Lag on `#not-a-tag` the #Kafka group")
        );
        let word = |word: &str, field: Field| {
            let id = vocabulary.get(word).unwrap();
            note.words
                .iter()
                .find(|count| count.word == id && count.field == field)
                .map(|count| count.count)
        };
        assert_eq!(word("ledger", Field::Title), Some(1));
        assert_eq!(word("lag", Field::Text), Some(1));
        assert_eq!(word("incidents", Field::Tags), Some(1));
        assert_eq!(word("heading", Field::Text), Some(1), "code is still text");
        assert_eq!(word("checking", Field::Headings), Some(1));
    }

    #[test]
    fn a_note_without_a_heading_is_titled_by_its_name() {
        let mut vocabulary = Vocabulary::default();
        let note = read_note(
            Path::new(r"inbox\Consumer lag spikes.md"),
            0,
            0,
            "Some text first.\n\n# Later heading\n",
            &mut vocabulary,
        );
        assert_eq!(note.title, "Consumer lag spikes");
        assert_eq!(note.headings, ["Later heading"]);
        let long = "word ".repeat(200);
        let note = read_note(Path::new("a.md"), 0, 0, &long, &mut vocabulary);
        assert_eq!(note.snippet.chars().count(), SNIPPET_CHARS);
    }

    #[test]
    fn a_build_reads_only_what_changed() {
        let scratch = Scratch::new("index-build");
        let mut entries = vec![
            entry(&scratch, "a.md", "# Alpha\n\nkafka", 1),
            entry(&scratch, r"sub\b.markdown", "# Beta", 1),
            entry(&scratch, "c.txt", "not a note", 1),
        ];
        let (index, changed) = NoteIndex::build(&scratch.0, &entries, NoteIndex::default());
        assert!(changed);
        assert_eq!(index.notes.len(), 2);
        assert_eq!(
            index.get(Path::new(r"SUB\B.markdown")).unwrap().title,
            "Beta"
        );

        // Same stamps: nothing is read again, even though the file changed on disk.
        std::fs::write(scratch.0.join("a.md"), "# Rewritten").unwrap();
        let (index, changed) = NoteIndex::build(&scratch.0, &entries, index);
        assert!(!changed);
        assert_eq!(index.get(Path::new("a.md")).unwrap().title, "Alpha");

        // One note edited, one added, one deleted.
        entries[0].mtime = 2;
        entries[1] = entry(&scratch, "d.md", "# Delta", 1);
        let (index, changed) = NoteIndex::build(&scratch.0, &entries, index);
        assert!(changed);
        assert_eq!(index.get(Path::new("a.md")).unwrap().title, "Rewritten");
        assert!(index.get(Path::new(r"sub\b.markdown")).is_none());
        assert_eq!(index.get(Path::new("d.md")).unwrap().title, "Delta");
        assert_eq!(index.notes.len(), 2);

        // A deletion alone is a change.
        let (index, changed) = NoteIndex::build(&scratch.0, &entries[..1], index);
        assert!(changed);
        assert_eq!(index.notes.len(), 1);
    }

    #[test]
    fn large_notes_are_left_out() {
        let scratch = Scratch::new("index-large");
        let mut big = entry(&scratch, "big.md", "# Big", 1);
        big.size = MAX_NOTE_BYTES + 1;
        let (index, _) = NoteIndex::build(&scratch.0, &[big], NoteIndex::default());
        assert!(index.notes.is_empty());
    }

    #[test]
    fn the_cache_round_trips_and_a_bad_one_is_discarded() {
        let scratch = Scratch::new("index-cache");
        let entries = vec![
            entry(
                &scratch,
                r"sub\a b.md",
                "---\ntype: topic\naliases: [A | B]\ntags: [x]\n---\n# Title\n\n## Head\n\nBody \\ #y\n",
                7,
            ),
            entry(&scratch, "plain.md", "", 3),
        ];
        let (index, _) = NoteIndex::build(&scratch.0, &entries, NoteIndex::default());
        let path = cache_file(&scratch.0.join("local"), &scratch.0);
        write_cache(&path, &index).unwrap();
        let read = read_cache(&path);
        assert_eq!(read.notes.len(), 2);
        for path in [r"sub\a b.md", "plain.md"] {
            let (mut before, mut after) = (
                index.get(Path::new(path)).unwrap().clone(),
                read.get(Path::new(path)).unwrap().clone(),
            );
            assert_eq!(counts(&index, path), counts(&read, path));
            before.words.clear();
            after.words.clear();
            assert_eq!(before, after);
        }

        // Read from the cache, unchanged notes aren't read again.
        let (_, changed) = NoteIndex::build(&scratch.0, &entries, read);
        assert!(!changed);

        for bad in [
            "version=0\r\n",
            "version=1\r\ntitle=x\r\n",
            "version=1\r\nnote=1|2|a.md\r\nwhat=ever\r\n",
            "version=1\r\nnote=1|2|a.md\r\nwords=9|kafka:1\r\n",
            "version=1\r\nnote=1|2|a.md\r\nwords=0|kafka\r\n",
            "version=1\r\nnote=x|2|a.md\r\n",
        ] {
            assert!(NoteIndex::parse(bad).is_none(), "{bad:?}");
        }
        std::fs::write(&path, "version=1\r\nnote=x\r\n").unwrap();
        assert!(read_cache(&path).notes.is_empty());
        assert!(
            read_cache(&scratch.0.join("missing.cache"))
                .notes
                .is_empty()
        );
    }

    #[test]
    fn the_cache_file_is_the_same_for_every_spelling_of_a_folder() {
        let local = Path::new(r"C:\Local");
        assert_eq!(
            cache_file(local, Path::new(r"C:\Notes\Work")),
            cache_file(local, Path::new(r"c:\notes\work\"))
        );
        assert_ne!(
            cache_file(local, Path::new(r"C:\Notes\Work")),
            cache_file(local, Path::new(r"C:\Notes\Home"))
        );
        assert!(cache_file(local, Path::new(r"C:\Notes")).starts_with(r"C:\Local\index"));
    }
}
