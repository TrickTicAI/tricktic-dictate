//! An in-memory cloud that decides through the same planner as Firestore. Used
//! by the tests to run the shared vectors and multi-device scenarios.
use super::plan::{plan_with_links, Links, Plan, Receipt, Stored};
use super::sync::{Remote, RemoteError};
use super::wire::{Account, Consent, Decision, Operation, Record};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Default)]
pub struct Cloud {
    pub accounts: BTreeMap<String, Account>,
    pub records: BTreeMap<(String, Uuid), Stored>,
    pub receipts: BTreeMap<(String, Uuid), Receipt>,
    /// Every request fails as if the network were down.
    pub offline: bool,
    /// The next accepted commit is written but its reply is lost.
    pub lose_next_reply: bool,
    pub commits: usize,
    pub links: BTreeMap<String, Links>,
}

/// One authenticated user's view of the shared cloud.
#[derive(Clone)]
pub struct MemoryRemote {
    pub auth_uid: String,
    pub cloud: Arc<Mutex<Cloud>>,
}

impl MemoryRemote {
    fn online(&self) -> Result<std::sync::MutexGuard<'_, Cloud>, RemoteError> {
        let cloud = self.cloud.lock().unwrap();
        if cloud.offline {
            Err(RemoteError::Offline)
        } else {
            Ok(cloud)
        }
    }
}

#[async_trait::async_trait]
impl Remote for MemoryRemote {
    async fn account(&self) -> Result<Option<Account>, RemoteError> {
        Ok(self.online()?.accounts.get(&self.auth_uid).cloned())
    }
    async fn create_account(&self) -> Result<Account, RemoteError> {
        let mut cloud = self.online()?;
        Ok(cloud
            .accounts
            .entry(self.auth_uid.clone())
            .or_insert_with(|| Account {
                transcript_reset: None,
                epoch: 1,
                active: true,
                consent: Consent::initial(&self.auth_uid),
            })
            .clone())
    }
    async fn set_consent(&self, next: &Consent) -> Result<Option<Account>, RemoteError> {
        let mut cloud = self.online()?;
        let Some(account) = cloud.accounts.get_mut(&self.auth_uid) else {
            return Err(RemoteError::PermissionDenied);
        };
        if account.consent.revision + 1 != next.revision {
            return Ok(None);
        }
        account.consent = next.clone();
        Ok(Some(account.clone()))
    }
    async fn records(&self) -> Result<Vec<Record>, RemoteError> {
        let cloud = self.online()?;
        Ok(cloud
            .records
            .iter()
            .filter(|((uid, _), _)| *uid == self.auth_uid)
            .map(|(_, stored)| stored.record.clone())
            .collect())
    }
    async fn history_page(
        &self,
        after: Option<Uuid>,
    ) -> Result<super::sync::HistoryPage, RemoteError> {
        let cloud = self.online()?;
        let records: Vec<_> = cloud
            .records
            .iter()
            .filter(|((uid, id), s)| {
                uid == &self.auth_uid
                    && after.is_none_or(|a| *id > a)
                    && s.record.category == super::wire::Category::Transcripts
            })
            .take(super::sync::HISTORY_PAGE)
            .map(|(_, s)| {
                let mut r = s.record.clone();
                if let Some(super::wire::Data::Transcript(
                    super::history_wire::Transcript::Derived {
                        original_record_id, ..
                    },
                )) = &r.content
                {
                    if cloud
                        .links
                        .get(&self.auth_uid)
                        .and_then(|l| l.get(&("families".into(), *original_record_id)))
                        .is_some_and(|f| f["deleted"] == true)
                    {
                        r.content = None;
                    }
                }
                r
            })
            .collect();
        let next = (records.len() == super::sync::HISTORY_PAGE).then(|| records.last().unwrap().id);
        Ok(super::sync::HistoryPage { records, next })
    }
    async fn commit(&self, op: &Operation) -> Result<Decision, RemoteError> {
        let mut cloud = self.online()?;
        let uid = op.binding.uid.clone();
        let mut links = cloud.links.get(&uid).cloned().unwrap_or_default();
        for ((owner, id), stored) in &cloud.records {
            if owner == &uid {
                links.insert(("records".into(), *id), stored.record.encode());
            }
        }
        let decided = plan_with_links(
            &self.auth_uid,
            op,
            cloud.accounts.get(&uid),
            cloud.receipts.get(&(uid.clone(), op.operation_id)),
            cloud.records.get(&(uid.clone(), op.record_id)),
            &links,
        );
        match decided {
            Plan::Reply(decision) => Ok(decision),
            Plan::Write {
                stored,
                receipt,
                decision,
                links,
            } => {
                cloud.links.entry(uid.clone()).or_default().extend(links);
                cloud.records.insert((uid.clone(), op.record_id), *stored);
                cloud.receipts.insert((uid, op.operation_id), receipt);
                cloud.commits += 1;
                if std::mem::take(&mut cloud.lose_next_reply) {
                    return Err(RemoteError::Offline);
                }
                Ok(decision)
            }
        }
    }
}
