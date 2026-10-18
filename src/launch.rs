//! The command line (spec §5.1):
//!
//! ```text
//! fastcapture                       the tray app
//! fastcapture add ["text" | -] [--url URL]... [--attach PATH]... [--notebook PATH]
//! fastcapture --settings            open fastcapture.ini
//! fastcapture --version | --help
//! ```

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The most text `add` takes, from the argument or stdin.
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Launch {
    Tray,
    Add(AddArgs),
    Settings,
    Version,
    Help,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AddArgs {
    pub text: AddText,
    pub urls: Vec<String>,
    /// Absolute, made so against the caller's working folder.
    pub attachments: Vec<PathBuf>,
    pub notebook: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum AddText {
    #[default]
    None,
    Given(String),
    /// `-`: read stdin.
    Stdin,
}

pub const USAGE: &str = "Usage:
  fastcapture                     run in the tray
  fastcapture add [\"text\" | -] [--url URL]... [--attach PATH]... [--notebook PATH]
                                  capture text, links and files with no window
  fastcapture --settings          open the settings file
  fastcapture --version";

/// Reads the arguments after the program name. `cwd` makes relative paths absolute. The error
/// is a message for stderr, and means exit code 2.
pub fn parse(args: &[OsString], cwd: &Path) -> Result<Launch, String> {
    let text = |arg: &OsString| -> Result<String, String> {
        arg.to_str()
            .map(str::to_owned)
            .ok_or_else(|| "Arguments must be valid Unicode.".to_owned())
    };
    let Some(first) = args.first() else {
        return Ok(Launch::Tray);
    };
    let rest = &args[1..];
    let only = |launch: Launch| {
        if rest.is_empty() {
            Ok(launch)
        } else {
            Err(format!(
                "Unexpected argument: {}",
                rest[0].to_string_lossy()
            ))
        }
    };
    match text(first)?.as_str() {
        "add" => parse_add(rest, cwd).map(Launch::Add),
        "--settings" => only(Launch::Settings),
        "--version" | "-V" => only(Launch::Version),
        "--help" | "-h" | "/?" => only(Launch::Help),
        other => Err(format!("Unknown command: {other}\n\n{USAGE}")),
    }
}

fn parse_add(args: &[OsString], cwd: &Path) -> Result<AddArgs, String> {
    let mut add = AddArgs::default();
    let mut index = 0;
    let absolute = |path: &OsString| {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        }
    };
    while index < args.len() {
        let arg = &args[index];
        let value = |name: &str| {
            args.get(index + 1)
                .ok_or_else(|| format!("{name} needs a value."))
        };
        match arg.to_str() {
            Some("--url") => {
                let url = value("--url")?.to_string_lossy().into_owned();
                if !crate::capture::detect::is_url_line(&url) {
                    return Err(format!("Not an http or https URL: {url}"));
                }
                add.urls.push(url.trim().to_owned());
                index += 2;
            }
            Some("--attach") => {
                add.attachments.push(absolute(value("--attach")?));
                index += 2;
            }
            Some("--notebook") => {
                if add.notebook.is_some() {
                    return Err("--notebook can be given once.".to_owned());
                }
                add.notebook = Some(absolute(value("--notebook")?));
                index += 2;
            }
            Some(flag) if flag.starts_with("--") => {
                return Err(format!("Unknown option: {flag}"));
            }
            _ => {
                if add.text != AddText::None {
                    return Err(
                        "Give the text once (quote it if it has spaces), or - to read it from stdin."
                            .to_owned(),
                    );
                }
                add.text = match arg.to_str() {
                    Some("-") => AddText::Stdin,
                    Some(text) if text.len() > MAX_TEXT_BYTES => {
                        return Err(too_long());
                    }
                    Some(text) => AddText::Given(text.to_owned()),
                    None => return Err("Arguments must be valid Unicode.".to_owned()),
                };
                index += 1;
            }
        }
    }
    if add.text == AddText::None && add.urls.is_empty() && add.attachments.is_empty() {
        return Err(format!("Nothing to capture.\n\n{USAGE}"));
    }
    Ok(add)
}

pub fn too_long() -> String {
    format!(
        "The text is longer than {} MB.",
        MAX_TEXT_BYTES / (1024 * 1024)
    )
}

impl AddArgs {
    /// The capture's text: the given or read text, then each `--url` on a line of its own.
    pub fn capture_text(&self, text: &str) -> String {
        let mut lines: Vec<String> = Vec::new();
        if !text.trim().is_empty() {
            lines.push(text.trim_end().to_owned());
        }
        lines.extend(self.urls.iter().cloned());
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn cwd() -> PathBuf {
        PathBuf::from(r"C:\work")
    }

    #[test]
    fn no_arguments_is_the_tray_app_and_flags_stand_alone() {
        assert_eq!(parse(&[], &cwd()), Ok(Launch::Tray));
        assert_eq!(parse(&args(&["--settings"]), &cwd()), Ok(Launch::Settings));
        assert_eq!(parse(&args(&["--version"]), &cwd()), Ok(Launch::Version));
        assert_eq!(parse(&args(&["/?"]), &cwd()), Ok(Launch::Help));
        assert!(parse(&args(&["--version", "x"]), &cwd()).is_err());
        assert!(parse(&args(&["capture"]), &cwd()).is_err());
    }

    #[test]
    fn add_takes_text_links_files_and_a_notebook() {
        let launch = parse(
            &args(&[
                "add",
                "lag spikes",
                "--url",
                "https://grafana.example/d/x",
                "--attach",
                "ledger.log",
                "--attach",
                r"D:\shots\a.png",
                "--notebook",
                r"..\notes",
            ]),
            &cwd(),
        )
        .unwrap();
        assert_eq!(
            launch,
            Launch::Add(AddArgs {
                text: AddText::Given("lag spikes".to_owned()),
                urls: vec!["https://grafana.example/d/x".to_owned()],
                attachments: vec![
                    PathBuf::from(r"C:\work\ledger.log"),
                    PathBuf::from(r"D:\shots\a.png")
                ],
                notebook: Some(PathBuf::from(r"C:\work\..\notes")),
            })
        );
        let Launch::Add(add) = launch else {
            unreachable!()
        };
        assert_eq!(
            add.capture_text("lag spikes\n"),
            "lag spikes\nhttps://grafana.example/d/x"
        );
        assert_eq!(
            parse(&args(&["add", "-"]), &cwd()),
            Ok(Launch::Add(AddArgs {
                text: AddText::Stdin,
                ..AddArgs::default()
            }))
        );
    }

    #[test]
    fn bad_add_arguments_say_what_is_wrong() {
        let error = |list: &[&str]| parse(&args(list), &cwd()).unwrap_err();
        assert!(error(&["add"]).starts_with("Nothing to capture."));
        assert_eq!(error(&["add", "a", "b"]).split(' ').next(), Some("Give"));
        assert_eq!(error(&["add", "--attach"]), "--attach needs a value.");
        assert_eq!(
            error(&["add", "--url", "example.com"]),
            "Not an http or https URL: example.com"
        );
        assert_eq!(error(&["add", "x", "--force"]), "Unknown option: --force");
        assert_eq!(
            error(&["add", "x", "--notebook", "a", "--notebook", "b"]),
            "--notebook can be given once."
        );
        let long = "x".repeat(MAX_TEXT_BYTES + 1);
        assert_eq!(error(&["add", &long]), too_long());
    }
}
