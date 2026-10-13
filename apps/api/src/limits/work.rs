//! Temporary processing capacity; completed work never consumes a daily allowance.
use super::{busy, Limits};
use crate::transcription::ApiError;
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
pub(crate) struct WorkCapacity {
    limits: Limits,
    slots: Arc<Semaphore>,
    pending: Arc<std::sync::atomic::AtomicUsize>,
}
#[derive(Clone)]
pub(crate) struct WorkAccount {
    capacity: Arc<WorkCapacity>,
    slots: Arc<Semaphore>,
    recognition: Arc<Semaphore>,
    final_text_waiters: Arc<std::sync::atomic::AtomicUsize>,
}
pub(crate) struct WorkPermit {
    _global: OwnedSemaphorePermit,
    _owner: OwnedSemaphorePermit,
}
impl WorkCapacity {
    pub fn new(limits: Limits) -> Arc<Self> {
        Arc::new(Self {
            slots: Arc::new(Semaphore::new(limits.work_concurrency)),
            limits,
            pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })
    }
    pub fn account(self: &Arc<Self>) -> WorkAccount {
        WorkAccount {
            capacity: self.clone(),
            slots: Arc::new(Semaphore::new(self.limits.device_work_concurrency)),
            // Recognition has its own lane: speculative primary/backup polishing
            // must not occupy every slot needed to accept saved audio.
            recognition: Arc::new(Semaphore::new(1)),
            final_text_waiters: Default::default(),
        }
    }
}
impl WorkAccount {
    /// Final text gets a short, cancelable chance to acquire capacity. Drafts
    /// never queue ahead of it. Recognition retains its existing reservation.
    pub async fn admit_text(&self) -> Result<WorkPermit, ApiError> {
        use std::sync::atomic::Ordering::SeqCst;
        if BACKGROUND_TEXT.try_with(|_| ()).is_ok() {
            if self.final_text_waiters.load(SeqCst) != 0 {
                return Err(busy(
                    "Background polishing yielded to final text processing.",
                ));
            }
            return self.admit();
        }
        self.final_text_waiters.fetch_add(1, SeqCst);
        let _waiting = FinalTextWaiter(self.final_text_waiters.clone());
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(250);
        loop {
            match self.admit() {
                Ok(permit) => return Ok(permit),
                Err(error) if tokio::time::Instant::now() >= deadline => return Err(error),
                Err(_) => {
                    tokio::time::sleep_until(
                        deadline.min(
                            tokio::time::Instant::now() + std::time::Duration::from_millis(10),
                        ),
                    )
                    .await
                }
            }
        }
    }
    pub fn queue(&self, recovered: bool) -> Result<QueueTicket, ApiError> {
        use std::sync::atomic::Ordering::SeqCst;
        let pending = &self.capacity.pending;
        if recovered {
            pending.fetch_add(1, SeqCst);
        } else {
            pending
                .fetch_update(SeqCst, SeqCst, |n| {
                    (n < self.capacity.limits.pending).then_some(n + 1)
                })
                .map_err(|_| busy("Server queue is full; retry from saved audio."))?;
        }
        Ok(QueueTicket(pending.clone()))
    }
    pub fn timeout(&self) -> std::time::Duration {
        self.capacity.limits.work_timeout
    }
    pub fn admit(&self) -> Result<WorkPermit, ApiError> {
        // Give durable queued recognition the last global slot ahead of new text work.
        if self
            .capacity
            .pending
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0
            && self.capacity.slots.available_permits() <= 1
        {
            return Err(busy(
                "Text processing is waiting for queued transcription capacity.",
            ));
        }
        let owner = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| busy("Device processing is busy; retry from saved audio/text."))?;
        let global = self
            .capacity
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| busy("Server processing is busy; retry from saved audio/text."))?;
        Ok(WorkPermit {
            _global: global,
            _owner: owner,
        })
    }
    /// None means temporary contention, not a failed provider attempt.
    pub fn try_admit_transcription(&self) -> Option<WorkPermit> {
        let owner = self.recognition.clone().try_acquire_owned().ok()?;
        let global = self.capacity.slots.clone().try_acquire_owned().ok()?;
        Some(WorkPermit {
            _global: global,
            _owner: owner,
        })
    }
}

tokio::task_local! { static BACKGROUND_TEXT: (); }
// Super's recording-time drafts were its caller; drafts now start only at
// release, at final priority. Kept for any later background polishing.
#[allow(dead_code)]
pub(crate) async fn background_text<T>(future: impl std::future::Future<Output = T>) -> T {
    BACKGROUND_TEXT.scope((), future).await
}
struct FinalTextWaiter(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for FinalTextWaiter {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

pub(crate) struct QueueTicket(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for QueueTicket {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
mod text_tests {
    use super::*;
    use std::sync::atomic::Ordering::SeqCst;
    use std::time::Duration;
    fn account() -> WorkAccount {
        WorkCapacity::new(Limits {
            device_work_concurrency: 1,
            work_concurrency: 1,
            ..Limits::default()
        })
        .account()
    }
    #[tokio::test]
    async fn final_waits_for_capacity_and_new_drafts_yield() {
        let work = account();
        let held = work.admit().unwrap();
        let waiting = work.admit_text();
        tokio::pin!(waiting);
        tokio::select! {
            _ = &mut waiting => panic!("occupied capacity must wait"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
        drop(held);
        assert!(
            background_text(work.admit_text()).await.is_err(),
            "new draft must yield even after the slot opens"
        );
        let final_permit = waiting.await.unwrap();
        assert!(
            work.admit().is_err(),
            "the existing concurrency bound still applies"
        );
        drop(final_permit);
        assert!(background_text(work.admit_text()).await.is_ok());
    }
    #[tokio::test]
    async fn final_capacity_wait_is_bounded_and_cancellation_releases_priority() {
        let work = account();
        let held = work.admit().unwrap();
        let error = tokio::time::timeout(Duration::from_secs(1), work.admit_text())
            .await
            .unwrap()
            .err()
            .unwrap();
        assert!(matches!(
            error.code,
            crate::transcription::ErrorCode::RateLimited
        ));
        assert_eq!(work.final_text_waiters.load(SeqCst), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), work.admit_text())
                .await
                .is_err()
        );
        assert_eq!(work.final_text_waiters.load(SeqCst), 0);
        drop(held);
        assert!(background_text(work.admit_text()).await.is_ok());
    }
    #[tokio::test]
    async fn final_text_keeps_the_last_slot_available_for_queued_recognition() {
        let work = account();
        let queued = work.queue(false).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), work.admit_text())
                .await
                .is_err()
        );
        let recognition = work.try_admit_transcription().unwrap();
        drop(queued);
        drop(recognition);
        assert!(work.admit_text().await.is_ok());
    }
}
