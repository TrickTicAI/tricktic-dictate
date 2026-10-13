//! Provider credentials from the explicit server environment or the native protected
//! store written by the Windows setup module. Values are read on use and never
//! serialized, logged, echoed or accepted from HTTP.
use crate::transcription::ProviderId;
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// File layout: magic line, then a DPAPI (CurrentUser) blob protected with [`ENTROPY`].
pub const STORE_MAGIC: &[u8] = b"MWCRED1\n";
/// Part of every saved key's encryption, so it keeps its original name.
pub const ENTROPY: &[u8] = b"MetrixWriter.Credential.v1";
/// macOS Keychain service of the desktop app's provider keys: its bundle
/// identifier (`identifier` in apps/desktop/tauri/tauri.conf.json).
pub const KEYCHAIN_SERVICE: &str = "com.tricktic.dictate";
const VALUE_LIMIT: usize = 512;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStatus {
    Absent,
    Environment,
    ProtectedStore,
    /// A protected file exists but this process/platform cannot decrypt it.
    Unreadable,
}

pub struct Credential {
    pub value: String,
    pub status: CredentialStatus,
}

pub trait CredentialSource: Send + Sync {
    fn read(&self, provider: &ProviderId) -> Option<String>;
}

pub struct EnvironmentCredentials;
impl CredentialSource for EnvironmentCredentials {
    fn read(&self, provider: &ProviderId) -> Option<String> {
        std::env::var(variable(provider)?)
            .ok()
            .and_then(|v| sanitize(v.into_bytes()))
    }
}

pub fn variable(provider: &ProviderId) -> Option<&'static str> {
    match provider {
        ProviderId::Openai => Some("OPENAI_API_KEY"),
        ProviderId::Gemini => Some("GEMINI_API_KEY"),
        ProviderId::Grok => Some("XAI_API_KEY"),
        ProviderId::Openrouter => Some("OPENROUTER_API_KEY"),
        ProviderId::Local => None,
    }
}
pub fn file_name(provider: &ProviderId) -> Option<&'static str> {
    match provider {
        ProviderId::Openai => Some("openai.cred"),
        ProviderId::Gemini => Some("gemini.cred"),
        ProviderId::Grok => Some("grok.cred"),
        ProviderId::Openrouter => Some("openrouter.cred"),
        ProviderId::Local => None,
    }
}

fn sanitize(bytes: Vec<u8>) -> Option<String> {
    let value = String::from_utf8(bytes).ok()?;
    let value = value.trim();
    if value.is_empty()
        || value.len() > VALUE_LIMIT
        || !value.is_ascii()
        || value.chars().any(char::is_control)
    {
        return None;
    }
    Some(value.to_owned())
}

pub struct Credentials {
    dir: PathBuf,
    extra: Option<Arc<dyn CredentialSource>>,
}

impl Credentials {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            dir: state_dir.join("credentials"),
            extra: None,
        }
    }
    /// Additional trusted in-process source, used by tests instead of DPAPI.
    pub fn with_source(state_dir: &Path, source: Arc<dyn CredentialSource>) -> Self {
        Self {
            dir: state_dir.join("credentials"),
            extra: Some(source),
        }
    }
    pub fn store_path(&self, provider: &ProviderId) -> Option<PathBuf> {
        file_name(provider).map(|name| self.dir.join(name))
    }
    fn protected(&self, provider: &ProviderId) -> Result<Option<String>, ()> {
        #[cfg(target_os = "macos")]
        {
            let name = match provider {
                ProviderId::Openai => "openai",
                ProviderId::Gemini => "gemini",
                ProviderId::Grok => "grok",
                ProviderId::Openrouter => "openrouter",
                ProviderId::Local => return Ok(None),
            };
            let root = self.dir.parent().and_then(Path::parent).ok_or(())?;
            let entry =
                keyring::Entry::new(KEYCHAIN_SERVICE, &format!("{}:{name}", root.display()))
                    .map_err(|_| ())?;
            match entry.get_password() {
                Ok(value) => return sanitize(value.into_bytes()).map(Some).ok_or(()),
                Err(keyring::Error::NoEntry) => {}
                Err(_) => return Err(()),
            }
        }
        let Some(path) = self.store_path(provider) else {
            return Ok(None);
        };
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(()),
        };
        let Some(blob) = bytes.strip_prefix(STORE_MAGIC) else {
            return Err(());
        };
        match protected::unprotect(blob).and_then(sanitize) {
            Some(value) => Ok(Some(value)),
            None => Err(()),
        }
    }
    pub fn read(&self, provider: &ProviderId) -> Option<Credential> {
        if let Some(value) = EnvironmentCredentials.read(provider) {
            return Some(Credential {
                value,
                status: CredentialStatus::Environment,
            });
        }
        if let Some(value) = self
            .extra
            .as_ref()
            .and_then(|s| s.read(provider))
            .and_then(|v| sanitize(v.into_bytes()))
        {
            return Some(Credential {
                value,
                status: CredentialStatus::ProtectedStore,
            });
        }
        self.protected(provider)
            .ok()
            .flatten()
            .map(|value| Credential {
                value,
                status: CredentialStatus::ProtectedStore,
            })
    }
    pub fn status(&self, provider: &ProviderId) -> CredentialStatus {
        if let Some(credential) = self.read(provider) {
            return credential.status;
        }
        match self.protected(provider) {
            Err(()) => CredentialStatus::Unreadable,
            Ok(_) => CredentialStatus::Absent,
        }
    }
}

impl CredentialSource for Credentials {
    fn read(&self, provider: &ProviderId) -> Option<String> {
        Credentials::read(self, provider).map(|c| c.value)
    }
}

