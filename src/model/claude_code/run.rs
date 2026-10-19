//! Running a program once: stdin written and closed, stdout and stderr read on threads of their
//! own so a large reply can't block it, no console window, a working folder, some environment
//! variables removed, and a time limit. The program and anything it starts run in a job object,
//! so a timeout or a finished run leaves nothing behind.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How much of stderr is kept.
const STDERR_KEPT: usize = 4096;

#[derive(Debug, Eq, PartialEq)]
pub struct Output {
    /// The exit code, when there was one.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    /// The start of stderr.
    pub stderr: String,
}

#[derive(Debug, Eq, PartialEq)]
pub enum RunError {
    /// It couldn't be started (not found, refused arguments).
    Start(String),
    /// It ran past the time limit and was stopped.
    TimedOut,
}

/// A job object that ends every process in it when it is closed.
struct Job(HANDLE);

impl Job {
    fn new() -> Option<Self> {
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return None;
        }
        let job = Self(handle);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        (ok != 0).then_some(job)
    }

    fn add(&self, child: &std::process::Child) -> bool {
        unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) != 0 }
    }

    fn end(&self) {
        unsafe { TerminateJobObject(self.0, 1) };
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Runs `program` with `args` in `dir`, writes `stdin` to it, and waits up to `timeout`. A
/// `.cmd` or `.bat` goes through `cmd.exe`, with the standard library's own quoting for it.
pub fn run(
    program: &Path,
    args: &[String],
    stdin: &[u8],
    dir: &Path,
    remove: &[OsString],
    timeout: Duration,
) -> Result<Output, RunError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    for name in remove {
        command.env_remove(name);
    }
    let job = Job::new();
    let mut child = command
        .spawn()
        .map_err(|error| RunError::Start(error.to_string()))?;
    let in_job = job.as_ref().is_some_and(|job| job.add(&child));

    let mut input = child.stdin.take();
    let stdin = stdin.to_vec();
    std::thread::spawn(move || {
        if let Some(input) = input.as_mut() {
            let _ = input.write_all(&stdin);
        }
        // Dropped here, which closes it: the program sees the end of its input.
    });
    let mut out = child.stdout.take();
    let stdout = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(out) = out.as_mut() {
            let _ = out.read_to_end(&mut bytes);
        }
        bytes
    });
    let mut err = child.stderr.take();
    let stderr = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(err) = err.as_mut() {
            let _ = err.read_to_end(&mut bytes);
        }
        bytes.truncate(STDERR_KEPT);
        String::from_utf8_lossy(&bytes).into_owned()
    });

    let until = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                match &job {
                    Some(job) if in_job => job.end(),
                    _ => {
                        let _ = child.kill();
                    }
                }
                let _ = child.wait();
                // Without the job, something the program started may still hold the pipes, so
                // the reading threads are left to finish on their own.
                return Err(RunError::TimedOut);
            }
        }
    };
    // Anything the program left running is ended, so the pipes close.
    drop(job);
    Ok(Output {
        code: status.code(),
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::claude_code::cli::tests::Temp;

    fn run_script(
        temp: &Temp,
        lines: &[&str],
        stdin: &[u8],
        remove: &[&str],
    ) -> Result<Output, RunError> {
        let script = temp.script("fake.cmd", lines);
        let remove: Vec<OsString> = remove.iter().map(OsString::from).collect();
        run(
            &script,
            &[],
            stdin,
            &temp.0,
            &remove,
            Duration::from_secs(10),
        )
    }

    #[test]
    fn stdin_goes_in_and_stdout_stderr_and_the_exit_code_come_back() {
        let temp = Temp::new("run");
        let output = run_script(
            &temp,
            &[r#"findstr "^""#, "echo oops 1>&2", "exit /b 3"],
            b"{\"type\":\"user\"}\r\n",
            &[],
        )
        .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "{\"type\":\"user\"}"
        );
        assert_eq!(output.stderr.trim(), "oops");
        assert_eq!(output.code, Some(3));
    }

    #[test]
    fn it_runs_in_the_folder_given_without_the_variables_removed() {
        let temp = Temp::new("run-env");
        let output = run_script(
            &temp,
            &["echo [%COMPUTERNAME%]", "cd"],
            b"",
            &["COMPUTERNAME"],
        )
        .unwrap();
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("[]"));
        assert_eq!(
            lines.next().map(|line| line.to_ascii_lowercase()),
            Some(temp.0.display().to_string().to_ascii_lowercase())
        );
    }

    #[test]
    fn a_run_past_its_time_is_stopped_with_what_it_started() {
        let temp = Temp::new("run-timeout");
        let script = temp.script("slow.cmd", &["ping -n 30 127.0.0.1 >nul"]);
        let started = Instant::now();
        assert_eq!(
            run(&script, &[], b"", &temp.0, &[], Duration::from_millis(500)),
            Err(RunError::TimedOut)
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn an_empty_argument_and_json_reach_a_cmd_as_separate_arguments() {
        let temp = Temp::new("run-args");
        let script = temp.script("args.cmd", &["echo 1=[%1] 2=[%2] 3=[%3] 4=[%4]"]);
        let args = [
            "--tools",
            "",
            "--json-schema",
            r#"{"type":"object","required":["title"]}"#,
        ]
        .map(str::to_owned);
        let output = run(&script, &args, b"", &temp.0, &[], Duration::from_secs(10)).unwrap();
        // Quotes inside an argument are doubled for cmd.exe, which a .cmd shim passes on as it
        // is; the program it starts reads "" inside quotes as one quote.
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            r#"1=[--tools] 2=[""] 3=[--json-schema] 4=["{""type"":""object"",""required"":[""title""]}"]"#
        );
    }

    #[test]
    fn a_missing_program_doesnt_start() {
        let temp = Temp::new("run-missing");
        assert!(matches!(
            run(
                &temp.0.join("nope.exe"),
                &[],
                b"",
                &temp.0,
                &[],
                Duration::from_secs(1)
            ),
            Err(RunError::Start(_))
        ));
    }
}
