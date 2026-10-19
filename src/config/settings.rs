//! `%AppData%\FastCapture\fastcapture.ini`: per-PC settings. `key=value` lines; `#`
//! or `;` comments. FastCapture writes it by changing only the lines it sets, so comments,
//! unknown keys and their order survive. A bad value falls back to its default with a warning
//! naming its line.

use super::keymap::KeyStroke;
use crate::notebook::notebook_ini::Warning;
use std::path::{Path, PathBuf};

pub const SETTINGS_FILE: &str = "fastcapture.ini";
pub const DEFAULT_CAPTURE_HOTKEY: &str = "Ctrl+Alt+C";
/// How many notebooks the box's picker remembers.
pub const RECENT_LIMIT: usize = 10;

/// `%AppData%\FastCapture\fastcapture.ini`.
pub fn settings_path() -> Option<PathBuf> {
    crate::paths::roaming().map(|dir| dir.join(SETTINGS_FILE))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Settings {
    /// The capture notebook. `None` until chosen (first run picks one, `default_notebook`).
    pub notebook: Option<PathBuf>,
    /// Notebooks offered by the box's picker, most recent first.
    pub recent_notebooks: Vec<PathBuf>,
    /// `None`: turned off (an empty value).
    pub capture_hotkey: Option<KeyStroke>,
    pub clipboard_hotkey: Option<KeyStroke>,
    pub capture_source: bool,
    pub start_at_sign_in: bool,
    /// Where notes are written: `None` until chosen in Settings.
    pub mode: Option<Mode>,
    /// Cloud mode's backend.
    pub backend: CloudBackend,
    /// Local mode: the server's address and the model to use.
    pub local_endpoint: Option<String>,
    pub local_model: Option<String>,
    pub claude_model: String,
    /// The first-use screen for Cloud mode was accepted (for Anthropic).
    pub cloud_accepted: bool,
    /// The OpenAI-compatible cloud API: its `https://` base URL and model.
    pub openai_endpoint: Option<String>,
    pub openai_model: Option<String>,
    /// The host the first-use screen was accepted for, for the OpenAI-compatible API.
    pub openai_accepted: Option<String>,
    /// The most tokens cloud requests may use in a month; `None` for no cap.
    pub token_cap: Option<u64>,
    /// Claude Code: the model (an alias such as `sonnet`, or a full name), where `claude` is
    /// when it isn't found by itself, and whether its first-use screen was accepted.
    pub claude_code_model: String,
    pub claude_code_path: Option<PathBuf>,
    pub claude_code_accepted: bool,
}

/// One model provider at a time: on this PC, or in the cloud.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Local,
    Cloud,
}

/// Cloud mode's backends.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CloudBackend {
    #[default]
    Claude,
    /// Any OpenAI-compatible API: OpenAI, OpenRouter, Azure OpenAI…
    OpenAi,
    /// The user's own installed Claude Code, signed in with their Claude plan.
    ClaudeCode,
}

impl CloudBackend {
    /// The value `backend=` takes, and the API key's Credential Manager name.
    pub fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::OpenAi => "openai",
            Self::ClaudeCode => "claude_code",
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            notebook: None,
            recent_notebooks: Vec::new(),
            capture_hotkey: KeyStroke::parse(DEFAULT_CAPTURE_HOTKEY),
            clipboard_hotkey: None,
            capture_source: false,
            start_at_sign_in: true,
            mode: None,
            backend: CloudBackend::Claude,
            local_endpoint: None,
            local_model: None,
            claude_model: crate::model::claude::DEFAULT_MODEL.to_owned(),
            cloud_accepted: false,
            openai_endpoint: None,
            openai_model: None,
            openai_accepted: None,
            token_cap: None,
            claude_code_model: crate::model::claude_code::DEFAULT_MODEL.to_owned(),
            claude_code_path: None,
            claude_code_accepted: false,
        }
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// An empty value turns a hotkey off; anything else must be a hotkey with a modifier.
fn parse_hotkey(value: &str) -> Option<Option<KeyStroke>> {
    if value.is_empty() {
        return Some(None);
    }
    KeyStroke::parse(value)
        .filter(|stroke| stroke.has_modifier())
        .map(Some)
}

