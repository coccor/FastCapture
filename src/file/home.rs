//! Homes: where a note can be filed. A home is a note whose `type:` is one of the notebook's
//! `home_types` (a topic, a project, a map of content), or a folder. Pure.

use std::path::Path;

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

#[cfg(test)]
mod tests {
    use super::*;

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
