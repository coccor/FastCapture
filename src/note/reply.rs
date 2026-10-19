//! A model's reply as a note: parsed from JSON, then checked and capped so every note has the
//! same shape whatever the model wrote. Problems that make the reply unusable are a
//! `ReplyError`, which the runner sends back to the model once.

use serde_json::Value;

pub const TITLE_LIMIT: usize = 120;
pub const SUMMARY_LIMIT: usize = 300;
pub const LIST_LIMIT: usize = 12;
pub const TAG_LIMIT: usize = 5;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NoteJson {
    pub title: String,
    pub summary: String,
    pub body: String,
    pub key_points: Vec<String>,
    pub tasks: Vec<String>,
    pub tags: Vec<String>,
    pub links: Vec<Link>,
    pub home: Option<Home>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Link {
    pub note: String,
    /// A note the notebook doesn't have yet.
    pub new: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Home {
    /// `topic` or `folder`.
    pub kind: String,
    pub note: String,
    pub reason: String,
}

/// Why a reply can't be used, in words the model can act on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplyError(pub Vec<String>);

impl std::fmt::Display for ReplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.join("; "))
    }
}

/// The note in `text`: the JSON object itself, or the first `{` to the last `}` when a model
/// wrapped it in prose.
pub fn parse(text: &str) -> Result<NoteJson, ReplyError> {
    let value: Value = serde_json::from_str(text.trim())
        .or_else(|first| {
            let start = text.find('{');
            let end = text.rfind('}');
            match (start, end) {
                (Some(start), Some(end)) if start < end => serde_json::from_str(&text[start..=end]),
                _ => Err(first),
            }
        })
        .map_err(|error| ReplyError(vec![format!("the reply is not JSON: {error}")]))?;
    let object = value
        .as_object()
        .ok_or_else(|| ReplyError(vec!["the reply must be a JSON object".to_owned()]))?;
    let mut problems = Vec::new();
    let mut text_of = |key: &str| match object.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(_) => {
            problems.push(format!("\"{key}\" must be a string"));
            String::new()
        }
        None => {
            problems.push(format!("\"{key}\" is missing"));
            String::new()
        }
    };
    let title = text_of("title");
    let summary = text_of("summary");
    let body = text_of("body");
    let mut list_of = |key: &str| -> Vec<String> {
        match object.get(key) {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect(),
            Some(Value::Null) | None => Vec::new(),
            Some(_) => {
                problems.push(format!("\"{key}\" must be a list of strings"));
                Vec::new()
            }
        }
    };
    let key_points = list_of("key_points");
    let tasks = list_of("tasks");
    let tags = list_of("tags");
    let links = object
        .get("links")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(Link {
                        note: item.get("note")?.as_str()?.to_owned(),
                        new: item.get("new").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let home = object.get("home").and_then(|home| {
        Some(Home {
            kind: home.get("kind")?.as_str()?.to_owned(),
            note: home.get("note")?.as_str()?.to_owned(),
            reason: home
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        })
    });
    if title.trim().is_empty() && summary.trim().is_empty() && body.trim().is_empty() {
        problems.push("the title, summary and body are all empty".to_owned());
    }
    if !problems.is_empty() {
        return Err(ReplyError(problems));
    }
    Ok(NoteJson {
        title,
        summary,
        body,
        key_points,
        tasks,
        tags,
        links,
        home,
    })
}

/// What a note may link to and be filed under: the candidate notes the model was offered by
/// name, the home notes among them, and the folders offered (`Payments/Incidents/`). New notes
/// may be linked only when candidates were offered.
#[derive(Clone, Debug, Default)]
pub struct Allowed<'a> {
    pub notes: &'a [String],
    pub home_notes: &'a [String],
    pub home_folders: &'a [String],
}

impl Allowed<'_> {
    /// The offered spelling of the candidate `name` names.
    fn note(&self, name: &str) -> Option<&String> {
        self.notes
            .iter()
            .find(|candidate| candidate.eq_ignore_ascii_case(name.trim()))
    }

    /// The home `home` names, as offered: a home note, else a folder.
    fn home(&self, home: &Home) -> Option<Home> {
        let note = home.note.trim();
        let note = note
            .strip_prefix("[[")
            .and_then(|note| note.strip_suffix("]]"))
            .unwrap_or(note);
        let found_note = self
            .home_notes
            .iter()
            .find(|candidate| candidate.eq_ignore_ascii_case(note));
        let found = match found_note {
            Some(name) => Some((HOME_TOPIC, name)),
            None => crate::file::home::normalize_folder(note).and_then(|folder| {
                self.home_folders
                    .iter()
                    .find(|candidate| candidate.eq_ignore_ascii_case(&folder))
                    .map(|name| (HOME_FOLDER, name))
            }),
        };
        found.map(|(kind, name)| Home {
            kind: kind.to_owned(),
            note: name.clone(),
            reason: home.reason.trim().to_owned(),
        })
    }
}

