//! What the Settings window edits, apart from the window: the form's values, checking them, and
//! writing them to `fastcapture.ini`. API keys never go in the file.

use crate::config::settings::{
    CloudBackend, Mode, Settings, SettingsFile, ThemePreference, parse_token_cap,
};
use crate::model::local::is_on_this_pc;
use crate::model::openai;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Form {
    pub notebook: String,
    pub mode: Option<Mode>,
    /// Cloud mode's provider.
    pub backend: CloudBackend,
    pub local_endpoint: String,
    pub local_model: String,
    pub claude_model: String,
    /// The OpenAI-compatible API's base URL and model.
    pub openai_endpoint: String,
    pub openai_model: String,
    /// Claude Code's model: an alias such as `sonnet`, or a full name.
    pub claude_code_model: String,
    /// Copilot's model: `auto`, or one the plan offers.
    pub copilot_model: String,
    /// A key typed into the form for the chosen provider; empty keeps the saved one.
    pub new_key: String,
    /// A key for the chosen provider is already saved in Credential Manager.
    pub key_saved: bool,
    /// The monthly token cap as typed: empty for none, `2000000`, `2M`, `500k`.
    pub token_cap: String,
    pub start_at_sign_in: bool,
    pub capture_source: bool,
    pub theme: ThemePreference,
}

/// A cap as the form shows it: `2M`, `500k`, or the plain count.
fn cap_text(cap: Option<u64>) -> String {
    match cap {
        None => String::new(),
        Some(cap) if cap.is_multiple_of(1_000_000) => format!("{}M", cap / 1_000_000),
        Some(cap) if cap.is_multiple_of(1_000) => format!("{}k", cap / 1_000),
        Some(cap) => cap.to_string(),
    }
}

