//! Admission limits are independent of capture duration and never evict saved data.
pub(crate) mod http;
pub(crate) mod storage;
pub(crate) mod work;
use crate::transcription::{ApiError, ErrorCode};
use std::time::Duration;

#[derive(Clone, Debug)]
pub(crate) struct Limits {
    pub storage_bytes: u64,
    pub device_storage_bytes: u64,
    pub storage_entries: usize,
    pub uploads: usize,
    pub device_uploads: usize,
    pub idle: Duration,
    pub pending: usize,
    pub device_pending: usize,
    pub jobs: usize,
    pub work_concurrency: usize,
    pub device_work_concurrency: usize,
    pub work_timeout: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            storage_bytes: 20 * 1024 * 1024 * 1024,
            device_storage_bytes: 5 * 1024 * 1024 * 1024,
            storage_entries: 50_000,
            uploads: 4,
            device_uploads: 2,
            idle: Duration::from_secs(30),
            pending: 32,
            device_pending: 8,
            jobs: 10_000,
            work_concurrency: 4,
            device_work_concurrency: 2,
            work_timeout: Duration::from_secs(900),
        }
    }
}
impl Limits {
    pub fn from_env() -> Result<Self, ApiError> {
        let mut s = Self::default();
        macro_rules! set {
            ($field:ident, $name:literal) => {
                if let Some(value) = std::env::var_os($name) {
                    s.$field = value
                        .to_str()
                        .and_then(|v| v.parse().ok())
                        .filter(|v| *v > 0)
                        .ok_or_else(|| {
                            crate::recordings::disk::error(
                                ErrorCode::InvalidRequest,
                                concat!($name, " must be a positive integer."),
                                false,
                            )
                        })?;
                }
            };
        }
        set!(storage_bytes, "WRITER_STORAGE_BYTES");
        set!(device_storage_bytes, "WRITER_DEVICE_STORAGE_BYTES");
        set!(storage_entries, "WRITER_MAX_STORAGE_ENTRIES");
        set!(uploads, "WRITER_UPLOAD_CONCURRENCY");
        set!(device_uploads, "WRITER_DEVICE_UPLOAD_CONCURRENCY");
        set!(pending, "WRITER_MAX_PENDING_JOBS");
        set!(device_pending, "WRITER_DEVICE_MAX_PENDING_JOBS");
        set!(jobs, "WRITER_MAX_JOBS");
        set!(work_concurrency, "WRITER_WORK_CONCURRENCY");
        set!(device_work_concurrency, "WRITER_DEVICE_WORK_CONCURRENCY");
        for (name, target) in [
            ("WRITER_UPLOAD_IDLE_SECONDS", &mut s.idle),
            ("WRITER_WORK_TIMEOUT_SECONDS", &mut s.work_timeout),
        ] {
            if let Some(value) = std::env::var_os(name) {
                let seconds = value
                    .to_str()
                    .and_then(|v| v.parse::<u64>().ok())
                    .filter(|v| *v > 0 && *v <= 86400)
                    .ok_or_else(|| {
                        crate::recordings::disk::error(
                            ErrorCode::InvalidRequest,
                            "Idle/work timeouts must be integers between 1 and 86400 seconds.",
                            false,
                        )
                    })?;
                *target = Duration::from_secs(seconds);
            }
        }
        Ok(s)
    }
}
pub(crate) fn busy(message: &str) -> ApiError {
    crate::recordings::disk::error(ErrorCode::RateLimited, message, true)
}
pub(crate) fn idle() -> ApiError {
    crate::recordings::disk::error(
        ErrorCode::ProviderTimeout,
        "Upload stopped making progress; retry from retained client audio.",
        true,
    )
}

#[cfg(test)]
mod tests;