/// A token count: digits, with `k` or `M` allowed (`500k`, `2M`). Empty or `0` is no cap.
pub fn parse_token_cap(value: &str) -> Option<Option<u64>> {
    let value = value.trim();
    if value.is_empty() {
        return Some(None);
    }
    let (digits, times) = match value.as_bytes()[value.len() - 1] {
        b'k' | b'K' => (&value[..value.len() - 1], 1_000),
        b'm' | b'M' => (&value[..value.len() - 1], 1_000_000),
        _ => (value, 1),
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let cap = digits.parse::<u64>().ok()?.checked_mul(times)?;
    Some((cap > 0).then_some(cap))
}

/// An absolute path.
fn parse_folder(value: &str) -> Option<PathBuf> {
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
}

/// Reads `text`, the file's contents.
pub fn parse(text: &str) -> (Settings, Vec<Warning>) {
    let mut settings = Settings::default();
    let mut warnings = Vec::new();
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let mut warn = |message: String| {
            warnings.push(Warning {
                line: index + 1,
                message,
            })
        };
        let Some((key, value)) = trimmed.split_once('=') else {
            warn(format!(
                "line is not a recognized \"key=value\" setting: {trimmed}"
            ));
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let applied = match key {
            "version" => Some(()),
            "notebook" => parse_folder(value).map(|path| settings.notebook = Some(path)),
            "recent_notebook" => parse_folder(value).map(|path| {
                if settings.recent_notebooks.len() < RECENT_LIMIT
                    && !settings.recent_notebooks.contains(&path)
                {
                    settings.recent_notebooks.push(path);
                }
            }),
            "capture_hotkey" => parse_hotkey(value).map(|hotkey| settings.capture_hotkey = hotkey),
            "clipboard_hotkey" => {
                parse_hotkey(value).map(|hotkey| settings.clipboard_hotkey = hotkey)
            }
            "capture_source" => parse_bool(value).map(|on| settings.capture_source = on),
            "start_at_sign_in" => parse_bool(value).map(|on| settings.start_at_sign_in = on),
            "mode" => match value.to_ascii_lowercase().as_str() {
                "local" => Some(Some(Mode::Local)),
                "cloud" => Some(Some(Mode::Cloud)),
                "" => Some(None),
                _ => None,
            }
            .map(|mode| settings.mode = mode),
            "backend" => [
                CloudBackend::Claude,
                CloudBackend::OpenAi,
                CloudBackend::ClaudeCode,
            ]
            .into_iter()
            .find(|backend| value.eq_ignore_ascii_case(backend.key()))
            .map(|backend| settings.backend = backend),
            "local_endpoint" if value.is_empty() => {
                settings.local_endpoint = None;
                Some(())
            }
            "local_endpoint" => crate::net::http::parse_url(value)
                .ok()
                .map(|_| settings.local_endpoint = Some(value.to_owned())),
            "local_model" => {
                settings.local_model = (!value.is_empty()).then(|| value.to_owned());
                Some(())
            }
            "claude_model" => (!value.is_empty() && !value.contains(char::is_whitespace))
                .then(|| settings.claude_model = value.to_owned()),
            "cloud_accepted" => parse_bool(value).map(|on| settings.cloud_accepted = on),
            "openai_endpoint" if value.is_empty() => {
                settings.openai_endpoint = None;
                Some(())
            }
            "openai_endpoint" => crate::net::http::parse_url(value)
                .ok()
                .filter(|target| target.secure)
                .map(|_| settings.openai_endpoint = Some(value.to_owned())),
            "openai_model" => {
                settings.openai_model = (!value.is_empty()).then(|| value.to_owned());
                Some(())
            }
            "token_cap" => parse_token_cap(value).map(|cap| settings.token_cap = cap),
            "claude_code_model" => (!value.is_empty() && !value.contains(char::is_whitespace))
                .then(|| settings.claude_code_model = value.to_owned()),
            "claude_code_path" if value.is_empty() => {
                settings.claude_code_path = None;
                Some(())
            }
            "claude_code_path" => {
                parse_folder(value).map(|path| settings.claude_code_path = Some(path))
            }
            "claude_code_accepted" => {
                parse_bool(value).map(|on| settings.claude_code_accepted = on)
            }
            "openai_accepted" => {
                settings.openai_accepted = (!value.is_empty()).then(|| value.to_ascii_lowercase());
                Some(())
            }
            // Kept for later milestones and other versions; never an error.
            _ if is_reserved(key) => Some(()),
            _ => {
                warn(format!("unknown setting key: {key}"));
                continue;
            }
        };
        if applied.is_none() {
            warn(format!("invalid value for {key}: \"{value}\""));
        }
    }
    (settings, warnings)
}

/// Keys FastCapture writes itself that `parse` doesn't read: window placement and later
/// milestones' keys.
fn is_reserved(key: &str) -> bool {
    key.starts_with("review_")
}

/// The file's text, kept line by line so FastCapture can change one setting and leave the rest.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SettingsFile {
    lines: Vec<String>,
}

