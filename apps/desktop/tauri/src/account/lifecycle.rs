//! Account lifecycle hooks v1 (MW-OUATH-8), the desktop half of
//! `docs/testing/account-sync/account-lifecycle.md`.
//!
//! Sync adapters (MW-OUATH-6/9) register a [`Participant`] and keep everything
//! for an account in its [`Workspace`], never in the guest stores. The account
//! service suspends every participant before it publishes an invalidation, and
//! runs the I/O hooks only after the session is already invalidated, so a slow
//! or failing hook can never keep an old account's work alive.
use super::AccountKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A sync data category, as the portable contract names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Settings,
    Dictionary,
    Transcripts,
}
pub const CATEGORIES: [Category; 3] = [
    Category::Settings,
    Category::Dictionary,
    Category::Transcripts,
];

/// What a participant holds in an account's workspace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LocalCopy {
    pub items: u64,
    /// Changes made on this device that the cloud has not acknowledged.
    pub unsynced: u64,
}

/// The user's choice for an account's downloaded copies when it leaves this device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalCopyChoice {
    /// Nothing copied or removed; the workspace waits, detached, for this account.
    #[default]
    Leave,
    /// Copy portable content into the guest store, then leave the workspace detached.
    Keep,
    /// Remove this account's downloaded copies from this device.
    Remove,
}

/// A cloud deletion fence a participant must honour for one category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Fence {
    pub category: Category,
    /// The category generation the fence advanced to.
    pub generation: u64,
    /// `everywhere`: also remove downloaded copies of the category.
    pub everywhere: bool,
}

/// Implemented by each sync adapter. Every hook concerns exactly `key`.
pub trait Participant: Send + Sync {
    /// Stops listeners and outbox dispatch synchronously. No I/O; queued
    /// operations stay in the workspace.
    fn suspend(&self, key: &AccountKey);
    fn local_copy(&self, key: &AccountKey) -> Result<LocalCopy, ()>;
    /// Copies portable content to the guest store without account metadata,
    /// consent or queued operations.
    fn keep_local_copy(&self, key: &AccountKey) -> Result<(), ()>;
    /// Removes this account's downloaded copies and caches only: no cloud
    /// tombstones, and never an unsynced original.
    fn remove_local_copy(&self, key: &AccountKey) -> Result<(), ()>;
    fn apply_fence(&self, key: &AccountKey, fence: &Fence) -> Result<(), ()>;
}

/// The registered participants, in registration order.
#[derive(Clone, Default)]
pub struct Participants(Arc<Mutex<Vec<Arc<dyn Participant>>>>);
impl Participants {
    pub fn register(&self, participant: Arc<dyn Participant>) {
        self.0.lock().unwrap().push(participant);
    }
    fn all(&self) -> Vec<Arc<dyn Participant>> {
        self.0.lock().unwrap().clone()
    }
    /// Called with the account lock held, before an invalidation is published.
    pub fn suspend(&self, key: &AccountKey) {
        for participant in self.all() {
            participant.suspend(key);
        }
    }
    /// The sum over every participant; an error from any of them is reported as
    /// unknown rather than as nothing.
    pub fn local_copy(&self, key: &AccountKey) -> Result<LocalCopy, ()> {
        self.all().iter().try_fold(LocalCopy::default(), |sum, p| {
            let copy = p.local_copy(key)?;
            Ok(LocalCopy {
                items: sum.items + copy.items,
                unsynced: sum.unsynced + copy.unsynced,
            })
        })
    }
    /// Applies the choice with every participant; all of them run even if one fails.
    pub fn settle(&self, key: &AccountKey, choice: LocalCopyChoice) -> Result<(), ()> {
        let mut result = Ok(());
        for participant in self.all() {
            let outcome = match choice {
                LocalCopyChoice::Leave => Ok(()),
                LocalCopyChoice::Keep => participant.keep_local_copy(key),
                LocalCopyChoice::Remove => participant.remove_local_copy(key),
            };
            result = result.and(outcome);
        }
        result
    }
    pub fn apply_fence(&self, key: &AccountKey, fence: &Fence) -> Result<(), ()> {
        let mut result = Ok(());
        for participant in self.all() {
            result = result.and(participant.apply_fence(key, fence));
        }
        result
    }
}

