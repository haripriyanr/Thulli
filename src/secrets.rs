//! Secrets kept outside the data file. On Windows they live in the user's
//! Credential Manager, which Windows encrypts for this user account only.
//! Elsewhere nothing is stored and the key must be entered each run.

/// Credential Manager entry for the OpenRouter API key.
pub const AI_KEY: &str = "Thulli/openrouter-api-key";

#[cfg(target_os = "windows")]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Saves `value` under `name`, replacing any earlier value.
#[cfg(target_os = "windows")]
pub fn save(name: &str, value: &str) -> bool {
    use windows_sys::Win32::Security::Credentials::{
        CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };
    let mut target = wide(name);
    let mut user = wide("Thulli");
    let mut blob = value.as_bytes().to_vec();
    let credential = CREDENTIALW {
        Flags: 0,
        Type: CRED_TYPE_GENERIC,
        TargetName: target.as_mut_ptr(),
        Comment: std::ptr::null_mut(),
        LastWritten: unsafe { std::mem::zeroed() },
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: std::ptr::null_mut(),
        TargetAlias: std::ptr::null_mut(),
        UserName: user.as_mut_ptr(),
    };
    unsafe { CredWriteW(&credential, 0) != 0 }
}

/// The value saved under `name`, if any.
#[cfg(target_os = "windows")]
pub fn load(name: &str) -> Option<String> {
    use windows_sys::Win32::Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC};
    let target = wide(name);
    let mut found: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut found) == 0 || found.is_null() {
            return None;
        }
        let c = &*found;
        let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec();
        CredFree(found as *const _);
        String::from_utf8(bytes).ok().filter(|s| !s.is_empty())
    }
}

/// Removes the value saved under `name`.
#[cfg(target_os = "windows")]
pub fn remove(name: &str) -> bool {
    use windows_sys::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};
    let target = wide(name);
    unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) != 0 }
}

#[cfg(not(target_os = "windows"))]
pub fn save(_name: &str, _value: &str) -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn load(_name: &str) -> Option<String> {
    None
}

#[cfg(not(target_os = "windows"))]
pub fn remove(_name: &str) -> bool {
    false
}
