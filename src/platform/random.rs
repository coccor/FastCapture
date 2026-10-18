//! Random bytes from the system's preferred generator (`BCryptGenRandom`).

use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};

/// Fills `bytes` with random bytes. The system generator only fails on invalid arguments, so a
/// failure is a bug and panics.
pub fn fill(bytes: &mut [u8]) {
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            u32::try_from(bytes.len()).expect("a small buffer"),
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    assert_eq!(status, 0, "BCryptGenRandom failed with 0x{status:08X}");
}

#[cfg(test)]
mod tests {
    #[test]
    fn two_fills_differ() {
        let (mut a, mut b) = ([0u8; 16], [0u8; 16]);
        super::fill(&mut a);
        super::fill(&mut b);
        assert_ne!(a, b);
    }
}
