//! Desktop identity and the one-time carry-over from the Metrix Writer identity.
//!
//! Until 0.39 the desktop app was `com.metrix.writer` with its data in a
//! `MetrixWriter` folder. The first launch of the `com.tricktic.dictate` app
//! moves that folder, and on macOS its Keychain items and WebKit storage, to
//! the new names. Each move is a rename on the same volume or a copy that is
//! read back before the original is deleted, so an interrupted or refused
//! carry-over leaves the old copy intact and the next launch tries again.
//! See docs/release/identity.md.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// The bundle identifier. It must equal `identifier` in tauri.conf.json.
pub const IDENTIFIER: &str = "com.tricktic.dictate";
pub const LEGACY_IDENTIFIER: &str = "com.metrix.writer";
/// The data folder under `%LOCALAPPDATA%` or `~/Library/Application Support`.
pub const DATA_FOLDER: &str = IDENTIFIER;
pub const LEGACY_DATA_FOLDER: &str = "MetrixWriter";
/// Keychain accounts are `<data root>:<name>`: the provider keys and the
/// account session (`account::vault`).
pub const SECRET_NAMES: [&str; 5] = ["openai", "gemini", "grok", "openrouter", "google-session"];
/// Written into `State` once every Keychain item has been carried over.
const SECRETS_DONE: &str = "State/identity-keychain-carried-over";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Neither folder exists: a new installation.
    Fresh,
    /// Only the new folder exists.
    Current,
    /// The Metrix Writer folder became the new folder.
    Moved,
    /// The previous app still holds its data; nothing was moved.
    InUse,
    /// The rename kept failing; the old folder stays in use until a later launch.
    Failed,
    /// Both exist. The new one is used and the old one is left untouched.
    Both,
}

/// The ordinary data root under `base` for this launch: the new folder, unless
/// only a Metrix Writer folder exists because its carry-over has not happened.
pub fn resolve(base: &Path) -> PathBuf {
    let current = base.join(DATA_FOLDER);
    let legacy = base.join(LEGACY_DATA_FOLDER);
    if !current.exists() && legacy.is_dir() {
        legacy
    } else {
        current
    }
}

/// Moves `base/MetrixWriter` to `base/com.tricktic.dictate` when only the old
/// folder exists. A rename on one volume never leaves a partial copy: the
/// folder is either still at its old path or complete at its new one. Windows
/// refuses it while any file inside is open, for example by a WebView2 process
/// of the previous app that is still exiting, so it is retried for `wait`.
pub fn carry_over_root(base: &Path, wait: Duration) -> Outcome {
    let current = base.join(DATA_FOLDER);
    let legacy = base.join(LEGACY_DATA_FOLDER);
    match (current.exists(), legacy.is_dir()) {
        (false, false) => return Outcome::Fresh,
        (true, false) => return Outcome::Current,
        (true, true) => return Outcome::Both,
        (false, true) => {}
    }
    // macOS renames a folder whose files are open, so a running previous app
    // must be detected through its data lease, not through a failed rename.
    if in_use(&legacy) {
        return Outcome::InUse;
    }
    let step = Duration::from_millis(250);
    let mut waited = Duration::ZERO;
    loop {
        match std::fs::rename(&legacy, &current) {
            Ok(()) => return Outcome::Moved,
            Err(_) if waited < wait => {
                std::thread::sleep(step);
                waited += step;
            }
            Err(_) => return Outcome::Failed,
        }
    }
}

/// The previous app holds `Data/desktop.lock` for as long as it runs; Windows
/// opens it without sharing, so even opening it fails then.
fn in_use(root: &Path) -> bool {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("Data/desktop.lock"))
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let locked = fs2::FileExt::try_lock_exclusive(&file).is_err();
    if !locked {
        let _ = fs2::FileExt::unlock(&file);
    }
    locked
}

/// Renames a per-identity folder such as `~/Library/WebKit/com.metrix.writer`
/// when the new one does not exist yet. Returns whether it was moved.
pub fn carry_over_folder(parent: &Path) -> std::io::Result<bool> {
    let legacy = parent.join(LEGACY_IDENTIFIER);
    let current = parent.join(IDENTIFIER);
    if current.exists() || !legacy.is_dir() {
        return Ok(false);
    }
    std::fs::rename(legacy, current).map(|()| true)
}

/// The Keychain as the carry-over sees it: generic passwords by service and account.
pub trait Secrets {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, ()>;
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), ()>;
    fn delete(&self, service: &str, account: &str) -> Result<(), ()>;
}

