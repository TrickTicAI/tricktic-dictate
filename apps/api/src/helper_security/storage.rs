//! Private storage excludes other ordinary users, not the current account or root.
use std::{
    io,
    path::{Component, Path, PathBuf},
};

pub fn protect_private_dir(root: &Path) -> io::Result<()> {
    if !root.is_absolute() || root.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(denied());
    }
    // Check before creating anything below a redirected component. This protects
    // against pre-existing links, not a concurrent malicious same-user rename.
    let mut path = PathBuf::new();
    for component in root.components() {
        path.push(component);
        if !matches!(component, Component::Normal(_)) {
            continue;
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let builder = std::fs::DirBuilder::new();
                #[cfg(unix)]
                let builder = {
                    use std::os::unix::fs::DirBuilderExt;
                    let mut builder = builder;
                    builder.mode(0o700);
                    builder
                };
                match builder.create(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                // A concurrent creator is subject to the same ownership and
                // permission checks as every pre-existing ancestor below.
                std::fs::symlink_metadata(&path)?
            }
            Err(error) => return Err(error),
        };
        if trusted_system_alias(&path, &metadata) {
            continue;
        }
        if is_redirect(&metadata) || !metadata.is_dir() {
            return Err(denied());
        }
        #[cfg(unix)]
        if path != root {
            use std::os::unix::fs::MetadataExt;
            // Other users must not be able to replace an ancestor. Root-owned
            // sticky temp directories are the OS exception. Root may appear as
            // the overflow UID in a non-root user namespace/test runner.
            let uid = unsafe { libc::geteuid() };
            let root_uid = std::fs::metadata("/")?.uid();
            let sticky_root = metadata.uid() == root_uid && metadata.mode() & 0o1000 != 0;
            if (metadata.uid() != root_uid && metadata.uid() != uid)
                || (metadata.mode() & 0o022 != 0 && !sticky_root)
            {
                return Err(denied());
            }
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(root)?;
        if directory.metadata()?.uid() != unsafe { libc::geteuid() } {
            return Err(denied());
        }
        directory.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    windows::protect(root)?;
    Ok(())
}

fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Private storage requires an owned directory without links or reparse points.",
    )
}
fn trusted_system_alias(path: &Path, metadata: &std::fs::Metadata) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::MetadataExt;
        // Apple's root-owned /var and /tmp aliases are used by std::env::temp_dir.
        // No user-controlled ancestor link is allowed by this exception.
        metadata.file_type().is_symlink()
            && metadata.uid() == 0
            && [("/var", "private/var"), ("/tmp", "private/tmp")]
                .iter()
                .any(|(alias, target)| {
                    path == Path::new(alias)
                        && std::fs::read_link(path).is_ok_and(|p| {
                            p == Path::new(target) || p == Path::new("/").join(target)
                        })
                })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, metadata);
        false
    }
}
fn is_redirect(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT, including junctions
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
#[cfg(windows)]
#[path = "storage_windows.rs"]
mod windows;
