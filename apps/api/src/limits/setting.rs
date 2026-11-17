//! The desktop user's storage limit: one small JSON file under State, written
//! atomically. WRITER_DEVICE_STORAGE_BYTES, when set, wins over it.
use crate::{
    recordings::disk::{error, private_dir, read_json, write_json},
    transcription::{ApiError, ErrorCode},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(crate) const GIB: u64 = 1024 * 1024 * 1024;
/// The largest choice accepted, far above any drive this runs on.
pub(crate) const MAX_GIB: u64 = 100_000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    device_storage_bytes: u64,
}

/// Where a desktop profile keeps its choice, and whether the environment fixes it.
#[derive(Clone, Debug)]
pub(crate) struct LimitSetting {
    pub path: PathBuf,
    pub fixed_by_environment: bool,
}
impl LimitSetting {
    pub fn new(state_dir: &Path, fixed_by_environment: bool) -> Self {
        Self {
            path: state_dir.join("storage-limit.json"),
            fixed_by_environment,
        }
    }
    /// The saved choice. Missing, unreadable or out-of-range files read as none.
    pub async fn load(&self) -> Option<u64> {
        let stored: Stored = read_json(&self.path).await.ok()?;
        valid(stored.device_storage_bytes).then_some(stored.device_storage_bytes)
    }
    pub async fn save(&self, bytes: u64) -> Result<(), ApiError> {
        if self.fixed_by_environment {
            return Err(error(
                ErrorCode::InvalidRequest,
                "The storage limit is set by WRITER_DEVICE_STORAGE_BYTES on this computer.",
                false,
            ));
        }
        if !valid(bytes) {
            return Err(error(
                ErrorCode::InvalidRequest,
                "Choose a storage limit of at least 1 GB, in whole GB.",
                false,
            ));
        }
        private_dir(self.path.parent().unwrap()).await?;
        write_json(
            &self.path,
            &Stored {
                device_storage_bytes: bytes,
            },
        )
        .await
    }
}
fn valid(bytes: u64) -> bool {
    bytes.is_multiple_of(GIB) && (1..=MAX_GIB).contains(&(bytes / GIB))
}

/// Bytes free to this user on the drive holding `path`, when the system says.
pub(crate) fn free_bytes(path: &Path) -> Option<u64> {
    imp::free_bytes(path)
}
#[cfg(unix)]
mod imp {
    use std::{ffi::CString, os::unix::ffi::OsStrExt, path::Path};
    pub fn free_bytes(path: &Path) -> Option<u64> {
        let path = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: a valid NUL-terminated path and a writable statvfs buffer.
        if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
            return None;
        }
        // SAFETY: statvfs succeeded and filled the buffer.
        let stat = unsafe { stat.assume_init() };
        #[allow(clippy::unnecessary_cast)] // The field widths differ by platform.
        Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
    }
}
#[cfg(windows)]
mod imp {
    use std::{os::windows::ffi::OsStrExt, path::Path};
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            directory: *const u16,
            available: *mut u64,
            total: *mut u64,
            free: *mut u64,
        ) -> i32;
    }
    pub fn free_bytes(path: &Path) -> Option<u64> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut available = 0_u64;
        // SAFETY: a NUL-terminated wide path; optional outputs may be null.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(available)
    }
}
#[cfg(not(any(unix, windows)))]
mod imp {
    pub fn free_bytes(_: &std::path::Path) -> Option<u64> {
        None
    }
}

#[cfg(test)]
#[path = "setting_tests.rs"]
mod tests;
