//! Native credential entry. Key values never pass through a WebView or IPC.
use crate::storage::Result;
#[cfg(windows)]
use metrix_writer_api::desktop_settings::credentials::{protected, STORE_MAGIC};
use std::path::Path;
fn validate(provider: &str) -> Result<()> {
    if ["openai", "gemini", "grok", "openrouter"].contains(&provider) {
        Ok(())
    } else {
        Err("Unknown provider.".into())
    }
}
pub fn remove(root: &Path, provider: &str) -> Result<()> {
    validate(provider)?;
    #[cfg(target_os = "macos")]
    {
        match keyring_entry(root, provider)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(_) => return Err("Keychain could not remove the key.".into()),
        }
    }
    let path = root.join(format!("State/credentials/{provider}.cred"));
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Could not remove protected key.".into()),
    }
}
/// Runs the native dialog and stores what it returns.
///
/// The answer is `"saved"` or `"canceled"` — never the key. Both the Windows
/// dialog (`ERROR_CANCELLED`) and the macOS one report a dismissal as an empty
/// value, so an empty key is the cancel signal. Returning the same `Ok(())` for
/// both left the page unable to say whether anything had been stored.
pub fn prompt(root: &Path, provider: &str) -> Result<&'static str> {
    validate(provider)?;
    let mut key = native_prompt(provider)?;
    if key.is_empty() {
        return Ok("canceled");
    }
    let key_text = String::from_utf8_lossy(&key);
    let trimmed = key_text.trim();
    if trimmed.len() > 512 || !trimmed.is_ascii() || trimmed.chars().any(char::is_control) {
        return Err("Invalid API key.".into());
    }
    #[cfg(windows)]
    {
        let blob = protected::protect(trimmed.as_bytes()).ok_or("Could not protect API key.")?;
        let dir = root.join("State/credentials");
        std::fs::create_dir_all(&dir).map_err(|_| "Credential storage unavailable.")?;
        let path = dir.join(format!("{provider}.cred"));
        let mut file = STORE_MAGIC.to_vec();
        file.extend_from_slice(&blob);
        // DPAPI blob is already encrypted; replace atomically using the same helper
        // semantics as the recording journal, without ever writing plaintext.
        let temporary = dir.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&temporary, &file).map_err(|_| "Could not save protected key.")?;
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn MoveFileExW(a: *const u16, b: *const u16, flags: u32) -> i32;
        }
        let a: Vec<_> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let b: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), 9) } == 0 {
            return Err("Could not save protected key.".into());
        }
    }
    #[cfg(target_os = "macos")]
    keyring_entry(root, provider)?
        .set_password(trimmed)
        .map_err(|_| "Keychain could not save the key.")?;
    key.fill(0);
    Ok("saved")
}
#[cfg(windows)]
fn native_prompt(provider: &str) -> Result<Vec<u8>> {
    #[repr(C)]
    struct Info {
        size: u32,
        parent: isize,
        message: *const u16,
        caption: *const u16,
        banner: isize,
    }
    #[link(name = "credui")]
    extern "system" {
        fn CredUIPromptForCredentialsW(
            info: *const Info,
            target: *const u16,
            reserved: isize,
            error: u32,
            user: *mut u16,
            user_len: u32,
            password: *mut u16,
            password_len: u32,
            save: *mut i32,
            flags: u32,
        ) -> u32;
    }
    let message:Vec<_>=format!("Enter your {provider} API key in the password field. It will be protected for this Windows user.").encode_utf16().chain(Some(0)).collect();
    let title: Vec<_> = concat!(product_name!(), " API key")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let target: Vec<_> = format!("{}/{provider}", crate::identity::IDENTIFIER)
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let info = Info {
        size: std::mem::size_of::<Info>() as u32,
        parent: 0,
        message: message.as_ptr(),
        caption: title.as_ptr(),
        banner: 0,
    };
    let mut user = [0u16; 128];
    for (dst, src) in user.iter_mut().zip(provider.encode_utf16()) {
        *dst = src;
    }
    let mut password = [0u16; 513];
    let mut save = 0;
    let result = unsafe {
        CredUIPromptForCredentialsW(
            &info,
            target.as_ptr(),
            0,
            0,
            user.as_mut_ptr(),
            128,
            password.as_mut_ptr(),
            513,
            &mut save,
            0x40000 | 0x2 | 0x80,
        )
    };
    let length = password.iter().position(|v| *v == 0).unwrap_or(513);
    let key = String::from_utf16(&password[..length])
        .map_err(|_| "Invalid API key.")?
        .into_bytes();
    password.fill(0);
    match result {
        0 => Ok(key),
        1223 => Ok(Vec::new()),
        _ => Err("Native credential dialog failed.".into()),
    }
}
#[cfg(target_os = "macos")]
fn keyring_entry(root: &Path, provider: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(
        crate::identity::IDENTIFIER,
        &crate::identity::account(root, provider),
    )
    .map_err(|_| "Keychain unavailable.".into())
}
/// The login Keychain for the identity carry-over. Reading an item the
/// Metrix Writer app created makes macOS ask once whether to allow it.
#[cfg(target_os = "macos")]
pub struct Keychain;
#[cfg(target_os = "macos")]
impl crate::identity::Secrets for Keychain {
    fn get(&self, service: &str, account: &str) -> std::result::Result<Option<String>, ()> {
        match keyring::Entry::new(service, account)
            .map_err(|_| ())?
            .get_password()
        {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(()),
        }
    }
    fn set(&self, service: &str, account: &str, value: &str) -> std::result::Result<(), ()> {
        keyring::Entry::new(service, account)
            .and_then(|entry| entry.set_password(value))
            .map_err(|_| ())
    }
    fn delete(&self, service: &str, account: &str) -> std::result::Result<(), ()> {
        match keyring::Entry::new(service, account).and_then(|entry| entry.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(()),
        }
    }
}
#[cfg(target_os = "macos")]
fn native_prompt(provider: &str) -> Result<Vec<u8>> {
    let script=format!("text returned of (display dialog \"Enter your {provider} API key\" default answer \"\" with hidden answer buttons {{\"Cancel\", \"Save\"}} default button \"Save\")");
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .output()
        .map_err(|_| "Native key dialog unavailable.")?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    Ok(output.stdout)
}
