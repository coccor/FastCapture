//! The user's own installed GitHub Copilot CLI (`copilot`): where it is, and running it as a
//! server on its stdin and stdout, which is how GitHub's Copilot SDKs drive it.

use super::rpc::{Connection, METHOD_NOT_FOUND, RpcError};
use crate::platform::job::{CREATE_NO_WINDOW, Job};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The protocol version FastCapture speaks.
pub const PROTOCOL_VERSION: u64 = 3;
/// A server on stdio, which never updates itself and writes no logs.
pub const SERVER_ARGS: [&str; 5] = [
    "--headless",
    "--no-auto-update",
    "--stdio",
    "--log-level",
    "none",
];
/// How much of stderr is kept.
const STDERR_KEPT: usize = 4096;

/// Where npm puts the program itself, beside the `copilot.cmd` it makes.
#[cfg(target_arch = "aarch64")]
const NPM_PACKAGE: &str = "copilot-win32-arm64";
#[cfg(not(target_arch = "aarch64"))]
const NPM_PACKAGE: &str = "copilot-win32-x64";

/// Where `copilot` is: `configured` when set (and nowhere else, so a wrong path isn't covered
/// up), else the first `copilot.exe` or `copilot.cmd` on `PATH`.
pub fn find(configured: Option<&Path>) -> Option<PathBuf> {
    find_in(configured, std::env::var_os("PATH"))
}

pub fn find_in(configured: Option<&Path>, path_variable: Option<OsString>) -> Option<PathBuf> {
    let found = match configured {
        Some(configured) => configured.is_file().then(|| configured.to_path_buf()),
        None => {
            let dirs: Vec<PathBuf> = path_variable
                .map(|value| std::env::split_paths(&value).collect())
                .unwrap_or_default();
            ["copilot.exe", "copilot.cmd"].iter().find_map(|name| {
                dirs.iter()
                    .map(|dir| dir.join(name))
                    .find(|path| path.is_file())
            })
        }
    }?;
    Some(program_behind(&found).unwrap_or(found))
}

/// npm's `copilot.cmd` only starts Node to start `copilot.exe` from its package; that program is
/// started directly instead, without `cmd.exe` or Node in between.
fn program_behind(shim: &Path) -> Option<PathBuf> {
    let is_cmd = shim
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cmd"));
    if !is_cmd {
        return None;
    }
    let modules = shim.parent()?.join("node_modules").join("@github");
    [
        modules
            .join("copilot")
            .join("node_modules")
            .join("@github")
            .join(NPM_PACKAGE),
        modules.join(NPM_PACKAGE),
    ]
    .into_iter()
    .map(|dir| dir.join("copilot.exe"))
    .find(|path| path.is_file())
}

/// Where the CLI runs: an empty folder of FastCapture's, so no project's instructions are read.
pub fn scratch_dir() -> PathBuf {
    crate::paths::local()
        .map(|local| local.join("copilot"))
        .unwrap_or_else(|| std::env::temp_dir().join("fastcapture-copilot"))
}

#[derive(Clone, Debug, PartialEq)]
pub enum StartError {
    /// It couldn't be started.
    Start(String),
    /// It ended before answering, with the first line it wrote to stderr.
    Ended(String),
    /// It didn't answer in time.
    TimedOut,
    /// It speaks another version of the protocol (none given by very old versions).
    Protocol(Option<u64>),
    /// It refused to start a connection.
    Refused(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Start(reason) => write!(f, "the Copilot CLI couldn't be started: {reason}"),
            Self::Ended(line) if line.is_empty() => f.write_str("the Copilot CLI ended at once"),
            Self::Ended(line) => write!(f, "the Copilot CLI ended at once: {line}"),
            Self::TimedOut => f.write_str("the Copilot CLI didn't answer in time"),
            Self::Protocol(Some(version)) => write!(
                f,
                "this Copilot CLI speaks protocol version {version}, and FastCapture speaks {PROTOCOL_VERSION}"
            ),
            Self::Protocol(None) => f.write_str("this Copilot CLI is too old for FastCapture"),
            Self::Refused(reason) => write!(f, "the Copilot CLI refused to connect: {reason}"),
        }
    }
}