impl Form {
    /// The form for `settings`. `key_saved` is for the provider the settings choose.
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
            openai_endpoint: settings.openai_endpoint.clone().unwrap_or_default(),
            openai_model: settings.openai_model.clone().unwrap_or_default(),
            claude_code_model: settings.claude_code_model.clone(),
            copilot_model: settings.copilot_model.clone(),
            new_key: String::new(),
            key_saved,
            token_cap: cap_text(settings.token_cap),
            start_at_sign_in: settings.start_at_sign_in,
            capture_source: settings.capture_source,
            theme: settings.theme,
        }
    }

    /// The OpenAI-compatible API's host, lowercase.
    pub fn openai_host(&self) -> String {
        openai::host(self.openai_endpoint.trim()).unwrap_or_default()
    }

    /// What's wrong with the form, said for the user, or nothing.
    pub fn problem(&self) -> Option<String> {
        let notebook = self.notebook.trim();
        if notebook.is_empty() || !PathBuf::from(notebook).is_absolute() {
            return Some("Choose the notebook folder.".to_owned());
        }
        if parse_token_cap(&self.token_cap).is_none() {
            return Some(
                "The token cap is a number of tokens, like 2000000 or 2M. Leave it empty for none."
                    .to_owned(),
            );
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
            Some(Mode::Cloud) => match self.backend {
                CloudBackend::Claude => {
                    if self.claude_model.trim().is_empty() {
                        return Some("Choose a Claude model.".to_owned());
                    }
                    if self.new_key.trim().is_empty() && !self.key_saved {
                        return Some("Enter your Claude API key.".to_owned());
                    }
                    None
                }
                CloudBackend::OpenAi => {
                    let endpoint = self.openai_endpoint.trim();
                    if endpoint.is_empty() {
                        return Some(
                            "Enter the API's address, like https://api.openai.com/v1.".to_owned(),
                        );
                    }
                    if !crate::net::http::parse_url(endpoint).is_ok_and(|target| target.secure) {
                        return Some(format!(
                            "\"{endpoint}\" isn't an https:// address. The key is only sent over https."
                        ));
                    }
                    if self.openai_model.trim().is_empty() {
                        return Some("Choose or type the model.".to_owned());
                    }
                    if self.new_key.trim().is_empty() && !self.key_saved {
                        return Some(format!("Enter your API key for {}.", self.openai_host()));
                    }
                    None
                }
                CloudBackend::ClaudeCode => {
                    let model = self.claude_code_model.trim();
                    if model.is_empty() || model.contains(char::is_whitespace) {
                        return Some("Choose or type a Claude Code model, like sonnet.".to_owned());
                    }
                    None
                }
                CloudBackend::Copilot => {
                    let model = self.copilot_model.trim();
                    if model.is_empty() || model.contains(char::is_whitespace) {
                        return Some("Choose or type a Copilot model, like auto.".to_owned());
                    }
                    None
                }
            },
        }
    }

    /// Whether saving needs the first-use screen for Cloud mode: what is sent, and to whom.
    /// Asked once for Anthropic, and again whenever the OpenAI-compatible host changes.
    pub fn needs_cloud_consent(&self, settings: &Settings) -> bool {
        if self.mode != Some(Mode::Cloud) {
            return false;
        }
        match self.backend {
            CloudBackend::Claude => !settings.cloud_accepted,
            CloudBackend::OpenAi => {
                settings.openai_accepted.as_deref() != Some(self.openai_host().as_str())
            }
            CloudBackend::ClaudeCode => !settings.claude_code_accepted,
            CloudBackend::Copilot => !settings.copilot_accepted,
        }
    }

    /// Writes the form into `file`. `cloud_accepted_now` is set when the first-use screen was
    /// accepted now, for the chosen provider.
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
        file.set("openai_endpoint", self.openai_endpoint.trim());
        file.set("openai_model", self.openai_model.trim());
        if !self.claude_code_model.trim().is_empty() {
            file.set("claude_code_model", self.claude_code_model.trim());
        }
        if !self.copilot_model.trim().is_empty() {
            file.set("copilot_model", self.copilot_model.trim());
        }
        if cloud_accepted_now {
            match self.backend {
                CloudBackend::Claude => file.set("cloud_accepted", "true"),
                CloudBackend::OpenAi => file.set("openai_accepted", &self.openai_host()),
                CloudBackend::ClaudeCode => file.set("claude_code_accepted", "true"),
                CloudBackend::Copilot => file.set("copilot_accepted", "true"),
            }
        }
        file.set(
            "token_cap",
            &cap_text(parse_token_cap(&self.token_cap).flatten()),
        );
        file.set("start_at_sign_in", bool_text(self.start_at_sign_in));
        file.set("capture_source", bool_text(self.capture_source));
        file.set("theme", self.theme.key());
    }

    /// The first-use screen's title and text, naming the chosen provider.
    pub fn consent(&self) -> (String, String) {
        match self.backend {
            CloudBackend::Claude => (
                "Send captures to Anthropic?".to_owned(),
                cloud_consent_text(
                    &format!("Anthropic's Claude API ({})", self.claude_model.trim()),
                    "your Anthropic account",
                    "Anthropic",
                ),
            ),
            CloudBackend::OpenAi => {
                let host = self.openai_host();
                (
                    format!("Send captures to {host}?"),
                    cloud_consent_text(
                        &format!("the API at {host} ({})", self.openai_model.trim()),
                        "your account with that provider",
                        &host,
                    ),
                )
            }
            CloudBackend::ClaudeCode => (
                "Send captures to Anthropic through Claude Code?".to_owned(),
                format!(
                    "In Cloud mode with Claude Code, FastCapture runs your own installed Claude \
                    Code for each capture, which sends it to Anthropic with your Claude plan's \
                    login ({}): what you typed, the text of links and files you capture, your \
                    screenshots and images, and short descriptions of related notes in your \
                    notebook. Claude Code runs with no tools, so it can't read or change \
                    anything else.\n\n\
                     Nothing is sent before you save a capture. Notes count against your plan's \
                     usage limits; there is no API bill.\n\n\
                     This is experimental and for your own use: Anthropic lets you use Claude \
                     Code with your own plan, but its terms don't say whether another app may \
                     start it for you, so this may have to change.\n\n\
                     Send your captures to Anthropic through Claude Code?",
                    self.claude_code_model.trim()
                ),
            ),
            CloudBackend::Copilot => {
                let model = self.copilot_model.trim();
                let model = if model.eq_ignore_ascii_case(crate::model::copilot::DEFAULT_MODEL) {
                    "the model Copilot chooses".to_owned()
                } else {
                    model.to_owned()
                };
                (
                    "Send captures to GitHub Copilot?".to_owned(),
                    format!(
                        "In Cloud mode with GitHub Copilot, FastCapture runs your own installed \
                         Copilot CLI for each capture, which sends it to GitHub with your Copilot \
                         login, and GitHub passes it on to the provider of {model}: what you \
                         typed, the text of links and files you capture, your screenshots and \
                         images, and short descriptions of related notes in your notebook. The \
                         CLI runs with no tools, so it can't read or change anything else.\n\n\
                         Nothing is sent before you save a capture. Notes use your Copilot \
                         plan's allowance; there is no API bill.\n\n\
                         This is experimental and for your own use: GitHub lets you use the \
                         Copilot CLI with your own plan, but its terms don't say whether another \
                         app may start it for you, so this may have to change.\n\n\
                         Send your captures to GitHub Copilot?"
                    ),
                )
            }
        }
    }
}

