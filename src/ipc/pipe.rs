//! The named pipe between `fastcapture add` and the tray app (spec §5.1), after FastPad's
//! `ipc` module: one pipe per Windows session, `\\.\pipe\FastCapture-<session>`, that only the
//! current user can open and that refuses remote clients. Unlike FastPad's inbound pipe it is
//! duplex: the tray app answers each request with where the capture went. The server blocks on
//! a thread of its own; the client verifies the server's user and session before sending.

use super::protocol::{self, Reply, Request};
use super::security::{CurrentUserAcl, ProcessIdentity};
use crate::platform::{last_error, wide_path};
use crate::{Error, Result};
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE,
    GetLastError, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_SHARE_NONE, FlushFileBuffers, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;

/// How long `add` waits for the tray app's answer.
pub const REPLY_BUDGET: Duration = Duration::from_secs(5);

pub(crate) fn session_of(process_id: u32) -> Result<u32> {
    let mut session = 0;
    if unsafe { ProcessIdToSessionId(process_id, &mut session) } == 0 {
        return Err(last_error("ProcessIdToSessionId"));
    }
    Ok(session)
}

/// `\\.\pipe\FastCapture-<session>` for this process's session.
pub fn pipe_name() -> Result<PathBuf> {
    Ok(pipe_name_for(session_of(unsafe { GetCurrentProcessId() })?))
}

fn pipe_name_for(session: u32) -> PathBuf {
    PathBuf::from(format!(r"\\.\pipe\FastCapture-{session}"))
}

/// Reads one length-prefixed frame body.
fn read_frame(file: &mut File) -> std::result::Result<Vec<u8>, String> {
    let mut prefix = [0u8; 4];
    file.read_exact(&mut prefix)
        .map_err(|error| error.to_string())?;
    let mut body = vec![0u8; protocol::body_length(prefix)?];
    file.read_exact(&mut body)
        .map_err(|error| error.to_string())?;
    Ok(body)
}

/// The tray app's end of the pipe.
pub struct Server {
    pipe: File,
}

impl Server {
    /// Creates the pipe. Fails when another process already has it (a second tray app, or a
    /// squatter): `FILE_FLAG_FIRST_PIPE_INSTANCE` makes sure this process is its only server.
    pub fn create() -> Result<Self> {
        let name = wide_path(&pipe_name()?);
        let security = CurrentUserAcl::current()?;
        let attributes = security.attributes();
        let raw = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64 * 1024,
                64 * 1024,
                0,
                &attributes,
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(last_error("CreateNamedPipeW"));
        }
        let pipe = File::from(unsafe { OwnedHandle::from_raw_handle(raw) });
        Ok(Self { pipe })
    }

    /// Answers requests with `handle` until `stop` is set; `wake` then makes the waiting
    /// `ConnectNamedPipe` return. A broken or invalid request is dropped without an answer.
    pub fn serve(&mut self, stop: &AtomicBool, mut handle: impl FnMut(Request) -> Reply) {
        while !stop.load(Ordering::SeqCst) {
            let connected =
                unsafe { ConnectNamedPipe(self.pipe.as_raw_handle(), std::ptr::null_mut()) } != 0
                    || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if connected && !stop.load(Ordering::SeqCst) {
                match read_frame(&mut self.pipe).and_then(|body| protocol::decode_request(&body)) {
                    Ok(request) => {
                        let reply = protocol::encode_reply(&handle(request));
                        if self.pipe.write_all(&reply).is_ok() {
                            unsafe { FlushFileBuffers(self.pipe.as_raw_handle()) };
                        }
                    }
                    Err(error) => crate::log::line(&format!("pipe: request dropped ({error})")),
                }
            }
            unsafe { DisconnectNamedPipe(self.pipe.as_raw_handle()) };
        }
    }
}

/// Unblocks a `Server::serve` waiting for a client, after its `stop` flag was set.
pub fn wake() {
    if let Ok(name) = pipe_name() {
        let _ = open_client(&name);
    }
}

#[derive(Debug)]
pub enum SendError {
    /// No tray app is running in this session: `add` files the capture itself.
    NotRunning,
    /// Something went wrong talking to it; the message is for stderr.
    Failed(String),
}

