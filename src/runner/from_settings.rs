//! The backend `fastcapture.ini` chooses: a local server when the mode is Local; the Claude API
//! or an OpenAI-compatible API, with its key from Credential Manager, when it is Cloud. Never
//! both, and never one standing in for the other.

use super::worker::Factory;
use crate::config::settings::{CloudBackend, Mode, Settings, SettingsFile};
use crate::model::Backend;
use crate::model::claude::Claude;
use crate::model::local::is_on_this_pc;
use crate::model::openai::{self, OpenAi};
use std::path::PathBuf;

/// The backend for `settings`, with `key` finding a saved API key by credential name.
pub fn build(
    settings: &Settings,
    key: impl Fn(&str) -> Option<String>,
) -> Result<Box<dyn Backend>, String> {
    match settings.mode {
        None => Err("Choose Local or Cloud in Settings.".to_owned()),
        Some(Mode::Local) => {
            let endpoint = settings
                .local_endpoint
                .as_deref()
                .ok_or("Choose a local model server in Settings.")?;
            if !is_on_this_pc(endpoint) {
                return Err(format!(
                    "{endpoint} isn't on this PC. Local mode only uses servers on this PC."
                ));
            }
            let model = settings
                .local_model
                .as_deref()
                .ok_or("Choose a local model in Settings.")?;
            OpenAi::local(endpoint, model)
                .map(|backend| Box::new(backend) as Box<dyn Backend>)
                .map_err(|error| error.to_string())
        }
        Some(Mode::Cloud) => match settings.backend {
            CloudBackend::Claude => {
                if !settings.cloud_accepted {
                    return Err("Confirm Cloud mode in Settings first.".to_owned());
                }
                let key = key(&crate::secrets::target("claude"))
                    .ok_or("Add your Claude API key in Settings.")?;
                Claude::new(&settings.claude_model, key)
                    .map(|backend| Box::new(backend) as Box<dyn Backend>)
                    .map_err(|error| error.to_string())
            }
            CloudBackend::OpenAi => {
                let endpoint = settings
                    .openai_endpoint
                    .as_deref()
                    .ok_or("Enter the OpenAI-compatible API's address in Settings.")?;
                let model = settings
                    .openai_model
                    .as_deref()
                    .ok_or("Choose the OpenAI-compatible model in Settings.")?;
                let host = openai::host(endpoint).unwrap_or_default();
                if settings.openai_accepted.as_deref() != Some(host.as_str()) {
                    return Err(format!("Confirm Cloud mode for {host} in Settings first."));
                }
                let key = key(&crate::secrets::target("openai"))
                    .ok_or("Add the OpenAI-compatible API key in Settings.")?;
                OpenAi::cloud(endpoint, model, key)
                    .map(|backend| Box::new(backend) as Box<dyn Backend>)
                    .map_err(|error| error.to_string())
            }
        },
    }
}

/// The worker's factory: reads `fastcapture.ini` each time it builds.
pub struct FromSettings {
    pub settings: Option<PathBuf>,
}

impl Factory for FromSettings {
    fn backend(&self) -> Result<Box<dyn Backend>, String> {
        let settings = self
            .settings
            .as_deref()
            .map(|path| SettingsFile::load(path).settings().0)
            .unwrap_or_default();
        build(&settings, crate::secrets::load)
    }

    fn token_cap(&self) -> Option<u64> {
        let path = self.settings.as_deref()?;
        SettingsFile::load(path).settings().0.token_cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(text: &str) -> Settings {
        crate::config::settings::parse(text).0
    }

    #[test]
    fn the_mode_chooses_one_backend_and_says_what_is_missing() {
        let none = |_: &str| None;
        let error = |text: &str| build(&settings(text), none).err().unwrap();
        assert_eq!(error(""), "Choose Local or Cloud in Settings.");
        assert_eq!(
            error("mode=local"),
            "Choose a local model server in Settings."
        );
        assert_eq!(
            error("mode=local\nlocal_endpoint=http://localhost:11434"),
            "Choose a local model in Settings."
        );
        assert!(
            error("mode=local\nlocal_endpoint=http://192.168.1.5:11434\nlocal_model=m")
                .contains("isn't on this PC")
        );
        assert_eq!(error("mode=cloud"), "Confirm Cloud mode in Settings first.");
        assert_eq!(
            error("mode=cloud\ncloud_accepted=true"),
            "Add your Claude API key in Settings."
        );

        let local = build(
            &settings("mode=local\nlocal_endpoint=http://localhost:11434\nlocal_model=gemma3"),
            none,
        )
        .unwrap();
        assert_eq!(local.label(), "local/gemma3");
        let key = |target: &str| (target == "FastCapture/claude").then(|| "sk-ant-x".to_owned());
        let cloud = build(
            &settings("mode=cloud\ncloud_accepted=true\nclaude_model=claude-haiku-5-5"),
            key,
        )
        .unwrap();
        assert_eq!(cloud.label(), "cloud/claude-haiku-5-5");
    }

    #[test]
    fn the_openai_compatible_api_needs_its_address_model_consent_and_key() {
        let none = |_: &str| None;
        let base = "mode=cloud
backend=openai
";
        let error = |text: &str| {
            build(&settings(&format!("{base}{text}")), none)
                .err()
                .unwrap()
        };
        assert_eq!(
            error(""),
            "Enter the OpenAI-compatible API's address in Settings."
        );
        assert_eq!(
            error("openai_endpoint=https://api.openai.com/v1"),
            "Choose the OpenAI-compatible model in Settings."
        );
        let chosen = "openai_endpoint=https://api.openai.com/v1
openai_model=gpt-5-mini
";
        assert_eq!(
            error(chosen),
            "Confirm Cloud mode for api.openai.com in Settings first."
        );
        assert_eq!(
            error(&format!("{chosen}openai_accepted=openrouter.ai")),
            "Confirm Cloud mode for api.openai.com in Settings first.",
            "accepted for another host"
        );
        assert_eq!(
            error(&format!(
                "{chosen}openai_accepted=api.openai.com
cloud_accepted=true"
            )),
            "Add the OpenAI-compatible API key in Settings."
        );
        let key = |target: &str| (target == "FastCapture/openai").then(|| "sk-x".to_owned());
        let cloud = build(
            &settings(&format!("{base}{chosen}openai_accepted=api.openai.com")),
            key,
        )
        .unwrap();
        assert_eq!(cloud.label(), "cloud/gpt-5-mini");
    }
}
