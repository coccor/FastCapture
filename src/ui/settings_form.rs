//! What the Settings window edits, apart from the window: the form's values, checking them, and
//! writing them to `fastcapture.ini`. The API key never goes in the file.

use crate::config::settings::{CloudBackend, Mode, Settings, SettingsFile};
use crate::model::local::is_on_this_pc;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Form {
    pub notebook: String,
    pub mode: Option<Mode>,
    /// Cloud mode's backend, kept as the file has it.
    pub backend: CloudBackend,
    pub local_endpoint: String,
    pub local_model: String,
    pub claude_model: String,
    /// A key typed into the form; empty keeps the saved one.
    pub new_key: String,
    /// A key is already saved in Credential Manager.
    pub key_saved: bool,
    pub start_at_sign_in: bool,
    pub capture_source: bool,
}

impl Form {
    /// The form for `settings`.
    pub fn from_settings(settings: &Settings, key_saved: bool) -> Self {
        Self {
            notebook: settings
                .notebook
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            mode: settings.mode,
            backend: settings.backend,
            local_endpoint: settings.local_endpoint.clone().unwrap_or_default(),
            local_model: settings.local_model.clone().unwrap_or_default(),
            claude_model: settings.claude_model.clone(),
            new_key: String::new(),
            key_saved,
            start_at_sign_in: settings.start_at_sign_in,
            capture_source: settings.capture_source,
        }
    }

    /// What's wrong with the form, said for the user, or nothing.
    pub fn problem(&self) -> Option<String> {
        let notebook = self.notebook.trim();
        if notebook.is_empty() || !PathBuf::from(notebook).is_absolute() {
            return Some("Choose the notebook folder.".to_owned());
        }
        match self.mode {
            None => Some("Choose Local or Cloud.".to_owned()),
            Some(Mode::Local) => {
                let endpoint = self.local_endpoint.trim();
                if endpoint.is_empty() {
                    return Some("Choose a local model server.".to_owned());
                }
                if crate::net::http::parse_url(endpoint).is_err() {
                    return Some(format!("\"{endpoint}\" isn't an http:// address."));
                }
                if !is_on_this_pc(endpoint) {
                    return Some(format!(
                        "{endpoint} isn't on this PC. Local mode only uses servers on this PC."
                    ));
                }
                if self.local_model.trim().is_empty() {
                    return Some("Choose a local model.".to_owned());
                }
                None
            }
            Some(Mode::Cloud) => {
                if self.claude_model.trim().is_empty() {
                    return Some("Choose a Claude model.".to_owned());
                }
                if self.new_key.trim().is_empty() && !self.key_saved {
                    return Some("Enter your Claude API key.".to_owned());
                }
                None
            }
        }
    }

    /// Whether saving needs the first-use screen for Cloud mode: what is sent, and to whom.
    pub fn needs_cloud_consent(&self, settings: &Settings) -> bool {
        self.mode == Some(Mode::Cloud) && !settings.cloud_accepted
    }

    /// Writes the form into `file`. `cloud_accepted` is set when the first-use screen was
    /// accepted now.
    pub fn apply(&self, file: &mut SettingsFile, cloud_accepted_now: bool) {
        let notebook = PathBuf::from(self.notebook.trim());
        if file.settings().0.notebook.as_ref() != Some(&notebook) {
            file.choose_notebook(&notebook);
        }
        file.set(
            "mode",
            match self.mode {
                Some(Mode::Local) => "local",
                Some(Mode::Cloud) => "cloud",
                None => "",
            },
        );
        file.set("backend", self.backend.key());
        file.set("local_endpoint", self.local_endpoint.trim());
        file.set("local_model", self.local_model.trim());
        file.set("claude_model", self.claude_model.trim());
        if cloud_accepted_now {
            file.set("cloud_accepted", "true");
        }
        file.set("start_at_sign_in", bool_text(self.start_at_sign_in));
        file.set("capture_source", bool_text(self.capture_source));
    }
}

