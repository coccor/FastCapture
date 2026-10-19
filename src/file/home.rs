//! Homes: where a note can be filed. A home is a note whose `type:` is one of the notebook's
//! `home_types` (a topic, a project, a map of content), or a folder; and the homes the picker
//! offers. Pure.

use super::Target;
use std::path::{Path, PathBuf};

/// Whether a note of type `kind` is a home under `home_types` (lowercase, from `capture.ini`).
pub fn is_home_type(kind: Option<&str>, home_types: &[String]) -> bool {
    kind.is_some_and(|kind| {
        home_types
            .iter()
            .any(|home| home.eq_ignore_ascii_case(kind.trim()))
    })
}

/// How a folder home is written: relative to the notebook, with `/` and a trailing `/`
/// (`Payments/Incidents/`).
pub fn folder_name(folder: &Path) -> String {
    let name = folder.to_string_lossy().replace('\\', "/");
    let name = name.trim_matches('/');
    format!("{name}/")
}

/// `name` as `folder_name` writes it, for comparing what a model wrote with the folders offered.
pub fn normalize_folder(name: &str) -> Option<String> {
    let name = name.trim().replace('\\', "/");
    let name = name.trim_matches('/');
    (!name.is_empty()).then(|| format!("{name}/"))
}

/// The folders holding the notes at `paths` (relative to the notebook), as homes: each once, in
/// the order first seen, without the notebook's root and `capture_folder` (where notes wait to
/// be filed).
pub fn home_folders<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
    capture_folder: &Path,
) -> Vec<String> {
    let inbox = normalize_folder(&capture_folder.to_string_lossy());
    let mut folders: Vec<String> = Vec::new();
    for path in paths {
        let Some(folder) = path
            .parent()
            .and_then(|parent| normalize_folder(&parent.to_string_lossy()))
        else {
            continue;
        };
        let same = |other: &String| other.eq_ignore_ascii_case(&folder);
        if inbox.as_ref().is_some_and(same) || folders.iter().any(same) {
            continue;
        }
        folders.push(folder);
    }
    folders
}

/// A home the picker offers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HomeOption {
    /// A home note's name, or a folder as `Payments/Incidents/`.
    pub label: String,
    /// `topic in Payments/`, `folder`.
    pub detail: String,
    pub target: Target,
}

/// Every home in `notes` (relative paths with their `type:`) and `folders` (relative), the
/// suggested one first, then home notes, then folders, each by name. The capture folder isn't
/// a home: notes wait there.
pub fn options<'a>(
    notes: impl IntoIterator<Item = (&'a Path, Option<&'a str>)>,
    folders: &[PathBuf],
    home_types: &[String],
    capture_folder: &Path,
    suggested: Option<&Target>,
) -> Vec<HomeOption> {
    let inbox = normalize_folder(&capture_folder.to_string_lossy());
    let mut topics: Vec<HomeOption> = notes
        .into_iter()
        .filter(|(_, kind)| is_home_type(*kind, home_types))
        .map(|(path, kind)| {
            let folder = path
                .parent()
                .and_then(|folder| normalize_folder(&folder.to_string_lossy()));
            HomeOption {
                label: path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                detail: match folder {
                    Some(folder) => format!("{} in {folder}", kind.unwrap_or("").trim()),
                    None => kind.unwrap_or("").trim().to_owned(),
                },
                target: Target::Topic(path.to_path_buf()),
            }
        })
        .collect();
    topics.sort_by_key(|option| option.label.to_lowercase());
    let mut places: Vec<HomeOption> = folders
        .iter()
        .filter(|folder| {
            normalize_folder(&folder.to_string_lossy()).is_some_and(|name| {
                !inbox
                    .as_ref()
                    .is_some_and(|inbox| inbox.eq_ignore_ascii_case(&name))
            })
        })
        .map(|folder| HomeOption {
            label: folder_name(folder),
            detail: "folder".to_owned(),
            target: Target::Folder(folder.clone()),
        })
        .collect();
    places.sort_by_key(|option| option.label.to_lowercase());
    let mut all: Vec<HomeOption> = topics.into_iter().chain(places).collect();
    if let Some(suggested) = suggested
        && let Some(at) = all
            .iter()
            .position(|option| same_target(&option.target, suggested))
    {
        let first = all.remove(at);
        all.insert(0, first);
    }
    all
}

