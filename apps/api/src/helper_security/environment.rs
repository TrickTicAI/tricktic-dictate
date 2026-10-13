use tokio::process::Command;

/// Start from nothing: new provider, proxy, signing and loader variables are
/// excluded automatically. PATH is fixed, never copied from the launcher.
pub(crate) fn configure(command: &mut Command) {
    command.env_clear();
    #[cfg(windows)]
    {
        for name in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(unix)]
    {
        command
            .env("PATH", "/usr/bin:/bin")
            .env("LANG", "en_US.UTF-8");
        #[cfg(target_os = "macos")]
        if let Some(value) = std::env::var_os("TMPDIR") {
            command.env("TMPDIR", value);
        }
        // Only async-signal-safe libc calls after fork; no allocation or locks.
        unsafe {
            command.pre_exec(|| {
                libc::umask(0o077);
                let limit = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                if libc::setrlimit(libc::RLIMIT_CORE, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                #[cfg(target_os = "linux")]
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}
