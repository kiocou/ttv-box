//! Credential protection boundary.
//!
//! Windows uses the current-user DPAPI. Other platforms return an explicit
//! unsupported error instead of silently storing credentials in plaintext.

use crate::error::AppError;

#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    #[error("credential protection is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("DPAPI operation failed with Windows error {0}")]
    Dpapi(u32),
    #[error("DPAPI returned invalid data")]
    InvalidData,
}

impl From<SecurityError> for AppError {
    fn from(value: SecurityError) -> Self {
        Self::Security(value.to_string())
    }
}

pub struct Dpapi;

impl Dpapi {
    pub fn encrypt(plaintext: &[u8]) -> Result<Vec<u8>, SecurityError> {
        protect(plaintext)
    }
    pub fn decrypt(ciphertext: &[u8]) -> Result<Vec<u8>, SecurityError> {
        unprotect(ciphertext)
    }
}

#[cfg(windows)]
fn protect(plaintext: &[u8]) -> Result<Vec<u8>, SecurityError> {
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: plaintext
            .len()
            .try_into()
            .map_err(|_| SecurityError::InvalidData)?,
        pbData: plaintext.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &input,
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(SecurityError::Dpapi(
            std::io::Error::last_os_error().raw_os_error().unwrap_or(1) as u32,
        ));
    }
    if output.pbData.is_null() && output.cbData != 0 {
        return Err(SecurityError::InvalidData);
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut core::ffi::c_void);
    }
    Ok(bytes)
}

#[cfg(windows)]
fn unprotect(ciphertext: &[u8]) -> Result<Vec<u8>, SecurityError> {
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: ciphertext
            .len()
            .try_into()
            .map_err(|_| SecurityError::InvalidData)?,
        pbData: ciphertext.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(SecurityError::Dpapi(
            std::io::Error::last_os_error().raw_os_error().unwrap_or(1) as u32,
        ));
    }
    if output.pbData.is_null() && output.cbData != 0 {
        return Err(SecurityError::InvalidData);
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut core::ffi::c_void);
    }
    Ok(bytes)
}

#[cfg(not(windows))]
fn protect(_plaintext: &[u8]) -> Result<Vec<u8>, SecurityError> {
    Err(SecurityError::UnsupportedPlatform)
}

#[cfg(not(windows))]
fn unprotect(_ciphertext: &[u8]) -> Result<Vec<u8>, SecurityError> {
    Err(SecurityError::UnsupportedPlatform)
}

/// Serializes a provider-owned session, encrypts it with DPAPI and stores the
/// result as hex in `kv`; no token fields are interpreted here.
pub struct CredentialStore<'a> {
    database: &'a crate::storage::Database,
}

impl<'a> CredentialStore<'a> {
    pub fn new(database: &'a crate::storage::Database) -> Self {
        Self { database }
    }

    pub fn save_json<T: serde::Serialize>(&self, key: &str, value: &T) -> Result<(), AppError> {
        let plaintext =
            serde_json::to_vec(value).map_err(|error| AppError::Security(error.to_string()))?;
        let encrypted = Dpapi::encrypt(&plaintext)?;
        self.database.kv_set(key, &hex_encode(&encrypted))
    }

    pub fn load_json<T: serde::de::DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<T>, AppError> {
        let Some(encoded) = self.database.kv_get(key)? else {
            return Ok(None);
        };
        let encrypted = hex_decode(&encoded).map_err(AppError::Security)?;
        let plaintext = Dpapi::decrypt(&encrypted)?;
        serde_json::from_slice(&plaintext)
            .map(Some)
            .map_err(|error| AppError::Security(format!("invalid protected credential: {error}")))
    }

    pub fn delete(&self, key: &str) -> Result<bool, AppError> {
        self.database.kv_delete(key)
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn hex_decode(value: &str) -> Result<Vec<u8>, String> {
    if value.len() % 2 != 0 {
        return Err("protected credential encoding has odd length".to_owned());
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = hex_digit(pair[0])
            .ok_or_else(|| "protected credential encoding is invalid".to_owned())?;
        let low = hex_digit(pair[1])
            .ok_or_else(|| "protected credential encoding is invalid".to_owned())?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hex_round_trip() {
        let bytes = [0, 1, 15, 16, 255];
        assert_eq!(hex_decode(&hex_encode(&bytes)).unwrap(), bytes);
    }
    #[cfg(not(windows))]
    #[test]
    fn non_windows_is_explicitly_unsupported() {
        assert!(matches!(
            Dpapi::encrypt(b"secret"),
            Err(SecurityError::UnsupportedPlatform)
        ));
    }
    #[cfg(windows)]
    #[test]
    fn windows_dpapi_round_trip() {
        let encrypted = Dpapi::encrypt(b"secret").unwrap();
        assert_ne!(encrypted, b"secret");
        assert_eq!(Dpapi::decrypt(&encrypted).unwrap(), b"secret");
    }
}
