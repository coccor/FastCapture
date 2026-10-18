//! API keys in Windows Credential Manager, as generic credentials named `FastCapture/<backend>`,
//! kept for this user on this PC. A key is never logged, never put in an error message and
//! never written to a file.

use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
use windows_sys::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};

/// The credential name for `backend` (`claude`, `openai`…).
pub fn target(backend: &str) -> String {
    format!("FastCapture/{backend}")
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Saves `secret` under `target`, replacing any earlier one.
pub fn save(target: &str, secret: &str) -> crate::Result<()> {
    let mut name = wide(target);
    let mut user = wide("FastCapture");
    let mut blob = secret.as_bytes().to_vec();
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: name.as_mut_ptr(),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: user.as_mut_ptr(),
        ..Default::default()
    };
    let ok = unsafe { CredWriteW(&credential, 0) };
    blob.fill(0);
    if ok == 0 {
        return Err(crate::platform::last_error("CredWriteW"));
    }
    Ok(())
}

/// The secret saved under `target`, or `None` when there is none.
pub fn load(target: &str) -> Option<String> {
    let name = wide(target);
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    if unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
        if unsafe { GetLastError() } != ERROR_NOT_FOUND {
            crate::log::line(&format!("secrets: {target} couldn't be read"));
        }
        return None;
    }
    let secret = unsafe {
        let credential = &*credential;
        let bytes = std::slice::from_raw_parts(
            credential.CredentialBlob,
            credential.CredentialBlobSize as usize,
        );
        String::from_utf8(bytes.to_vec()).ok()
    };
    unsafe { CredFree(credential.cast()) };
    secret.filter(|secret| !secret.is_empty())
}

/// Removes the secret under `target`; nothing happens when there is none.
pub fn delete(target: &str) {
    let name = wide(target);
    unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_round_trips_and_is_gone_after_delete() {
        let name = target(&format!("test-{}", std::process::id()));
        assert_eq!(load(&name), None);
        save(&name, "sk-ant-test-1").unwrap();
        assert_eq!(load(&name).as_deref(), Some("sk-ant-test-1"));
        save(&name, "sk-ant-test-2").unwrap();
        assert_eq!(load(&name).as_deref(), Some("sk-ant-test-2"), "replaced");
        delete(&name);
        assert_eq!(load(&name), None);
        delete(&name);
    }
}
