//! DPAPI: encrypt small secrets so they are bound to the current Windows user.

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
};

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

/// Encrypt `plain` for this user; returns `None` if Windows refuses.
pub fn protect(plain: &str) -> Option<Vec<u8>> {
    // SAFETY: the input blob points at a live slice and the output blob is
    // copied out and released with LocalFree.
    unsafe {
        let input = blob(plain.as_bytes());
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        if ok == 0 {
            return None;
        }
        let data = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData as _);
        Some(data)
    }
}

/// Decrypt a blob produced by [`protect`] for this user.
pub fn unprotect(cipher: &[u8]) -> Option<String> {
    // SAFETY: same contract as `protect`.
    unsafe {
        let input = blob(cipher);
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        if ok == 0 {
            return None;
        }
        let data = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData as _);
        String::from_utf8(data).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let Some(cipher) = protect("hunter2") else {
            // DPAPI can be unavailable in some CI sandboxes; skip then.
            return;
        };
        assert_ne!(cipher, b"hunter2");
        assert_eq!(unprotect(&cipher).as_deref(), Some("hunter2"));
    }
}