/// Per-account storage under `<data root>/Accounts/<project>/<hash of UID>/`.
/// The guest stores never live here, and the UID never appears in a path.
#[derive(Clone, Debug)]
pub struct Workspace {
    root: PathBuf,
}
impl Workspace {
    pub fn new(data_root: &Path) -> Self {
        Self {
            root: data_root.join("Accounts"),
        }
    }
    pub fn dir(&self, key: &AccountKey) -> PathBuf {
        let digest = Sha256::digest(key.uid.as_bytes());
        let hash: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
        // The project ID was validated against Google's project ID syntax, but
        // a path component is checked again rather than trusted.
        let project: String = key
            .project_id
            .chars()
            .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
            .collect();
        self.root.join(project).join(hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(uid: &str) -> AccountKey {
        AccountKey {
            project_id: "metrix-writer".into(),
            uid: uid.into(),
        }
    }

    #[derive(Default)]
    struct Recorder {
        log: Mutex<Vec<String>>,
        fail: bool,
        items: u64,
    }
    impl Participant for Recorder {
        fn suspend(&self, key: &AccountKey) {
            self.log
                .lock()
                .unwrap()
                .push(format!("suspend {}", key.uid));
        }
        fn local_copy(&self, _: &AccountKey) -> Result<LocalCopy, ()> {
            if self.fail {
                return Err(());
            }
            Ok(LocalCopy {
                items: self.items,
                unsynced: 1,
            })
        }
        fn keep_local_copy(&self, key: &AccountKey) -> Result<(), ()> {
            self.log.lock().unwrap().push(format!("keep {}", key.uid));
            if self.fail {
                Err(())
            } else {
                Ok(())
            }
        }
        fn remove_local_copy(&self, key: &AccountKey) -> Result<(), ()> {
            self.log.lock().unwrap().push(format!("remove {}", key.uid));
            if self.fail {
                Err(())
            } else {
                Ok(())
            }
        }
        fn apply_fence(&self, key: &AccountKey, fence: &Fence) -> Result<(), ()> {
            self.log.lock().unwrap().push(format!(
                "fence {} {:?} {}",
                key.uid, fence.category, fence.everywhere
            ));
            Ok(())
        }
    }

    #[test]
    fn workspaces_are_per_account_and_hide_the_uid() {
        let workspace = Workspace::new(Path::new("/data"));
        let a = workspace.dir(&key("uid-a"));
        let b = workspace.dir(&key("uid-b"));
        assert_ne!(a, b);
        assert!(a.starts_with("/data/Accounts/metrix-writer"));
        assert!(!a.to_string_lossy().contains("uid-a"));
        assert_eq!(a.file_name().unwrap().len(), 32);
        // A path-shaped project never escapes the accounts directory.
        let odd = AccountKey {
            project_id: "../x".into(),
            uid: "u".into(),
        };
        assert!(workspace.dir(&odd).starts_with("/data/Accounts/x"));
    }

    #[test]
    fn every_participant_settles_even_after_one_fails() {
        let participants = Participants::default();
        let failing = Arc::new(Recorder {
            fail: true,
            ..Default::default()
        });
        let working = Arc::new(Recorder {
            items: 3,
            ..Default::default()
        });
        participants.register(failing.clone());
        participants.register(working.clone());
        assert_eq!(
            participants.settle(&key("a"), LocalCopyChoice::Remove),
            Err(())
        );
        assert_eq!(*working.log.lock().unwrap(), ["remove a"]);
        assert_eq!(
            participants.settle(&key("a"), LocalCopyChoice::Leave),
            Ok(())
        );
        assert_eq!(working.log.lock().unwrap().len(), 1);
        // An unknown count is not reported as an empty workspace.
        assert_eq!(participants.local_copy(&key("a")), Err(()));
    }

    #[test]
    fn local_copy_sums_participants() {
        let participants = Participants::default();
        participants.register(Arc::new(Recorder {
            items: 2,
            ..Default::default()
        }));
        participants.register(Arc::new(Recorder {
            items: 5,
            ..Default::default()
        }));
        assert_eq!(
            participants.local_copy(&key("a")),
            Ok(LocalCopy {
                items: 7,
                unsynced: 2
            })
        );
        assert_eq!(
            Participants::default().local_copy(&key("a")),
            Ok(LocalCopy::default())
        );
    }
}