/// A running Copilot CLI and the connection to it. Dropping it ends the CLI and anything it
/// started.
pub struct Server {
    connection: Connection,
    child: Child,
    job: Option<Job>,
    stderr: Arc<Mutex<Vec<u8>>>,
    /// `1.0.100`.
    pub version: String,
}

impl Server {
    /// Starts the CLI at `path` in `dir` and connects, waiting up to `timeout` for it.
    pub fn start(path: &Path, dir: &Path, timeout: Duration) -> Result<Self, StartError> {
        let mut command = Command::new(path);
        command
            .args(SERVER_ARGS)
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW);
        let job = Job::new();
        let mut child = command
            .spawn()
            .map_err(|error| StartError::Start(error.to_string()))?;
        let job = job.filter(|job| job.add(&child));
        let (Some(stdin), Some(stdout), Some(mut err)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            return Err(StartError::Start("its pipes couldn't be opened".to_owned()));
        };
        let stderr: Arc<Mutex<Vec<u8>>> = Arc::default();
        let kept = Arc::clone(&stderr);
        std::thread::spawn(move || {
            let mut chunk = [0; 1024];
            while let Ok(read @ 1..) = err.read(&mut chunk) {
                if let Ok(mut kept) = kept.lock() {
                    let room = STDERR_KEPT.saturating_sub(kept.len());
                    kept.extend_from_slice(&chunk[..read.min(room)]);
                }
            }
        });
        let mut server = Self {
            connection: Connection::new(stdout, stdin),
            child,
            job,
            stderr,
            version: String::new(),
        };
        server.connect(timeout)?;
        Ok(server)
    }

    /// The SDK's handshake, `connect`, or `ping` for a CLI from before it; either says which
    /// protocol the CLI speaks.
    fn connect(&mut self, timeout: Duration) -> Result<(), StartError> {
        let answer = match self.call("connect", json!({"supportedTaskKinds": []}), timeout) {
            Err(RpcError::Remote { code, .. }) if code == METHOD_NOT_FOUND => {
                self.call("ping", json!({"message": "fastcapture"}), timeout)
            }
            answer => answer,
        };
        let answer = answer.map_err(|error| match error {
            RpcError::Closed => StartError::Ended(self.ended_with()),
            RpcError::TimedOut => StartError::TimedOut,
            RpcError::Remote { message, .. } => StartError::Refused(message),
        })?;
        let protocol = answer.get("protocolVersion").and_then(Value::as_u64);
        if protocol != Some(PROTOCOL_VERSION) {
            return Err(StartError::Protocol(protocol));
        }
        self.version = answer
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Ok(())
    }

    pub fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, RpcError> {
        self.connection.call(method, params, timeout)
    }

    /// The next notification from the CLI.
    pub fn notification(&self, timeout: Duration) -> Result<Value, RpcError> {
        self.connection.notification(timeout)
    }

    /// The first line the CLI wrote to stderr, once it has ended (waiting a moment for it to).
    pub fn ended_with(&mut self) -> String {
        let until = Instant::now() + Duration::from_secs(2);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        // Its last words may still be in the pipe.
        std::thread::sleep(Duration::from_millis(50));
        let stderr = self
            .stderr
            .lock()
            .map(|kept| kept.clone())
            .unwrap_or_default();
        String::from_utf8_lossy(&stderr)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .to_owned()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        match &self.job {
            Some(job) => job.end(),
            None => {
                let _ = self.child.kill();
            }
        }
        let _ = self.child.wait();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::claude_code::cli::tests::Temp;

    /// A fake Copilot CLI: a PowerShell script that answers each request from `replies`, one
    /// line per method: the method, a tab, the reply's `"result":…` or `"error":…` member, and
    /// optionally more tabs, each a whole notification to send after the reply. A method with
    /// no line is refused as the real CLI refuses one. A line `!exit` and a message makes it
    /// write that to stderr and end at once; `!silent` makes it read and never answer. Each
    /// request it gets is kept, one per line, in `received.txt` beside it.
    pub(crate) fn fake(temp: &Temp, replies: &[String]) -> PathBuf {
        const SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$replies = @{}
foreach ($line in [IO.File]::ReadAllLines((Join-Path $here 'replies.txt'), [Text.Encoding]::UTF8)) {
    $parts = $line -split "`t"
    $replies[$parts[0]] = $parts
}
if ($replies.ContainsKey('!exit')) { [Console]::Error.WriteLine($replies['!exit'][1]); exit 1 }
$in = [Console]::OpenStandardInput()
$out = [Console]::OpenStandardOutput()
$log = Join-Path $here 'received.txt'
function Send-Message([string]$json) {
    $body = [Text.Encoding]::UTF8.GetBytes($json)
    $head = [Text.Encoding]::ASCII.GetBytes("Content-Length: $($body.Length)`r`n`r`n")
    $out.Write($head, 0, $head.Length); $out.Write($body, 0, $body.Length); $out.Flush()
}
function Read-Message {
    $header = New-Object 'System.Collections.Generic.List[byte]'
    while ($true) {
        $b = $in.ReadByte()
        if ($b -lt 0) { return $null }
        $header.Add([byte]$b)
        $n = $header.Count
        if ($n -ge 4 -and $header[$n - 4] -eq 13 -and $header[$n - 3] -eq 10 -and $header[$n - 2] -eq 13 -and $header[$n - 1] -eq 10) { break }
    }
    $text = [Text.Encoding]::ASCII.GetString($header.ToArray())
    $length = [int][regex]::Match($text, '(?i)content-length:\s*(\d+)').Groups[1].Value
    $body = New-Object byte[] $length
    $read = 0
    while ($read -lt $length) {
        $r = $in.Read($body, $read, $length - $read)
        if ($r -le 0) { return $null }
        $read += $r
    }
    return [Text.Encoding]::UTF8.GetString($body)
}
while ($null -ne ($text = Read-Message)) {
    [IO.File]::AppendAllText($log, $text + "`n", (New-Object Text.UTF8Encoding $false))
    if ($replies.ContainsKey('!silent')) { continue }
    $id = [regex]::Match($text, '"id":(\d+)').Groups[1].Value
    $method = [regex]::Match($text, '"method":"([^"]+)"').Groups[1].Value
    if (-not $replies.ContainsKey($method)) {
        Send-Message ('{"jsonrpc":"2.0","id":' + $id + ',"error":{"code":-32601,"message":"Unhandled method ' + $method + '"}}')
        continue
    }
    $parts = $replies[$method]
    Send-Message ('{"jsonrpc":"2.0","id":' + $id + ',' + $parts[1] + '}')
    for ($i = 2; $i -lt $parts.Count; $i++) { Send-Message $parts[$i] }
}
"#;
        std::fs::write(temp.0.join("fake.ps1"), SCRIPT).unwrap();
        std::fs::write(temp.0.join("replies.txt"), replies.join("\n")).unwrap();
        temp.script(
            "copilot.cmd",
            &[r#"powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%~dp0fake.ps1""#],
        )
    }

    /// The requests the fake got.
    pub(crate) fn received(temp: &Temp) -> Vec<Value> {
        std::fs::read_to_string(temp.0.join("received.txt"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// The reply to `connect` from today's CLI.
    pub(crate) fn connects() -> String {
        r#"connect	"result":{"ok":true,"protocolVersion":3,"version":"1.0.100"}"#.to_owned()
    }

    const WAIT: Duration = Duration::from_secs(20);

    #[test]
    fn it_connects_and_calls_the_cli() {
        let temp = Temp::new("copilot-start");
        let cli = fake(
            &temp,
            &[
                connects(),
                r#"auth.getStatus	"result":{"isAuthenticated":true,"login":"someone"}	{"jsonrpc":"2.0","method":"session.lifecycle","params":{"type":"x"}}"#.to_owned(),
            ],
        );
        let server = Server::start(&cli, &temp.0, WAIT).unwrap();
        assert_eq!(server.version, "1.0.100");
        assert_eq!(
            server.call("auth.getStatus", json!({}), WAIT).unwrap()["login"],
            "someone"
        );
        assert_eq!(server.notification(WAIT).unwrap()["params"]["type"], "x");
        assert!(matches!(
            server.call("models.list", json!({}), WAIT),
            Err(RpcError::Remote {
                code: METHOD_NOT_FOUND,
                ..
            })
        ));
        let got = received(&temp);
        assert_eq!(got[0]["method"], "connect");
        assert_eq!(got[0]["params"], json!({"supportedTaskKinds": []}));
        assert_eq!(got[1]["method"], "auth.getStatus");
    }

    #[test]
    fn an_older_cli_is_asked_with_ping_and_another_protocol_is_refused() {
        let temp = Temp::new("copilot-ping");
        let cli = fake(
            &temp,
            &[r#"ping	"result":{"message":"pong","protocolVersion":3}"#.to_owned()],
        );
        let server = Server::start(&cli, &temp.0, WAIT).unwrap();
        assert_eq!(server.version, "", "ping doesn't say");
        drop(server);

        let temp = Temp::new("copilot-protocol");
        let cli = fake(
            &temp,
            &[r#"connect	"result":{"ok":true,"protocolVersion":4,"version":"9.0.0"}"#.to_owned()],
        );
        let error = Server::start(&cli, &temp.0, WAIT).err().unwrap();
        assert_eq!(error, StartError::Protocol(Some(4)));
        assert!(error.to_string().contains("protocol version 4"));
    }

    #[test]
    fn a_cli_that_ends_at_once_says_why() {
        let temp = Temp::new("copilot-ends");
        let cli = fake(&temp, &["!exit\tError: no Copilot here".to_owned()]);
        assert_eq!(
            Server::start(&cli, &temp.0, WAIT).err(),
            Some(StartError::Ended("Error: no Copilot here".to_owned()))
        );
        assert!(matches!(
            Server::start(&temp.0.join("missing.exe"), &temp.0, WAIT),
            Err(StartError::Start(_))
        ));
    }

    #[test]
    fn a_cli_that_never_answers_times_out_and_is_ended() {
        let temp = Temp::new("copilot-silent");
        let cli = fake(&temp, &["!silent".to_owned()]);
        let started = Instant::now();
        assert_eq!(
            Server::start(&cli, &temp.0, Duration::from_secs(3)).err(),
            Some(StartError::TimedOut)
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(received(&temp).len(), 1, "it was asked once");
    }

    #[test]
    fn it_is_found_where_it_is_set_or_on_path_and_npm_s_shim_is_followed() {
        let temp = Temp::new("copilot-find");
        let first = temp.0.join("first");
        let npm = temp.0.join("npm");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&npm).unwrap();
        let path_variable = || Some(std::env::join_paths([&first, &npm]).unwrap());
        assert_eq!(find_in(None, path_variable()), None);

        let shim = npm.join("copilot.cmd");
        std::fs::write(&shim, b"").unwrap();
        assert_eq!(
            find_in(None, path_variable()),
            Some(shim.clone()),
            "a shim with nothing behind it"
        );

        let hoisted = npm.join(r"node_modules\@github").join(NPM_PACKAGE);
        std::fs::create_dir_all(&hoisted).unwrap();
        std::fs::write(hoisted.join("copilot.exe"), b"").unwrap();
        assert_eq!(
            find_in(None, path_variable()),
            Some(hoisted.join("copilot.exe"))
        );

        let nested = npm
            .join(r"node_modules\@github\copilot\node_modules\@github")
            .join(NPM_PACKAGE);
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("copilot.exe"), b"").unwrap();
        assert_eq!(
            find_in(None, path_variable()),
            Some(nested.join("copilot.exe")),
            "where npm puts it today"
        );
        assert_eq!(
            find_in(Some(&shim), None),
            Some(nested.join("copilot.exe")),
            "a set shim is followed too"
        );

        let exe = first.join("copilot.exe");
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(
            find_in(None, path_variable()),
            Some(exe.clone()),
            "an .exe before a .cmd"
        );
        assert_eq!(find_in(Some(&exe), None), Some(exe));
        assert_eq!(
            find_in(Some(&temp.0.join("missing.exe")), path_variable()),
            None,
            "a set path that is wrong isn't covered up"
        );
    }
}