fn bool_text(on: bool) -> &'static str {
    if on { "true" } else { "false" }
}

/// What the first-use screen for Cloud mode says: `api` names where captures go, `billed` whose
/// account pays, and `to` who receives them.
fn cloud_consent_text(api: &str, billed: &str, to: &str) -> String {
    format!(
        "In Cloud mode, FastCapture sends each capture to {api} to write its note: what you \
         typed, the text of links and files you capture, your screenshots and images, and short \
         descriptions of related notes in your notebook.\n\n\
         Nothing is sent before you save a capture, and nothing you didn't capture is sent. \
         API use is billed to {billed}.\n\n\
         Send your captures to {to}?"
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
            claude_code_model: "sonnet".into(),
            copilot_model: "auto".into(),
            ..Form::default()
        }
    }

    fn openai() -> Form {
        Form {
            mode: Some(Mode::Cloud),
            backend: CloudBackend::OpenAi,
            openai_endpoint: "https://openrouter.ai/api/v1".into(),
            openai_model: "openai/gpt-5-mini".into(),
            key_saved: true,
            ..form()
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
        assert!(
            with(|form| form.token_cap = "lots".into())
                .unwrap()
                .starts_with("The token cap is a number")
        );
        assert_eq!(with(|form| form.token_cap = "2M".into()), None);
    }

    #[test]
    fn the_openai_compatible_api_needs_an_https_address_a_model_and_a_key() {
        assert_eq!(openai().problem(), None);
        let with = |change: fn(&mut Form)| {
            let mut form = openai();
            change(&mut form);
            form.problem()
        };
        assert!(
            with(|form| form.openai_endpoint.clear())
                .unwrap()
                .starts_with("Enter the API's address")
        );
        assert!(
            with(|form| form.openai_endpoint = "http://api.openai.com/v1".into())
                .unwrap()
                .contains("isn't an https:// address")
        );
        assert_eq!(
            with(|form| form.openai_model.clear()).as_deref(),
            Some("Choose or type the model.")
        );
        assert_eq!(
            with(|form| form.key_saved = false).as_deref(),
            Some("Enter your API key for openrouter.ai.")
        );
        assert_eq!(
            with(|form| {
                form.key_saved = false;
                form.new_key = "sk-or".into();
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
        cloud.token_cap = "500k".into();
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
        assert_eq!(settings.token_cap, Some(500_000));
        assert!(settings.cloud_accepted && settings.capture_source);
        assert_eq!(
            Form::from_settings(&settings, true),
            Form {
                new_key: String::new(),
                key_saved: true,
                ..cloud
            }
        );

        let mut file = SettingsFile::from_text("");
        let mut cloud = openai();
        cloud.token_cap = "1234567".into();
        cloud.apply(&mut file, true);
        let (settings, warnings) = parse(&file.text());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(settings.backend, CloudBackend::OpenAi);
        assert_eq!(
            settings.openai_endpoint.as_deref(),
            Some("https://openrouter.ai/api/v1")
        );
        assert_eq!(settings.openai_model.as_deref(), Some("openai/gpt-5-mini"));
        assert_eq!(settings.openai_accepted.as_deref(), Some("openrouter.ai"));
        assert!(!settings.cloud_accepted, "accepted for that host only");
        assert_eq!(Form::from_settings(&settings, true), cloud);
    }

    #[test]
    fn claude_code_needs_a_model_and_its_own_consent_but_no_key() {
        let code = Form {
            mode: Some(Mode::Cloud),
            backend: CloudBackend::ClaudeCode,
            claude_code_model: "haiku".into(),
            ..form()
        };
        assert_eq!(code.problem(), None, "no key");
        let mut nameless = code.clone();
        nameless.claude_code_model = " ".into();
        assert!(
            nameless
                .problem()
                .unwrap()
                .starts_with("Choose or type a Claude Code model")
        );
        assert!(code.needs_cloud_consent(&parse("cloud_accepted=true").0));
        assert!(!code.needs_cloud_consent(&parse("claude_code_accepted=true").0));
        let (title, text) = code.consent();
        assert_eq!(title, "Send captures to Anthropic through Claude Code?");
        assert!(text.contains("(haiku)") && text.contains("experimental"));

        let mut file = SettingsFile::from_text("");
        code.apply(&mut file, true);
        let (settings, warnings) = parse(&file.text());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(settings.backend, CloudBackend::ClaudeCode);
        assert_eq!(settings.claude_code_model, "haiku");
        assert!(settings.claude_code_accepted && !settings.cloud_accepted);
    }

    #[test]
    fn copilot_needs_a_model_and_its_own_consent_but_no_key() {
        let copilot = Form {
            mode: Some(Mode::Cloud),
            backend: CloudBackend::Copilot,
            ..form()
        };
        assert_eq!(copilot.problem(), None, "no key");
        let mut nameless = copilot.clone();
        nameless.copilot_model = String::new();
        assert!(
            nameless
                .problem()
                .unwrap()
                .starts_with("Choose or type a Copilot model")
        );
        assert!(copilot.needs_cloud_consent(&parse("claude_code_accepted=true").0));
        assert!(!copilot.needs_cloud_consent(&parse("copilot_accepted=true").0));
        let (title, text) = copilot.consent();
        assert_eq!(title, "Send captures to GitHub Copilot?");
        assert!(text.contains("the model Copilot chooses") && text.contains("experimental"));
        let mut named = copilot.clone();
        named.copilot_model = "gpt-5.4".into();
        assert!(named.consent().1.contains("provider of gpt-5.4:"));

        let mut file = SettingsFile::from_text("");
        named.apply(&mut file, true);
        let (settings, warnings) = parse(&file.text());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(settings.backend, CloudBackend::Copilot);
        assert_eq!(settings.copilot_model, "gpt-5.4");
        assert!(settings.copilot_accepted && !settings.claude_code_accepted);
    }

    #[test]
    fn the_theme_is_saved_with_the_rest() {
        let mut file = SettingsFile::from_text("");
        Form {
            theme: ThemePreference::Dark,
            ..form()
        }
        .apply(&mut file, false);
        let (settings, _) = parse(&file.text());
        assert_eq!(settings.theme, ThemePreference::Dark);
        assert_eq!(
            Form::from_settings(&settings, false).theme,
            ThemePreference::Dark
        );
    }

    #[test]
    fn every_first_use_screen_reads_as_plain_sentences() {
        for backend in [
            CloudBackend::Claude,
            CloudBackend::OpenAi,
            CloudBackend::ClaudeCode,
            CloudBackend::Copilot,
        ] {
            let (title, text) = Form {
                backend,
                ..openai()
            }
            .consent();
            assert!(!text.contains("  "), "{title}: {text:?}");
            assert!(text.ends_with('?'), "{title}");
        }
    }

    #[test]
    fn cloud_needs_consent_once_per_provider_and_host() {
        let mut cloud = form();
        cloud.mode = Some(Mode::Cloud);
        assert!(cloud.needs_cloud_consent(&parse("").0));
        assert!(!cloud.needs_cloud_consent(&parse("cloud_accepted=true").0));
        assert!(!form().needs_cloud_consent(&parse("").0));
        let (title, text) = cloud.consent();
        assert_eq!(title, "Send captures to Anthropic?");
        assert!(text.contains("Anthropic's Claude API (claude-sonnet-5-5)"));

        let mut other = openai();
        assert!(other.needs_cloud_consent(&parse("cloud_accepted=true").0));
        assert!(!other.needs_cloud_consent(&parse("openai_accepted=openrouter.ai").0));
        other.openai_endpoint = "https://api.openai.com/v1".into();
        assert!(
            other.needs_cloud_consent(&parse("openai_accepted=openrouter.ai").0),
            "another host asks again"
        );
        let (title, text) = other.consent();
        assert_eq!(title, "Send captures to api.openai.com?");
        assert!(text.contains("the API at api.openai.com (openai/gpt-5-mini)"));
        assert!(text.ends_with("Send your captures to api.openai.com?"));
    }
}