fn same_target(a: &Target, b: &Target) -> bool {
    let key = |path: &Path| path.to_string_lossy().to_lowercase();
    match (a, b) {
        (Target::Folder(a), Target::Folder(b)) | (Target::Topic(a), Target::Topic(b)) => {
            key(a) == key(b)
        }
        _ => false,
    }
}

/// The options matching `typed`: every word of it in the label or detail, ignoring case.
pub fn filter(options: &[HomeOption], typed: &str) -> Vec<usize> {
    let words: Vec<String> = typed.split_whitespace().map(str::to_lowercase).collect();
    options
        .iter()
        .enumerate()
        .filter(|(_, option)| {
            let text = format!("{} {}", option.label, option.detail).to_lowercase();
            words.iter().all(|word| text.contains(word.as_str()))
        })
        .map(|(index, _)| index)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_put_the_suggestion_first_and_filter_as_you_type() {
        let notes = [
            (Path::new(r"Payments\Payments incidents.md"), Some("topic")),
            (Path::new(r"Learning\Rust.md"), Some("project")),
            (Path::new(r"Payments\Lag.md"), None),
            (Path::new("Index.md"), Some("moc")),
        ];
        let folders = [
            PathBuf::from("Payments"),
            PathBuf::from(r"Payments\Incidents"),
            PathBuf::from("Inbox"),
        ];
        let types = ["topic", "project", "moc"].map(str::to_owned);
        let suggested = Target::Folder(PathBuf::from(r"payments\incidents"));
        let options = options(
            notes,
            &folders,
            &types,
            Path::new("inbox"),
            Some(&suggested),
        );
        let labels: Vec<&str> = options.iter().map(|option| option.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Payments/Incidents/",
                "Index",
                "Payments incidents",
                "Rust",
                "Payments/"
            ]
        );
        assert_eq!(options[2].detail, "topic in Payments/");
        assert_eq!(options[1].detail, "moc");
        let found: Vec<&str> = filter(&options, "pay INC")
            .into_iter()
            .map(|index| options[index].label.as_str())
            .collect();
        assert_eq!(found, ["Payments/Incidents/", "Payments incidents"]);
        assert_eq!(filter(&options, "").len(), options.len());
        assert!(filter(&options, "zebra").is_empty());
    }

    #[test]
    fn home_types_match_without_regard_to_case() {
        let types = ["topic", "project", "moc"].map(str::to_owned);
        assert!(is_home_type(Some("Topic"), &types));
        assert!(is_home_type(Some(" moc "), &types));
        assert!(!is_home_type(Some("capture"), &types));
        assert!(!is_home_type(None, &types));
    }

    #[test]
    fn folders_are_written_with_slashes() {
        assert_eq!(
            folder_name(Path::new(r"Payments\Incidents")),
            "Payments/Incidents/"
        );
        assert_eq!(
            normalize_folder(r" Payments\Incidents "),
            Some("Payments/Incidents/".to_owned())
        );
        assert_eq!(normalize_folder("/Payments/"), Some("Payments/".to_owned()));
        assert_eq!(normalize_folder("/"), None);
    }

    #[test]
    fn candidates_folders_are_homes_but_the_root_and_inbox_are_not() {
        let paths = [
            Path::new(r"Payments\Incidents\Lag.md"),
            Path::new(r"payments\incidents\Rebalance.md"),
            Path::new(r"Inbox\Captured.md"),
            Path::new("Kafka.md"),
            Path::new(r"Payments\Overview.md"),
        ];
        assert_eq!(
            home_folders(paths, Path::new("inbox")),
            ["Payments/Incidents/", "Payments/"]
        );
    }
}