impl SettingsFile {
    pub fn from_text(text: &str) -> Self {
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
        Self {
            lines: text.lines().map(str::to_owned).collect(),
        }
    }

    /// The file at `path`, or the starter text when it doesn't exist or can't be read.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|text| Self::from_text(&text))
            .unwrap_or_else(|_| Self::from_text(&starter_text()))
    }

    pub fn text(&self) -> String {
        let mut text = self.lines.join("\r\n");
        text.push_str("\r\n");
        text
    }

    pub fn settings(&self) -> (Settings, Vec<Warning>) {
        parse(&self.text())
    }

    /// The value of `key`'s first line, for keys `parse` doesn't read (window placement).
    pub fn value(&self, key: &str) -> Option<String> {
        self.lines
            .iter()
            .find(|line| Self::key_of(line) == Some(key))
            .and_then(|line| line.split_once('='))
            .map(|(_, value)| value.trim().to_owned())
    }

    fn key_of(line: &str) -> Option<&str> {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with(';') {
            return None;
        }
        trimmed.split_once('=').map(|(key, _)| key.trim())
    }

    /// Sets `key` to `values`, one line each: in place of the key's first line, removing its
    /// other lines, or at the end when it has none. No values removes the key.
    pub fn set_all(&mut self, key: &str, values: &[String]) {
        let first = self
            .lines
            .iter()
            .position(|line| Self::key_of(line) == Some(key));
        self.lines.retain(|line| Self::key_of(line) != Some(key));
        let new: Vec<String> = values
            .iter()
            .map(|value| format!("{key}={value}"))
            .collect();
        let at = first.unwrap_or(self.lines.len()).min(self.lines.len());
        self.lines.splice(at..at, new);
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.set_all(key, &[value.to_owned()]);
    }

    /// Makes `notebook` the capture notebook and the first of the recent ones.
    pub fn choose_notebook(&mut self, notebook: &Path) {
        let (settings, _) = self.settings();
        let mut recent = vec![notebook.to_path_buf()];
        recent.extend(
            settings
                .recent_notebooks
                .into_iter()
                .filter(|known| known != notebook),
        );
        recent.truncate(RECENT_LIMIT);
        self.set("notebook", &notebook.to_string_lossy());
        let values: Vec<String> = recent
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        self.set_all("recent_notebook", &values);
    }

    /// Writes the file atomically, making its folder.
    pub fn save(&self, path: &Path) -> crate::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| crate::Error::io(parent, error))?;
        }
        crate::platform::saver::save_atomic(path, self.text().as_bytes())
    }
}

/// The file FastCapture writes the first time: every key commented out at its default.
pub fn starter_text() -> String {
    [
        "# FastCapture settings. Remove the # in front of a line to change it.",
        "version=1",
        "# The notebook captures go to (a folder).",
        "# notebook=C:\\Users\\you\\Documents\\FastPad",
        "# Hotkeys: Ctrl, Shift, Alt and Win with a letter, digit, F1-F24 or a named key. Empty turns one off.",
        "# capture_hotkey=Ctrl+Alt+C",
        "# Captures the clipboard with no box.",
        "# clipboard_hotkey=",
        "# Record the title of the window in front when you capture (it may contain private things).",
        "# capture_source=false",
        "# start_at_sign_in=true",
        "",
    ]
    .join("\r\n")
}

/// The notebook to use the first time: FastPad's most recent notebook that still exists (from
/// `folders.ini` in FastPad's data folder), else `Documents\FastPad`, FastPad's own default.
pub fn default_notebook(fastpad_data: Option<&Path>, documents: Option<&Path>) -> Option<PathBuf> {
    let recent = fastpad_data
        .and_then(|dir| std::fs::read_to_string(dir.join("folders.ini")).ok())
        .map(|text| fastpad_recent_folders(&text))
        .unwrap_or_default();
    recent
        .into_iter()
        .find(|folder| folder.is_dir())
        .or_else(|| documents.map(|documents| documents.join("FastPad")))
}

/// The `folder=` lines of FastPad's `folders.ini`, most recent first, when it is version 1.
fn fastpad_recent_folders(text: &str) -> Vec<PathBuf> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut version = None;
    let mut folders = Vec::new();
    for line in text.lines() {
        match line.split_once('=') {
            Some(("version", value)) => version = Some(value.trim()),
            Some(("folder", value)) if !value.is_empty() => folders.push(PathBuf::from(value)),
            _ => {}
        }
    }
    if version == Some("1") {
        folders
    } else {
        Vec::new()
    }
}