/// `Home::kind` for a home note.
pub const HOME_TOPIC: &str = "topic";
/// `Home::kind` for a folder.
pub const HOME_FOLDER: &str = "folder";

/// `note` with every field trimmed and capped, tags cleaned, and links and the home kept only
/// when `allowed`. An empty title becomes `fallback_title`.
pub fn check(note: NoteJson, fallback_title: &str, allowed: &Allowed<'_>) -> NoteJson {
    let line = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title = cap(&line(&note.title), TITLE_LIMIT);
    if title.is_empty() {
        title = cap(&line(fallback_title), TITLE_LIMIT);
    }
    let list = |items: Vec<String>| -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        for item in items {
            let item = line(&item);
            let item = item.trim_start_matches(['-', '*', '•']).trim().to_owned();
            if !item.is_empty() && !seen.contains(&item) {
                seen.push(item);
            }
        }
        seen.truncate(LIST_LIMIT);
        seen
    };
    let mut tags: Vec<String> = Vec::new();
    for tag in note.tags.iter().filter_map(|tag| clean_tag(tag)) {
        if !tags.iter().any(|known| known.eq_ignore_ascii_case(&tag)) {
            tags.push(tag);
        }
    }
    tags.truncate(TAG_LIMIT);
    let mut links: Vec<Link> = Vec::new();
    for link in note.links {
        let name = line(&link.note);
        let name = name
            .strip_prefix("[[")
            .and_then(|name| name.strip_suffix("]]"))
            .unwrap_or(&name)
            .trim()
            .to_owned();
        // A note offered is linked as offered, even when the model marked it new.
        let kept = match allowed.note(&name) {
            Some(candidate) => Some(Link {
                note: candidate.clone(),
                new: false,
            }),
            None if link.new && !allowed.notes.is_empty() && is_new_name(&name) => Some(Link {
                note: name,
                new: true,
            }),
            None => None,
        };
        if let Some(kept) = kept
            && !links
                .iter()
                .any(|known| known.note.eq_ignore_ascii_case(&kept.note))
        {
            links.push(kept);
        }
    }
    links.truncate(LIST_LIMIT);
    let home = note.home.as_ref().and_then(|home| allowed.home(home));
    NoteJson {
        title,
        summary: cap(&line(&note.summary), SUMMARY_LIMIT),
        body: note.body.trim().to_owned(),
        key_points: list(note.key_points),
        tasks: list(note.tasks),
        tags,
        links,
        home,
    }
}

/// Whether `name` can be a new note's wikilink: a name, not a path or a link with parts.
fn is_new_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= TITLE_LIMIT
        && !name.contains(['[', ']', '|', '#', '/', '\\', '^'])
}

/// `text` cut to at most `limit` characters, at a word boundary when there is one, with `…`.
fn cap(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let cut: String = text.chars().take(limit - 1).collect();
    let cut = match cut.rfind(' ') {
        Some(space) if space > cut.len() / 2 => &cut[..space],
        _ => cut.as_str(),
    };
    format!("{}…", cut.trim_end())
}