pub fn account(root: &Path, name: &str) -> String {
    format!("{}:{name}", root.display())
}

/// Copies each Metrix Writer Keychain item to the new service and the account
/// for `root`, reads it back, and only then deletes the original. An item macOS
/// refuses to read (the user denied access) is left where it is. Returns
/// whether every item is now settled; the caller retries on a later launch
/// otherwise.
pub fn carry_over_secrets(store: &dyn Secrets, root: &Path, legacy_root: &Path) -> bool {
    let mut settled = true;
    for name in SECRET_NAMES {
        let target = account(root, name);
        let sources = [
            (LEGACY_IDENTIFIER, account(legacy_root, name)),
            (LEGACY_IDENTIFIER, target.clone()),
            (IDENTIFIER, account(legacy_root, name)),
        ];
        let sources = sources
            .iter()
            .filter(|(service, account)| (*service, account.as_str()) != (IDENTIFIER, &target));
        let mut current = match store.get(IDENTIFIER, &target) {
            Ok(value) => value,
            Err(()) => {
                settled = false;
                continue;
            }
        };
        for (service, source) in sources {
            let value = match store.get(service, source) {
                Ok(Some(value)) => value,
                Ok(None) => continue,
                Err(()) => {
                    settled = false;
                    continue;
                }
            };
            match &current {
                // Already carried over and only the delete was missed.
                Some(existing) if *existing == value => {}
                // A newer item exists for the new identity; keep both.
                Some(_) => continue,
                None => {
                    let copied = store.set(IDENTIFIER, &target, &value).is_ok()
                        && store.get(IDENTIFIER, &target) == Ok(Some(value.clone()));
                    if !copied {
                        settled = false;
                        continue;
                    }
                    current = Some(value);
                }
            }
            if store.delete(service, source).is_err() {
                settled = false;
            }
        }
    }
    settled
}

pub fn secrets_pending(root: &Path) -> bool {
    !root.join(SECRETS_DONE).exists()
}