/// FastPad's data folder, `%LocalAppData%\FastPad`, where its `folders.ini` lives.
pub fn fastpad_data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(value).join("FastPad"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn the_starter_file_means_the_defaults() {
        let (settings, warnings) = parse(&starter_text());
        assert_eq!(settings, Settings::default());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            settings.capture_hotkey.unwrap().to_string(),
            DEFAULT_CAPTURE_HOTKEY
        );
    }

    #[test]
    fn model_settings_are_read_and_bad_ones_named() {
        let (settings, warnings) = parse(
            "mode=Local\nlocal_endpoint=http://localhost:11434\nlocal_model=qwen2.5vl:7b\n\
             backend=claude\nclaude_model=claude-opus-5-5\ncloud_accepted=true\n",
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(settings.mode, Some(Mode::Local));
        assert_eq!(
            settings.local_endpoint.as_deref(),
            Some("http://localhost:11434")
        );
        assert_eq!(settings.local_model.as_deref(), Some("qwen2.5vl:7b"));
        assert_eq!(settings.claude_model, "claude-opus-5-5");
        assert!(settings.cloud_accepted);

        let (settings, warnings) = parse(
            "mode=remote\nlocal_endpoint=localhost:11434\nbackend=gpt\nclaude_model=a b\nmode=\n",
        );
        let lines: Vec<usize> = warnings.iter().map(|warning| warning.line).collect();
        assert_eq!(lines, [1, 2, 3, 4]);
        assert_eq!(settings.mode, None);
        assert_eq!(settings.claude_model, crate::model::claude::DEFAULT_MODEL);
    }

    #[test]
    fn the_claude_code_settings_are_read() {
        let (settings, warnings) = parse(
            "mode=cloud\nbackend=claude_code\nclaude_code_model=haiku\n\
             claude_code_path=C:\\Tools\\claude.exe\nclaude_code_accepted=true\n",
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(settings.backend, CloudBackend::ClaudeCode);
        assert_eq!(settings.claude_code_model, "haiku");
        assert_eq!(
            settings.claude_code_path,
            Some(PathBuf::from(r"C:\Tools\claude.exe"))
        );
        assert!(settings.claude_code_accepted);
        let (settings, warnings) = parse("claude_code_path=claude.exe\nclaude_code_model=\n");
        assert_eq!(warnings.len(), 2, "a relative path, an empty model");
        assert_eq!(settings.claude_code_path, None);
        assert_eq!(settings.claude_code_model, "sonnet");
    }

    #[test]
    fn the_token_cap_takes_counts_with_k_or_m() {
        assert_eq!(parse_token_cap(""), Some(None));
        assert_eq!(parse_token_cap("0"), Some(None));
        assert_eq!(parse_token_cap("250000"), Some(Some(250_000)));
        assert_eq!(parse_token_cap("500k"), Some(Some(500_000)));
        assert_eq!(parse_token_cap("2M"), Some(Some(2_000_000)));
        assert_eq!(parse_token_cap("2.5M"), None);
        assert_eq!(parse_token_cap("-5"), None);
        assert_eq!(parse_token_cap("M"), None);
        assert_eq!(parse_token_cap("99999999999999999999k"), None);
        let (settings, warnings) = parse(
            "token_cap=2M
",
        );
        assert!(warnings.is_empty());
        assert_eq!(settings.token_cap, Some(2_000_000));
        let (settings, warnings) = parse(
            "token_cap=lots
",
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(settings.token_cap, None, "a bad value means none");
    }

    #[test]
    fn the_openai_compatible_settings_are_read_and_need_https() {
        let (settings, warnings) = parse(
            "mode=cloud
backend=OpenAI
openai_endpoint=https://openrouter.ai/api/v1
             openai_model=openai/gpt-5-mini
openai_accepted=OpenRouter.ai
",
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(settings.backend, CloudBackend::OpenAi);
        assert_eq!(
            settings.openai_endpoint.as_deref(),
            Some("https://openrouter.ai/api/v1")
        );
        assert_eq!(settings.openai_model.as_deref(), Some("openai/gpt-5-mini"));
        assert_eq!(settings.openai_accepted.as_deref(), Some("openrouter.ai"));

        let (settings, warnings) = parse(
            "openai_endpoint=http://api.openai.com/v1
",
        );
        assert_eq!(warnings.len(), 1, "the key must not go over http");
        assert_eq!(settings.openai_endpoint, None);
    }

    #[test]
    fn values_are_read_and_bad_ones_keep_their_defaults() {
        let (settings, warnings) = parse(
            "notebook=D:\\Notes\r\nrecent_notebook=D:\\Notes\r\nrecent_notebook=E:\\Work\r\n\
             recent_notebook=D:\\Notes\r\ncapture_hotkey=Win+Shift+C\r\nclipboard_hotkey=\r\n\
             capture_source=yes\r\nstart_at_sign_in=off\r\nreview_width=900\r\n",
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(settings.notebook, Some(PathBuf::from(r"D:\Notes")));
        assert_eq!(
            settings.recent_notebooks,
            [PathBuf::from(r"D:\Notes"), PathBuf::from(r"E:\Work")]
        );
        assert_eq!(settings.capture_hotkey.unwrap().to_string(), "Shift+Win+C");
        assert_eq!(settings.clipboard_hotkey, None);
        assert!(settings.capture_source);
        assert!(!settings.start_at_sign_in);

        let (settings, warnings) = parse(
            "notebook=relative\\path\ncapture_hotkey=Space\ncapture_source=maybe\ncolour=blue\nnonsense\ncapture_hotkey=\n",
        );
        let lines: Vec<usize> = warnings.iter().map(|warning| warning.line).collect();
        assert_eq!(lines, [1, 2, 3, 4, 5]);
        assert_eq!(settings.notebook, None);
        assert_eq!(settings.capture_hotkey, None, "the last valid line wins");
        assert!(!settings.capture_source);
    }

    #[test]
    fn writing_a_setting_keeps_everything_else() {
        let mut file = SettingsFile::from_text(
            "# mine\r\ncapture_hotkey=Ctrl+Alt+K\r\nfuture_key=x\r\nrecent_notebook=E:\\Old\r\n",
        );
        file.choose_notebook(Path::new(r"D:\Notes"));
        file.set("capture_hotkey", "Ctrl+Alt+J");
        assert_eq!(
            file.text(),
            "# mine\r\ncapture_hotkey=Ctrl+Alt+J\r\nfuture_key=x\r\n\
             recent_notebook=D:\\Notes\r\nrecent_notebook=E:\\Old\r\nnotebook=D:\\Notes\r\n"
        );
        file.choose_notebook(Path::new(r"E:\Old"));
        let (settings, _) = file.settings();
        assert_eq!(settings.notebook, Some(PathBuf::from(r"E:\Old")));
        assert_eq!(
            settings.recent_notebooks,
            [PathBuf::from(r"E:\Old"), PathBuf::from(r"D:\Notes")]
        );
        file.set_all("recent_notebook", &[]);
        assert!(!file.text().contains("recent_notebook"));
    }

    #[test]
    fn the_first_notebook_is_fastpads_most_recent_that_exists() {
        let temp =
            std::env::temp_dir().join(format!("fastcapture-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let fastpad = temp.join("FastPad");
        let work = temp.join("Work");
        fs::create_dir_all(&fastpad).unwrap();
        fs::create_dir_all(&work).unwrap();
        fs::write(
            fastpad.join("folders.ini"),
            format!(
                "version=1\r\nfolder={}\r\nfolder={}\r\nfavorite=X:\\y\r\n",
                temp.join("Gone").display(),
                work.display()
            ),
        )
        .unwrap();
        let documents = temp.join("Documents");
        assert_eq!(
            default_notebook(Some(&fastpad), Some(&documents)),
            Some(work)
        );
        fs::write(fastpad.join("folders.ini"), "version=2\r\nfolder=C:\\\r\n").unwrap();
        assert_eq!(
            default_notebook(Some(&fastpad), Some(&documents)),
            Some(documents.join("FastPad"))
        );
        assert_eq!(default_notebook(None, None), None);
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn saved_settings_read_back() {
        let temp =
            std::env::temp_dir().join(format!("fastcapture-settings-save-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let path = temp.join("FastCapture").join(SETTINGS_FILE);
        let mut file = SettingsFile::load(&path);
        assert_eq!(file.text(), starter_text());
        file.choose_notebook(Path::new(r"D:\Notes"));
        file.save(&path).unwrap();
        assert_eq!(
            SettingsFile::load(&path).settings().0.notebook,
            Some(PathBuf::from(r"D:\Notes"))
        );
        let _ = fs::remove_dir_all(&temp);
    }
}