fn open_client(name: &std::path::Path) -> std::result::Result<File, u32> {
    let wide = wide_path(name);
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_NONE,
            std::ptr::null(),
            OPEN_EXISTING,
            // The server may identify this client but never act as it.
            SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(unsafe { GetLastError() });
    }
    Ok(File::from(unsafe { OwnedHandle::from_raw_handle(raw) }))
}

/// Sends `request` to the tray app and returns its reply.
pub fn send(request: &Request) -> std::result::Result<Reply, SendError> {
    let frame = protocol::encode_request(request).map_err(SendError::Failed)?;
    let name = pipe_name().map_err(|error| SendError::Failed(error.to_string()))?;
    let wide = wide_path(&name);
    let mut pipe = loop {
        match open_client(&name) {
            Ok(pipe) => break pipe,
            Err(ERROR_FILE_NOT_FOUND) => return Err(SendError::NotRunning),
            Err(ERROR_PIPE_BUSY) => {
                // Another `add` is being answered: wait for the pipe, then try again.
                if unsafe { WaitNamedPipeW(wide.as_ptr(), REPLY_BUDGET.as_millis() as u32) } == 0 {
                    return Err(SendError::Failed(
                        "FastCapture is busy; try again.".to_owned(),
                    ));
                }
            }
            Err(code) => {
                return Err(SendError::Failed(
                    Error::Win32 {
                        call: "CreateFileW",
                        code,
                    }
                    .to_string(),
                ));
            }
        }
    };
    let mut server_process = 0;
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut server_process) } == 0 {
        return Err(SendError::Failed(
            last_error("GetNamedPipeServerProcessId").to_string(),
        ));
    }
    let trusted = ProcessIdentity::of_process(server_process)
        .and_then(|server| Ok(ProcessIdentity::current()?.trusts_server(&server)));
    if !matches!(trusted, Ok(true)) {
        return Err(SendError::Failed(
            "The FastCapture pipe belongs to another user or session.".to_owned(),
        ));
    }

    // The exchange runs on a helper thread so a stuck server can't hang `add`.
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = pipe
            .write_all(&frame)
            .map_err(|error| error.to_string())
            .and_then(|()| read_frame(&mut pipe))
            .and_then(|body| protocol::decode_reply(&body));
        let _ = sender.send(result);
    });
    match receiver.recv_timeout(REPLY_BUDGET) {
        Ok(Ok(reply)) => Ok(reply),
        Ok(Err(error)) => Err(SendError::Failed(format!(
            "FastCapture didn't answer properly: {error}"
        ))),
        Err(_) => Err(SendError::Failed(
            "FastCapture didn't answer in time.".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn names_are_scoped_to_the_session() {
        assert_eq!(pipe_name_for(3), PathBuf::from(r"\\.\pipe\FastCapture-3"));
        let name = pipe_name().unwrap();
        assert!(name.to_string_lossy().starts_with(r"\\.\pipe\FastCapture-"));
    }

    /// The real pipe for this session: a running tray app would make this test fail with
    /// "already exists", which is the behaviour it checks too.
    #[test]
    fn a_request_gets_the_servers_reply_and_a_second_server_is_refused() {
        let mut server = Server::create().unwrap();
        assert!(Server::create().is_err(), "only one server per session");
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::spawn(move || {
            server.serve(&thread_stop, |request| match request {
                Request::Add { text, .. } => Reply::Filed(PathBuf::from(format!(r"C:\{text}.md"))),
            });
        });
        for text in ["one", "two"] {
            let reply = send(&Request::Add {
                text: text.to_owned(),
                attachments: Vec::new(),
                notebook: None,
            })
            .unwrap();
            assert_eq!(reply, Reply::Filed(PathBuf::from(format!(r"C:\{text}.md"))));
        }
        stop.store(true, Ordering::SeqCst);
        wake();
        thread.join().unwrap();
        assert!(matches!(
            send(&Request::Add {
                text: "x".to_owned(),
                attachments: Vec::new(),
                notebook: None,
            }),
            Err(SendError::NotRunning)
        ));
    }
}
