//! Stable filesystem identity, without reading audio or retaining open handles.
//! An unavailable identity falls back to counting every name conservatively.
use std::{fs::Metadata, path::Path};

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) struct Identity(u64, u128);

#[cfg(unix)]
pub(super) fn get(_path: &Path, metadata: &Metadata) -> Option<Identity> {
    use std::os::unix::fs::MetadataExt;
    (metadata.nlink() > 1).then(|| Identity(metadata.dev(), metadata.ino() as u128))
}

#[cfg(windows)]
pub(super) fn get(path: &Path, _metadata: &Metadata) -> Option<Identity> {
    use std::{
        ffi::c_void,
        fs::OpenOptions,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    };
    #[repr(C)]
    struct FileIdInfo {
        volume: u64,
        id: [u8; 16],
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandleEx(
            handle: *mut c_void,
            class: i32,
            info: *mut c_void,
            size: u32,
        ) -> i32;
    }
    // Metadata-only access, with read/write/delete sharing. Do not follow a
    // final reparse point or hold a journal against capture/maintenance.
    let file = OpenOptions::new()
        .access_mode(0)
        .share_mode(1 | 2 | 4)
        .custom_flags(0x00200000) // FILE_FLAG_OPEN_REPARSE_POINT
        .open(path)
        .ok()?;
    let mut info = FileIdInfo {
        volume: 0,
        id: [0; 16],
    };
    // FileIdInfo (18) gives the full 128-bit ID, also valid on ReFS.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            18,
            (&mut info as *mut FileIdInfo).cast(),
            std::mem::size_of::<FileIdInfo>() as u32,
        )
    };
    (ok != 0 && info.id != [0; 16]).then(|| Identity(info.volume, u128::from_le_bytes(info.id)))
}

#[cfg(not(any(unix, windows)))]
pub(super) fn get(_path: &Path, _metadata: &Metadata) -> Option<Identity> {
    None
}