pub mod protected {
    //! Windows DPAPI binding without extra crates; other platforms cannot read the store.
    #[cfg(windows)]
    mod ffi {
        use std::ffi::c_void;
        #[repr(C)]
        pub struct DataBlob {
            pub cb: u32,
            pub pb: *mut u8,
        }
        #[link(name = "crypt32")]
        extern "system" {
            pub fn CryptProtectData(
                input: *const DataBlob,
                description: *const u16,
                entropy: *const DataBlob,
                reserved: *mut c_void,
                prompt: *const c_void,
                flags: u32,
                output: *mut DataBlob,
            ) -> i32;
            pub fn CryptUnprotectData(
                input: *const DataBlob,
                description: *mut *mut u16,
                entropy: *const DataBlob,
                reserved: *mut c_void,
                prompt: *const c_void,
                flags: u32,
                output: *mut DataBlob,
            ) -> i32;
        }
        #[link(name = "kernel32")]
        extern "system" {
            pub fn LocalFree(handle: *mut c_void) -> *mut c_void;
        }
        pub const UI_FORBIDDEN: u32 = 0x1;
    }

    #[cfg(windows)]
    fn call(input: &[u8], protect: bool) -> Option<Vec<u8>> {
        use ffi::*;
        if input.len() > u32::MAX as usize {
            return None;
        }
        let mut data = input.to_vec();
        let mut entropy = super::ENTROPY.to_vec();
        let input_blob = DataBlob {
            cb: data.len() as u32,
            pb: data.as_mut_ptr(),
        };
        let entropy_blob = DataBlob {
            cb: entropy.len() as u32,
            pb: entropy.as_mut_ptr(),
        };
        let mut output = DataBlob {
            cb: 0,
            pb: std::ptr::null_mut(),
        };
        // SAFETY: blobs point at live buffers for the call; output is freed with LocalFree.
        let ok = unsafe {
            if protect {
                CryptProtectData(
                    &input_blob,
                    std::ptr::null(),
                    &entropy_blob,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    UI_FORBIDDEN,
                    &mut output,
                )
            } else {
                CryptUnprotectData(
                    &input_blob,
                    std::ptr::null_mut(),
                    &entropy_blob,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    UI_FORBIDDEN,
                    &mut output,
                )
            }
        };
        if ok == 0 || output.pb.is_null() {
            return None;
        }
        // SAFETY: the API returned cb bytes at pb.
        let result = unsafe { std::slice::from_raw_parts(output.pb, output.cb as usize).to_vec() };
        unsafe { LocalFree(output.pb as *mut _) };
        Some(result)
    }

    #[cfg(windows)]
    pub fn protect(value: &[u8]) -> Option<Vec<u8>> {
        call(value, true)
    }
    #[cfg(windows)]
    pub fn unprotect(blob: &[u8]) -> Option<Vec<u8>> {
        call(blob, false)
    }
    #[cfg(not(windows))]
    pub fn protect(_: &[u8]) -> Option<Vec<u8>> {
        None
    }
    #[cfg(not(windows))]
    pub fn unprotect(_: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(&'static str);
    impl CredentialSource for Fixed {
        fn read(&self, provider: &ProviderId) -> Option<String> {
            matches!(provider, ProviderId::Openai).then(|| self.0.to_owned())
        }
    }

    #[test]
    fn environment_and_store_are_distinguished_and_malformed_files_are_unreadable() {
        let dir = std::env::temp_dir().join(format!("writer-cred-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("credentials")).unwrap();
        let credentials = Credentials::with_source(&dir, Arc::new(Fixed(" sk-fixture-value ")));
        assert_eq!(
            credentials.status(&ProviderId::Openai),
            CredentialStatus::ProtectedStore
        );
        assert_eq!(
            credentials.read(&ProviderId::Openai).unwrap().value,
            "sk-fixture-value"
        );
        assert_eq!(
            credentials.status(&ProviderId::Gemini),
            CredentialStatus::Absent
        );
        assert_eq!(
            credentials.status(&ProviderId::Local),
            CredentialStatus::Absent
        );
        std::fs::write(dir.join("credentials/gemini.cred"), b"not a protected blob").unwrap();
        assert_eq!(
            credentials.status(&ProviderId::Gemini),
            CredentialStatus::Unreadable
        );
        assert!(credentials.read(&ProviderId::Gemini).is_none());
        assert!(sanitize(b"line\nbreak".to_vec()).is_none());
        assert!(sanitize(vec![b'x'; 513]).is_none());
        assert!(sanitize(b"   ".to_vec()).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_store_round_trips_for_the_current_user_only() {
        let dir = std::env::temp_dir().join(format!("writer-cred-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("credentials")).unwrap();
        let blob = protected::protect(b"sk-windows-fixture").unwrap();
        assert!(!blob.windows(18).any(|w| w == b"sk-windows-fixture"));
        let mut file = STORE_MAGIC.to_vec();
        file.extend_from_slice(&blob);
        std::fs::write(dir.join("credentials/openai.cred"), file).unwrap();
        let credentials = Credentials::new(&dir);
        assert_eq!(
            credentials.status(&ProviderId::Openai),
            CredentialStatus::ProtectedStore
        );
        assert_eq!(
            credentials.read(&ProviderId::Openai).unwrap().value,
            "sk-windows-fixture"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn keychain_service_is_the_desktop_bundle_identifier() {
        let config: serde_json::Value = serde_json::from_str(include_str!(
            "../../../desktop/tauri/tauri.conf.json"
        ))
        .unwrap();
        assert_eq!(config["identifier"], KEYCHAIN_SERVICE);
    }
}
