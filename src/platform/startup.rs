//! Starting at sign-in (spec §5.1): a value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, kept in step with `start_at_sign_in`.

use super::last_error;
use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The value name the app uses.
pub const VALUE: &str = "FastCapture";

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Sets `value` to start `command` at sign-in, or removes it.
pub fn set(value: &str, command: Option<&str>) -> crate::Result<()> {
    let key = wide(RUN_KEY);
    let name = wide(value);
    match command {
        Some(command) => {
            let data = wide(command);
            let status = unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    name.as_ptr(),
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            };
            if status != 0 {
                return Err(crate::Error::Win32 {
                    call: "RegSetKeyValueW",
                    code: status,
                });
            }
        }
        None => {
            let status =
                unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
            if status != 0 && status != ERROR_FILE_NOT_FOUND {
                return Err(crate::Error::Win32 {
                    call: "RegDeleteKeyValueW",
                    code: status,
                });
            }
        }
    }
    Ok(())
}

/// The command that starts this executable: its path in quotes.
pub fn this_command() -> crate::Result<String> {
    let exe = std::env::current_exe().map_err(|_| last_error("GetModuleFileNameW"))?;
    Ok(format!("\"{}\"", exe.display()))
}

/// The `Run` value's data, if set.
pub fn get(value: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_SZ, RegGetValueW};
    let key = wide(RUN_KEY);
    let name = wide(value);
    let mut buffer = vec![0u16; 2048];
    let mut bytes = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != 0 {
        return None;
    }
    buffer.truncate((bytes as usize / 2).saturating_sub(1));
    Some(String::from_utf16_lossy(&buffer))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_run_value_is_set_and_removed() {
        let value = format!("FastCaptureTest-{}", std::process::id());
        super::set(
            &value,
            Some(r#""C:\Program Files\FastCapture\fastcapture.exe""#),
        )
        .unwrap();
        assert_eq!(
            super::get(&value).as_deref(),
            Some(r#""C:\Program Files\FastCapture\fastcapture.exe""#)
        );
        super::set(&value, None).unwrap();
        assert_eq!(super::get(&value), None);
        super::set(&value, None).unwrap();
    }
}
