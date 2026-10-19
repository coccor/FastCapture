//! The user's own installed Claude Code (`claude`): where it is, its version and the options it
//! offers (read from `claude --help`, since they change between versions), and the command line
//! for one note.

use super::run::{self, RunError};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// How long `--version` and `--help` may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// An installed Claude Code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cli {
    pub path: PathBuf,
    /// `2.1.303`.
    pub version: String,
    pub options: Options,
}

/// The optional flags this version offers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Options {
    pub json_schema: bool,
    pub no_session_persistence: bool,
    pub setting_sources: bool,
    pub strict_mcp_config: bool,
    pub system_prompt_file: bool,
}

/// What `claude --help` says this version offers.
pub fn options_from_help(help: &str) -> Options {
    let words: Vec<&str> = help
        .split(|c: char| c.is_whitespace() || c == ',')
        .collect();
    let has = |flag: &str| words.contains(&flag);
    Options {
        json_schema: has("--json-schema"),
        no_session_persistence: has("--no-session-persistence"),
        setting_sources: has("--setting-sources"),
        strict_mcp_config: has("--strict-mcp-config"),
        // Listed only inside other options' descriptions in some versions.
        system_prompt_file: has("--system-prompt-file") || help.contains("--system-prompt[-file]"),
    }
}

/// The version in `claude --version`'s output (`2.1.303 (Claude Code)`).
pub fn version_from(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|word| word.starts_with(|c: char| c.is_ascii_digit()) && word.contains('.'))
        .map(str::to_owned)
}

/// Where `claude` is: `configured` when set (and nowhere else, so a wrong path isn't covered up),
/// else the first `claude.exe` or `claude.cmd` on `PATH`, else where the native installer puts
/// it.
pub fn find(configured: Option<&Path>) -> Option<PathBuf> {
    find_in(
        configured,
        std::env::var_os("PATH"),
        std::env::var_os("USERPROFILE").map(PathBuf::from),
    )
}

pub fn find_in(
    configured: Option<&Path>,
    path_variable: Option<OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(configured) = configured {
        return configured.is_file().then(|| configured.to_path_buf());
    }
    let dirs: Vec<PathBuf> = path_variable
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    for name in ["claude.exe", "claude.cmd"] {
        if let Some(found) = dirs
            .iter()
            .map(|dir| dir.join(name))
            .find(|path| path.is_file())
        {
            return Some(found);
        }
    }
    home.map(|home| home.join(r".local\bin\claude.exe"))
        .filter(|path| path.is_file())
}

/// Where Claude Code runs: an empty folder of FastCapture's, so no project's `CLAUDE.md` or
/// settings are read. Also holds the rules file.
pub fn scratch_dir() -> PathBuf {
    crate::paths::local()
        .map(|local| local.join("claude-code"))
        .unwrap_or_else(|| std::env::temp_dir().join("fastcapture-claude-code"))
}

/// Environment variables Claude Code mustn't inherit: an API key or base address would take the
/// place of the plan's login, and a host Claude Code session's own variables would redirect it.
pub fn removed_variables(names: impl Iterator<Item = OsString>) -> Vec<OsString> {
    names
        .filter(|name| {
            let name = name.to_string_lossy().to_ascii_uppercase();
            name.starts_with("ANTHROPIC_")
                || name.starts_with("CLAUDE_CODE_")
                || name == "CLAUDECODE"
        })
        .collect()
}

/// The variables to remove from this process's environment.
pub fn variables_to_remove() -> Vec<OsString> {
    removed_variables(std::env::vars_os().map(|(name, _)| name))
}

/// The CLIs already asked, by path, with the write time they were asked at, so an update is
/// noticed.
type Known = HashMap<PathBuf, (Option<SystemTime>, Cli)>;
static KNOWN: Mutex<Option<Known>> = Mutex::new(None);