/// A tag FastPad reads as an inline tag: letters, digits, `_`, `-` and `/`, with at least one
/// letter. Spaces become `-`; a leading `#` is dropped; anything else is removed.
pub fn clean_tag(tag: &str) -> Option<String> {
    let tag = tag.trim().trim_start_matches('#');
    let cleaned: String = tag
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'))
        .collect();
    let cleaned = cleaned.trim_matches(['-', '/']).to_owned();
    cleaned.chars().any(char::is_alphabetic).then_some(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(fields: &str) -> String {
        format!(
            r#"{{"title":"T","summary":"S","body":"B","key_points":[],"tasks":[],"tags":[],"links":[],"home":null{fields}}}"#
        )
    }

    #[test]
    fn a_good_reply_parses() {
        let note = parse(&reply("")).unwrap();
        assert_eq!(
            (
                note.title.as_str(),
                note.summary.as_str(),
                note.body.as_str()
            ),
            ("T", "S", "B")
        );
        let wrapped = format!("Here is the note:\n```json\n{}\n```", reply(""));
        assert_eq!(parse(&wrapped).unwrap().title, "T", "prose around the JSON");
    }

    #[test]
    fn broken_replies_name_their_problems() {
        assert!(parse("not json").unwrap_err().0[0].starts_with("the reply is not JSON"));
        assert_eq!(
            parse("[1]").unwrap_err().0,
            ["the reply must be a JSON object"]
        );
        let problems = parse(r#"{"title": 5, "summary": "", "body": "", "tags": "x"}"#)
            .unwrap_err()
            .0;
        assert!(
            problems.contains(&"\"title\" must be a string".to_owned()),
            "{problems:?}"
        );
        assert!(problems.contains(&"\"tags\" must be a list of strings".to_owned()));
        assert!(problems.contains(&"the title, summary and body are all empty".to_owned()));
    }

    #[test]
    fn checking_caps_cleans_and_drops_what_isnt_allowed() {
        let note = NoteJson {
            title: format!("  {}  ", "word ".repeat(40)),
            summary: "one\nsentence".to_owned(),
            body: "\n body \n".to_owned(),
            key_points: vec!["- a".into(), "a".into(), " ".into(), "b".into()],
            tasks: (0..20).map(|n| format!("task {n}")).collect(),
            tags: vec![
                "#Payments".into(),
                "payments".into(),
                "ledger batch".into(),
                "2026".into(),
                "c#".into(),
                "a/b/".into(),
                "x".into(),
                "y".into(),
            ],
            links: vec![
                Link {
                    note: "Payments incidents".into(),
                    new: false,
                },
                Link {
                    note: "Invented".into(),
                    new: false,
                },
                Link {
                    note: "Ledger batch".into(),
                    new: true,
                },
            ],
            home: Some(Home {
                kind: "topic".into(),
                note: "Elsewhere".into(),
                reason: "r".into(),
            }),
        };
        let candidates = vec!["payments incidents".to_owned()];
        let checked = check(
            note.clone(),
            "fallback",
            &Allowed {
                notes: &candidates,
                ..Allowed::default()
            },
        );
        assert!(checked.title.chars().count() <= TITLE_LIMIT);
        assert!(checked.title.ends_with('…') && !checked.title.contains("  "));
        assert_eq!(checked.summary, "one sentence");
        assert_eq!(checked.body, "body");
        assert_eq!(checked.key_points, ["a", "b"]);
        assert_eq!(checked.tasks.len(), LIST_LIMIT);
        assert_eq!(checked.tags, ["Payments", "ledger-batch", "c", "a/b", "x"]);
        assert_eq!(
            checked.links,
            [
                Link {
                    note: "payments incidents".into(),
                    new: false
                },
                Link {
                    note: "Ledger batch".into(),
                    new: true
                }
            ]
        );
        assert_eq!(checked.home, None);

        let none = check(note, "fallback", &Allowed::default());
        assert!(
            none.links.is_empty(),
            "no candidates, no links, not even new ones"
        );
        let empty_title = check(
            NoteJson {
                title: " ".into(),
                ..NoteJson::default()
            },
            "lag spikes during",
            &Allowed::default(),
        );
        assert_eq!(empty_title.title, "lag spikes during");
    }

    #[test]
    fn links_are_kept_as_offered_once_and_new_ones_only_as_names() {
        let link = |note: &str, new: bool| Link {
            note: note.to_owned(),
            new,
        };
        let candidates = ["Payments/Lag".to_owned(), "Kafka".to_owned()];
        let checked = check(
            NoteJson {
                title: "t".into(),
                links: vec![
                    link("[[payments/lag]]", false),
                    link("Payments/Lag", false),
                    link("kafka", true),
                    link("Search/Invented", true),
                    link("Consumer groups", true),
                    link("Made up", false),
                ],
                ..NoteJson::default()
            },
            "",
            &Allowed {
                notes: &candidates,
                ..Allowed::default()
            },
        );
        assert_eq!(
            checked.links,
            [
                link("Payments/Lag", false),
                link("Kafka", false),
                link("Consumer groups", true)
            ]
        );
    }

    #[test]
    fn a_home_is_kept_when_it_names_a_home_note_or_a_folder() {
        let home = |kind: &str, note: &str| {
            Some(Home {
                kind: kind.to_owned(),
                note: note.to_owned(),
                reason: " fits ".to_owned(),
            })
        };
        let notes = ["Payments incidents".to_owned(), "Kafka".to_owned()];
        let home_notes = ["Payments incidents".to_owned()];
        let folders = ["Payments/Incidents/".to_owned()];
        let allowed = Allowed {
            notes: &notes,
            home_notes: &home_notes,
            home_folders: &folders,
        };
        let checked = |suggested| {
            check(
                NoteJson {
                    title: "t".into(),
                    home: suggested,
                    ..NoteJson::default()
                },
                "",
                &allowed,
            )
            .home
        };
        assert_eq!(
            checked(home("topic", "[[payments incidents]]")),
            home("topic", "Payments incidents").map(|home| Home {
                reason: "fits".into(),
                ..home
            })
        );
        assert_eq!(
            checked(home("topic", r"payments\incidents")).map(|home| (home.kind, home.note)),
            Some(("folder".to_owned(), "Payments/Incidents/".to_owned())),
            "a folder named as a topic is still the folder"
        );
        assert_eq!(checked(home("topic", "Kafka")), None, "not a home type");
        assert_eq!(checked(home("folder", "Elsewhere/")), None);
        assert_eq!(checked(None), None);
    }

    #[test]
    fn tags_follow_fastpads_inline_tag_rule() {
        assert_eq!(
            clean_tag("#project/fastpad").as_deref(),
            Some("project/fastpad")
        );
        assert_eq!(clean_tag("2026-q4").as_deref(), Some("2026-q4"));
        assert_eq!(clean_tag("123"), None);
        assert_eq!(clean_tag("#"), None);
        assert_eq!(clean_tag("zăpadă").as_deref(), Some("zăpadă"));
    }
}