fn bool_text(on: bool) -> &'static str {
    if on { "true" } else { "false" }
}

/// What the first-use screen for Cloud mode says.
pub fn cloud_consent_text(model: &str) -> String {
    format!(
        "In Cloud mode, FastCapture sends each capture to Anthropic's Claude API ({model}) to write \
         its note: what you typed, the text of links and files you capture, your screenshots and \
         images, and, later, the titles of related notes in your notebook.\n\n\
         Nothing is sent before you save a capture, and nothing you didn't capture is sent. \
         API use is billed to your Anthropic account.\n\n\
         Send your captures to Anthropic?"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::parse;

    fn form() -> Form {
        Form {
            notebook: r"D:\Notes".into(),
            mode: Some(Mode::Local),
            backend: CloudBackend::Claude,
            local_endpoint: "http://localhost:11434".into(),
            local_model: "gemma3".into(),
            claude_model: "claude-sonnet-5-5".into(),
            ..Form::default()
        }
    }

    #[test]
    fn problems_are_named_one_at_a_time() {
        assert_eq!(form().problem(), None);
        let with = |change: fn(&mut Form)| {
            let mut form = form();
            change(&mut form);
            form.problem()
        };
        assert_eq!(
            with(|form| form.notebook.clear()).as_deref(),
            Some("Choose the notebook folder.")
        );
        assert_eq!(
            with(|form| form.mode = None).as_deref(),
            Some("Choose Local or Cloud.")
        );
        assert!(
            with(|form| form.local_endpoint = "http://192.168.1.9:11434".into())
                .unwrap()
                .contains("isn't on this PC")
        );
        assert!(
            with(|form| form.local_endpoint = "localhost".into())
                .unwrap()
                .contains("isn't an http://")
        );
        assert_eq!(
            with(|form| form.local_model.clear()).as_deref(),
            Some("Choose a local model.")
        );
        assert_eq!(
            with(|form| form.mode = Some(Mode::Cloud)).as_deref(),
            Some("Enter your Claude API key.")
        );
        assert_eq!(
            with(|form| {
                form.mode = Some(Mode::Cloud);
                form.key_saved = true;
            }),
            None
        );
    }

    #[test]
    fn the_form_round_trips_through_the_file_without_the_key() {
        let mut file = SettingsFile::from_text("# mine\r\ncapture_hotkey=Ctrl+Alt+K\r\n");
        let mut cloud = form();
        cloud.mode = Some(Mode::Cloud);
        cloud.new_key = "sk-ant-secret".into();
        cloud.claude_model = "claude-opus-5-5".into();
        cloud.capture_source = true;
        cloud.apply(&mut file, true);
        let text = file.text();
        assert!(!text.contains("sk-ant"), "{text}");
        assert!(text.starts_with("# mine\r\ncapture_hotkey=Ctrl+Alt+K\r\n"));
        let (settings, warnings) = parse(&text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(settings.mode, Some(Mode::Cloud));
        assert_eq!(settings.notebook, Some(PathBuf::from(r"D:\Notes")));
        assert_eq!(settings.claude_model, "claude-opus-5-5");
        assert!(settings.cloud_accepted && settings.capture_source);
        assert_eq!(
            Form::from_settings(&settings, true),
            Form {
                new_key: String::new(),
                key_saved: true,
                ..cloud
            }
        );
    }

    #[test]
    fn cloud_needs_consent_once() {
        let mut cloud = form();
        cloud.mode = Some(Mode::Cloud);
        assert!(cloud.needs_cloud_consent(&parse("").0));
        assert!(!cloud.needs_cloud_consent(&parse("cloud_accepted=true").0));
        assert!(!form().needs_cloud_consent(&parse("").0));
        assert!(
            cloud_consent_text("claude-sonnet-5-5").contains("Send your captures to Anthropic?")
        );
    }
}