/// The Claude Code at `path`: its version and options, asked once per session (again after it
/// is updated).
pub fn probe(path: &Path) -> Result<Cli, String> {
    let written = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok();
    if let Some(known) = KNOWN.lock().ok().and_then(|known| {
        known
            .as_ref()?
            .get(path)
            .filter(|(at, _)| *at == written)
            .map(|(_, cli)| cli.clone())
    }) {
        return Ok(known);
    }
    let dir = scratch_dir();
    let _ = std::fs::create_dir_all(&dir);
    let ask = |flag: &str| -> Result<String, String> {
        let output = run::run(
            path,
            &[flag.to_owned()],
            &[],
            &dir,
            &variables_to_remove(),
            PROBE_TIMEOUT,
        )
        .map_err(|error| match error {
            RunError::Start(reason) => format!("Claude Code couldn't be started: {reason}"),
            RunError::TimedOut => format!("Claude Code didn't answer {flag} in time"),
        })?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let version = version_from(&ask("--version")?)
        .ok_or_else(|| format!("{} doesn't look like Claude Code", path.display()))?;
    let cli = Cli {
        path: path.to_path_buf(),
        version,
        options: options_from_help(&ask("--help")?),
    };
    if let Ok(mut known) = KNOWN.lock() {
        known
            .get_or_insert_with(HashMap::new)
            .insert(path.to_path_buf(), (written, cli.clone()));
    }
    Ok(cli)
}

/// The writing rules: in a file when the CLI reads one, which keeps the command line short
/// (`cmd.exe` takes 8,191 characters at most), else on the command line.
pub enum SystemPrompt<'a> {
    File(&'a Path),
    Text(&'a str),
}

/// The command line for one note: printed, stream-json in and out, no tools, no MCP servers, no
/// settings, no saved session. `schema` is passed when the CLI takes one.
pub fn args(
    options: Options,
    model: &str,
    system: SystemPrompt,
    schema: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--input-format",
        "stream-json",
        "--tools",
        "",
        "--model",
        model,
    ]
    .map(str::to_owned)
    .to_vec();
    if options.strict_mcp_config {
        args.push("--strict-mcp-config".to_owned());
    }
    if options.setting_sources {
        args.extend(["--setting-sources".to_owned(), String::new()]);
    }
    if options.no_session_persistence {
        args.push("--no-session-persistence".to_owned());
    }
    match system {
        SystemPrompt::File(path) => args.extend([
            "--system-prompt-file".to_owned(),
            path.display().to_string(),
        ]),
        SystemPrompt::Text(text) => args.extend(["--system-prompt".to_owned(), text.to_owned()]),
    }
    if let (true, Some(schema)) = (options.json_schema, schema) {
        args.extend(["--json-schema".to_owned(), schema.to_owned()]);
    }
    args
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// What 2.1.303's `--help` says about the options that matter, cut down.
    pub(crate) const HELP_NOW: &str = "Usage: claude [options] [command] [prompt]\n\
        \x20 --bare                                Minimal mode: … via: --system-prompt[-file],\n\
        \x20                                       --append-system-prompt[-file], --add-dir\n\
        \x20 --json-schema <schema>                JSON Schema for structured output\n\
        \x20 --no-session-persistence              Disable session persistence\n\
        \x20 --setting-sources <sources>           Comma-separated list of setting sources\n\
        \x20 --strict-mcp-config                   Only use MCP servers from --mcp-config,\n\
        \x20 --system-prompt <prompt>              System prompt to use for the session\n";

    /// 2.0.42's, cut down.
    const HELP_THEN: &str = "  --input-format <format>  Input format\n\
        \x20 --tools <tools...>        Specify the list of available tools\n\
        \x20 --system-prompt <prompt>  System prompt to use for the session\n\
        \x20 --strict-mcp-config       Only use MCP servers from --mcp-config\n\
        \x20 --setting-sources <sources>  Comma-separated list\n";

    #[test]
    fn options_are_read_from_the_help_of_each_version() {
        assert_eq!(
            options_from_help(HELP_NOW),
            Options {
                json_schema: true,
                no_session_persistence: true,
                setting_sources: true,
                strict_mcp_config: true,
                system_prompt_file: true,
            }
        );
        assert_eq!(
            options_from_help(HELP_THEN),
            Options {
                setting_sources: true,
                strict_mcp_config: true,
                ..Options::default()
            },
            "--system-prompt alone isn't --system-prompt-file"
        );
        assert_eq!(
            version_from("2.1.303 (Claude Code)\n").as_deref(),
            Some("2.1.303")
        );
        assert_eq!(
            version_from("Claude Code v2.0.42").as_deref(),
            None,
            "v-prefixed isn't read"
        );
        assert_eq!(version_from("command not found"), None);
    }

    #[test]
    fn the_command_line_has_no_tools_and_takes_what_the_version_offers() {
        let rules = Path::new(r"C:\Local\FastCapture\claude-code\rules.md");
        let now = args(
            options_from_help(HELP_NOW),
            "haiku",
            SystemPrompt::File(rules),
            Some(r#"{"type":"object"}"#),
        );
        assert_eq!(
            now,
            [
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--input-format",
                "stream-json",
                "--tools",
                "",
                "--model",
                "haiku",
                "--strict-mcp-config",
                "--setting-sources",
                "",
                "--no-session-persistence",
                "--system-prompt-file",
                r"C:\Local\FastCapture\claude-code\rules.md",
                "--json-schema",
                r#"{"type":"object"}"#,
            ]
        );
        let then = args(
            options_from_help(HELP_THEN),
            "sonnet",
            SystemPrompt::Text("Rules."),
            Some("{}"),
        );
        assert!(!then.contains(&"--json-schema".to_owned()), "{then:?}");
        assert!(!then.contains(&"--no-session-persistence".to_owned()));
        assert!(then.ends_with(&["--system-prompt".to_owned(), "Rules.".to_owned()]));
    }

    #[test]
    fn api_keys_and_a_host_sessions_variables_are_removed() {
        let names = [
            "PATH",
            "ANTHROPIC_API_KEY",
            "anthropic_base_url",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CONFIG_DIR",
            "USERPROFILE",
        ]
        .map(OsString::from);
        assert_eq!(
            removed_variables(names.into_iter()),
            [
                "ANTHROPIC_API_KEY",
                "anthropic_base_url",
                "ANTHROPIC_AUTH_TOKEN",
                "CLAUDECODE",
                "CLAUDE_CODE_ENTRYPOINT"
            ]
            .map(OsString::from),
            "CLAUDE_CONFIG_DIR says where the user's own login is, so it stays"
        );
    }

    pub(crate) struct Temp(pub PathBuf);

    impl Temp {
        pub(crate) fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "fastcapture-claude-code-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        /// Writes `name` (a `.cmd`) with these lines.
        pub(crate) fn script(&self, name: &str, lines: &[&str]) -> PathBuf {
            let path = self.0.join(name);
            let mut text = String::from("@echo off\r\n");
            for line in lines {
                text.push_str(line);
                text.push_str("\r\n");
            }
            std::fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn it_is_found_where_it_is_set_on_path_or_where_the_installer_puts_it() {
        let temp = Temp::new("find");
        let on_path = temp.0.join("bin");
        let home = temp.0.join("home");
        std::fs::create_dir_all(home.join(r".local\bin")).unwrap();
        std::fs::create_dir_all(&on_path).unwrap();
        let installed = home.join(r".local\bin\claude.exe");
        std::fs::write(&installed, b"").unwrap();
        let path_variable = || Some(std::env::join_paths([&temp.0, &on_path]).unwrap());

        assert_eq!(
            find_in(None, path_variable(), Some(home.clone())),
            Some(installed.clone())
        );
        let shim = on_path.join("claude.cmd");
        std::fs::write(&shim, b"").unwrap();
        assert_eq!(
            find_in(None, path_variable(), Some(home.clone())),
            Some(shim.clone())
        );
        let exe = on_path.join("claude.exe");
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(
            find_in(None, path_variable(), Some(home.clone())),
            Some(exe),
            "an .exe before a .cmd"
        );
        assert_eq!(
            find_in(Some(&shim), path_variable(), Some(home.clone())),
            Some(shim)
        );
        assert_eq!(
            find_in(
                Some(&temp.0.join("missing.exe")),
                path_variable(),
                Some(home)
            ),
            None,
            "a set path that is wrong isn't covered up"
        );
        assert_eq!(find_in(None, None, None), None);
    }

    #[test]
    fn a_fake_cli_is_probed_for_its_version_and_options() {
        let temp = Temp::new("probe");
        let fake = temp.script(
            "claude.cmd",
            &[
                r#"if "%~1"=="--version" (echo 2.1.303 ^(Claude Code^)& exit /b 0)"#,
                "echo   --json-schema ^<schema^>   JSON Schema for structured output",
                "echo   --strict-mcp-config        Only use MCP servers",
            ],
        );
        let cli = probe(&fake).unwrap();
        assert_eq!(cli.version, "2.1.303");
        assert!(cli.options.json_schema && cli.options.strict_mcp_config);
        assert!(!cli.options.no_session_persistence);
        assert_eq!(probe(&fake).unwrap(), cli, "asked once");

        let not_claude = temp.script("other.cmd", &["echo hello"]);
        assert!(
            probe(&not_claude)
                .unwrap_err()
                .contains("doesn't look like Claude Code")
        );
    }
}
