//! Protected storage for the Firebase refresh credential.
//!
//! Windows encrypts it with DPAPI for the current user; macOS keeps it in the
//! login Keychain. It is never written in plaintext, never logged, and never
//! crosses to the WebView. Removing it signs this device out; it does not touch
//! recordings, transcripts, settings or provider keys.
use super::{AccountKey, Display};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// One stored session. `Debug` redacts the credential.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stored {
    pub version: u8,
    pub project_id: String,
    pub uid: String,
    pub refresh_token: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}
impl Stored {
    pub fn new(key: &AccountKey, display: &Display, refresh_token: &str) -> Self {
        Self {
            version: 1,
            project_id: key.project_id.clone(),
            uid: key.uid.clone(),
            refresh_token: refresh_token.into(),
            email: display.email.clone(),
            name: display.name.clone(),
        }
    }
    fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("stored session serializes")
    }
    fn decode(bytes: &[u8]) -> Result<Self, ()> {
        let stored: Self = serde_json::from_slice(bytes).map_err(|_| ())?;
        let valid = stored.version == 1
            && !stored.project_id.is_empty()
            && !stored.uid.is_empty()
            && stored.uid.len() <= 128
            && !stored.refresh_token.is_empty();
        valid.then_some(stored).ok_or(())
    }
}
impl std::fmt::Debug for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stored")
            .field("project_id", &self.project_id)
            .field("refresh_token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Errors carry no detail: a failure to read means "not usable", nothing more.
pub trait Vault: Send + Sync {
    fn load(&self) -> Result<Option<Stored>, ()>;
    /// Replaces the stored session atomically.
    fn save(&self, stored: &Stored) -> Result<(), ()>;
    fn clear(&self) -> Result<(), ()>;
}

/// Process memory only. Used by tests and by targets without native protection.
#[derive(Default)]
#[cfg_attr(all(not(test), any(windows, target_os = "macos")), allow(dead_code))]
pub struct Memory(Mutex<Option<Stored>>);
impl Vault for Memory {
    fn load(&self) -> Result<Option<Stored>, ()> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, stored: &Stored) -> Result<(), ()> {
        *self.0.lock().unwrap() = Some(stored.clone());
        Ok(())
    }
    fn clear(&self) -> Result<(), ()> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

/// The platform's protected store for this data root.
pub fn platform(root: &std::path::Path) -> Box<dyn Vault> {
    #[cfg(windows)]
    return Box::new(Dpapi {
        dir: root.join("State/account"),
    });
    #[cfg(target_os = "macos")]
    return Box::new(Keychain {
        account: crate::identity::account(root, "google-session"),
    });
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = root;
        Box::new(Memory::default())
    }
}

#[cfg(windows)]
const MAGIC: &[u8] = b"MWACCT1\n";

/// A DPAPI blob for the current Windows user, beside the provider-key store.
#[cfg(windows)]
pub struct Dpapi {
    dir: std::path::PathBuf,
}
#[cfg(windows)]
impl Dpapi {
    fn path(&self) -> std::path::PathBuf {
        self.dir.join("google-session.bin")
    }
}
#[cfg(windows)]
impl Vault for Dpapi {
    fn load(&self) -> Result<Option<Stored>, ()> {
        use metrix_writer_api::desktop_settings::credentials::protected;
        let bytes = match std::fs::read(self.path()) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(()),
        };
        let blob = bytes.strip_prefix(MAGIC).ok_or(())?;
        let plain = protected::unprotect(blob).ok_or(())?;
        Stored::decode(&plain).map(Some)
    }
    fn save(&self, stored: &Stored) -> Result<(), ()> {
        use metrix_writer_api::desktop_settings::credentials::protected;
        use std::io::Write;
        let blob = protected::protect(&stored.encode()).ok_or(())?;
        std::fs::create_dir_all(&self.dir).map_err(|_| ())?;
        let temporary = self.dir.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        let written = (|| {
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(MAGIC)?;
            file.write_all(&blob)?;
            file.sync_all()?;
            // Replaces an existing session in one step (MoveFileEx with
            // MOVEFILE_REPLACE_EXISTING), so a crash leaves old or new, never half.
            std::fs::rename(&temporary, self.path())
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        written.map_err(|_| ())
    }
    fn clear(&self) -> Result<(), ()> {
        match std::fs::remove_file(self.path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(()),
        }
    }
}

/// A login-Keychain item, the same store as the macOS provider keys.
#[cfg(target_os = "macos")]
pub struct Keychain {
    account: String,
}
#[cfg(target_os = "macos")]
impl Keychain {
    fn entry(&self) -> Result<keyring::Entry, ()> {
        keyring::Entry::new(crate::identity::IDENTIFIER, &self.account).map_err(|_| ())
    }
}
#[cfg(target_os = "macos")]
impl Vault for Keychain {
    fn load(&self) -> Result<Option<Stored>, ()> {
        match self.entry()?.get_password() {
            Ok(text) => Stored::decode(text.as_bytes()).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(()),
        }
    }
    fn save(&self, stored: &Stored) -> Result<(), ()> {
        let text = String::from_utf8(stored.encode()).map_err(|_| ())?;
        // A Keychain item update replaces the whole value.
        self.entry()?.set_password(&text).map_err(|_| ())
    }
    fn clear(&self) -> Result<(), ()> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_credential() {
        let stored = Stored::new(
            &AccountKey {
                project_id: "metrix-writer".into(),
                uid: "uid-1".into(),
            },
            &Display::default(),
            "synthetic-refresh-credential",
        );
        let text = format!("{stored:?}");
        assert!(!text.contains("synthetic-refresh-credential"));
        assert!(!text.contains("uid-1"));
    }

    #[test]
    fn decode_rejects_incomplete_or_unknown_documents() {
        assert!(
            Stored::decode(br#"{"version":1,"project_id":"p","uid":"u","refresh_token":"r"}"#)
                .is_ok()
        );
        assert!(
            Stored::decode(br#"{"version":2,"project_id":"p","uid":"u","refresh_token":"r"}"#)
                .is_err()
        );
        assert!(
            Stored::decode(br#"{"version":1,"project_id":"p","uid":"","refresh_token":"r"}"#)
                .is_err()
        );
        assert!(Stored::decode(
            br#"{"version":1,"project_id":"p","uid":"u","refresh_token":"r","id_token":"x"}"#
        )
        .is_err());
        assert!(Stored::decode(b"not json").is_err());
    }
}