pub fn secrets_settled(root: &Path) {
    let path = root.join(SECRETS_DONE);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, b"");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::BTreeMap};

    #[test]
    fn identifier_matches_tauri_config() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], IDENTIFIER);
    }

    /// A data root as a Metrix Writer 0.39 install leaves it.
    fn legacy_fixture(base: &Path) {
        let root = base.join(LEGACY_DATA_FOLDER);
        for (path, bytes) in [
            ("Data/desktop.lock", &b""[..]),
            ("Data/native-recordings/r1/audio.wav", b"RIFF saved audio"),
            ("Data/transcripts.json", b"[\"transcript\"]"),
            ("Data/vocabulary.json", b"{\"terms\":[\"TrickTic\"]}"),
            ("State/settings.json", b"{\"ui_language\":\"he\"}"),
            ("State/credentials/openai.cred", b"MWCRED1\nprotected"),
            ("State/account/google-session.bin", b"protected session"),
            ("TauriWebView2/EBWebView/Local State", b"{}"),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
    }

    #[test]
    fn moves_a_metrix_writer_root_whole_and_only_once() {
        let base = tempfile::tempdir().unwrap();
        assert_eq!(resolve(base.path()), base.path().join(DATA_FOLDER));
        assert_eq!(carry_over_root(base.path(), Duration::ZERO), Outcome::Fresh);
        legacy_fixture(base.path());
        assert_eq!(resolve(base.path()), base.path().join(LEGACY_DATA_FOLDER));
        assert_eq!(carry_over_root(base.path(), Duration::ZERO), Outcome::Moved);
        let root = base.path().join(DATA_FOLDER);
        assert!(!base.path().join(LEGACY_DATA_FOLDER).exists());
        assert_eq!(resolve(base.path()), root);
        assert_eq!(
            std::fs::read(root.join("Data/native-recordings/r1/audio.wav")).unwrap(),
            b"RIFF saved audio"
        );
        for path in [
            "Data/transcripts.json",
            "Data/vocabulary.json",
            "State/settings.json",
            "State/credentials/openai.cred",
            "State/account/google-session.bin",
            "TauriWebView2/EBWebView/Local State",
        ] {
            assert!(root.join(path).is_file(), "{path}");
        }
        assert_eq!(carry_over_root(base.path(), Duration::ZERO), Outcome::Current);
    }

    #[test]
    fn a_running_previous_app_keeps_its_data_where_it_is() {
        let base = tempfile::tempdir().unwrap();
        legacy_fixture(base.path());
        let lock = std::fs::File::open(
            base.path().join(LEGACY_DATA_FOLDER).join("Data/desktop.lock"),
        )
        .unwrap();
        fs2::FileExt::lock_exclusive(&lock).unwrap();
        assert_eq!(carry_over_root(base.path(), Duration::ZERO), Outcome::InUse);
        assert!(!base.path().join(DATA_FOLDER).exists());
        assert_eq!(resolve(base.path()), base.path().join(LEGACY_DATA_FOLDER));
        fs2::FileExt::unlock(&lock).unwrap();
        drop(lock);
        assert_eq!(carry_over_root(base.path(), Duration::ZERO), Outcome::Moved);
    }

    #[test]
    fn both_folders_are_left_untouched() {
        let base = tempfile::tempdir().unwrap();
        legacy_fixture(base.path());
        std::fs::create_dir_all(base.path().join(DATA_FOLDER).join("Data")).unwrap();
        assert_eq!(carry_over_root(base.path(), Duration::ZERO), Outcome::Both);
        assert_eq!(resolve(base.path()), base.path().join(DATA_FOLDER));
        assert!(base
            .path()
            .join(LEGACY_DATA_FOLDER)
            .join("Data/transcripts.json")
            .is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_rename_loses_nothing_and_is_retried_later() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        legacy_fixture(base.path());
        let readonly = std::fs::Permissions::from_mode(0o555);
        std::fs::set_permissions(base.path(), readonly).unwrap();
        let outcome = carry_over_root(base.path(), Duration::from_millis(300));
        std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        // Root may rename regardless of permissions; then it simply moved.
        if outcome == Outcome::Moved {
            return;
        }
        assert_eq!(outcome, Outcome::Failed);
        assert_eq!(resolve(base.path()), base.path().join(LEGACY_DATA_FOLDER));
        assert!(base
            .path()
            .join(LEGACY_DATA_FOLDER)
            .join("Data/native-recordings/r1/audio.wav")
            .is_file());
        assert_eq!(carry_over_root(base.path(), Duration::ZERO), Outcome::Moved);
    }

    #[test]
    fn per_identity_folders_move_only_into_an_empty_name() {
        let parent = tempfile::tempdir().unwrap();
        assert!(!carry_over_folder(parent.path()).unwrap());
        let legacy = parent.path().join(LEGACY_IDENTIFIER).join("WebsiteData/LocalStorage");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("tauri_localhost_0.localstorage"), b"theme").unwrap();
        assert!(carry_over_folder(parent.path()).unwrap());
        assert!(parent
            .path()
            .join(IDENTIFIER)
            .join("WebsiteData/LocalStorage/tauri_localhost_0.localstorage")
            .is_file());
        std::fs::create_dir_all(parent.path().join(LEGACY_IDENTIFIER)).unwrap();
        assert!(!carry_over_folder(parent.path()).unwrap());
        assert!(parent.path().join(LEGACY_IDENTIFIER).exists());
    }

    #[derive(Default)]
    struct Keychain {
        items: RefCell<BTreeMap<(String, String), String>>,
        unreadable: Vec<(String, String)>,
        corrupt_writes: bool,
        failing_deletes: bool,
    }
    impl Secrets for Keychain {
        fn get(&self, service: &str, account: &str) -> Result<Option<String>, ()> {
            let key = (service.to_owned(), account.to_owned());
            if self.unreadable.contains(&key) {
                return Err(());
            }
            Ok(self.items.borrow().get(&key).cloned())
        }
        fn set(&self, service: &str, account: &str, value: &str) -> Result<(), ()> {
            let value = if self.corrupt_writes { "" } else { value };
            self.items
                .borrow_mut()
                .insert((service.into(), account.into()), value.into());
            Ok(())
        }
        fn delete(&self, service: &str, account: &str) -> Result<(), ()> {
            if self.failing_deletes {
                return Err(());
            }
            self.items
                .borrow_mut()
                .remove(&(service.into(), account.into()));
            Ok(())
        }
    }
    impl Keychain {
        fn put(&self, service: &str, account: String, value: &str) {
            self.items
                .borrow_mut()
                .insert((service.into(), account), value.into());
        }
        fn item(&self, service: &str, account: String) -> Option<String> {
            self.items.borrow().get(&(service.into(), account)).cloned()
        }
    }

    fn roots() -> (PathBuf, PathBuf) {
        let base = PathBuf::from("/Users/someone/Library/Application Support");
        (base.join(DATA_FOLDER), base.join(LEGACY_DATA_FOLDER))
    }

    #[test]
    fn keychain_items_are_copied_verified_then_removed() {
        let (root, legacy) = roots();
        let keychain = Keychain::default();
        keychain.put(LEGACY_IDENTIFIER, account(&legacy, "openai"), "sk-openai");
        keychain.put(LEGACY_IDENTIFIER, account(&legacy, "google-session"), "{\"v\":1}");
        assert!(carry_over_secrets(&keychain, &root, &legacy));
        assert_eq!(
            keychain.item(IDENTIFIER, account(&root, "openai")).as_deref(),
            Some("sk-openai")
        );
        assert_eq!(
            keychain.item(IDENTIFIER, account(&root, "google-session")).as_deref(),
            Some("{\"v\":1}")
        );
        assert_eq!(keychain.items.borrow().len(), 2);
        // A second run finds nothing left to do.
        assert!(carry_over_secrets(&keychain, &root, &legacy));
        assert_eq!(keychain.items.borrow().len(), 2);
    }

    #[test]
    fn a_refused_or_unverified_copy_keeps_the_original() {
        let (root, legacy) = roots();
        let refused = Keychain {
            unreadable: vec![(LEGACY_IDENTIFIER.into(), account(&legacy, "gemini"))],
            ..Default::default()
        };
        refused.put(LEGACY_IDENTIFIER, account(&legacy, "gemini"), "gemini-key");
        refused.put(LEGACY_IDENTIFIER, account(&legacy, "grok"), "grok-key");
        assert!(!carry_over_secrets(&refused, &root, &legacy));
        assert_eq!(
            refused.item(LEGACY_IDENTIFIER, account(&legacy, "gemini")).as_deref(),
            Some("gemini-key")
        );
        assert_eq!(refused.item(IDENTIFIER, account(&root, "gemini")), None);
        assert_eq!(
            refused.item(IDENTIFIER, account(&root, "grok")).as_deref(),
            Some("grok-key")
        );

        let corrupt = Keychain {
            corrupt_writes: true,
            ..Default::default()
        };
        corrupt.put(LEGACY_IDENTIFIER, account(&legacy, "openrouter"), "or-key");
        assert!(!carry_over_secrets(&corrupt, &root, &legacy));
        assert_eq!(
            corrupt.item(LEGACY_IDENTIFIER, account(&legacy, "openrouter")).as_deref(),
            Some("or-key")
        );

        let undeletable = Keychain {
            failing_deletes: true,
            ..Default::default()
        };
        undeletable.put(LEGACY_IDENTIFIER, account(&legacy, "openai"), "sk-openai");
        assert!(!carry_over_secrets(&undeletable, &root, &legacy));
        assert_eq!(
            undeletable.item(IDENTIFIER, account(&root, "openai")).as_deref(),
            Some("sk-openai")
        );
    }

    #[test]
    fn a_newer_key_for_the_new_identity_is_never_overwritten() {
        let (root, legacy) = roots();
        let keychain = Keychain::default();
        keychain.put(IDENTIFIER, account(&root, "openai"), "sk-new");
        keychain.put(LEGACY_IDENTIFIER, account(&legacy, "openai"), "sk-old");
        keychain.put(LEGACY_IDENTIFIER, account(&root, "openai"), "sk-new");
        assert!(carry_over_secrets(&keychain, &root, &legacy));
        assert_eq!(
            keychain.item(IDENTIFIER, account(&root, "openai")).as_deref(),
            Some("sk-new")
        );
        // The differing old key stays; the identical duplicate is removed.
        assert_eq!(
            keychain.item(LEGACY_IDENTIFIER, account(&legacy, "openai")).as_deref(),
            Some("sk-old")
        );
        assert_eq!(keychain.item(LEGACY_IDENTIFIER, account(&root, "openai")), None);
    }

    #[test]
    fn items_saved_while_the_old_folder_was_still_in_use_follow_it() {
        // A launch that could not move the folder ran on the old root under the
        // new service; those items are carried to the new root's account.
        let (root, legacy) = roots();
        let keychain = Keychain::default();
        keychain.put(IDENTIFIER, account(&legacy, "grok"), "grok-key");
        assert!(carry_over_secrets(&keychain, &root, &legacy));
        assert_eq!(
            keychain.item(IDENTIFIER, account(&root, "grok")).as_deref(),
            Some("grok-key")
        );
        assert_eq!(keychain.item(IDENTIFIER, account(&legacy, "grok")), None);
    }

    #[test]
    fn the_keychain_marker_lives_in_the_data_root() {
        let root = tempfile::tempdir().unwrap();
        assert!(secrets_pending(root.path()));
        secrets_settled(root.path());
        assert!(!secrets_pending(root.path()));
    }
}
