use super::*;
use std::{path::PathBuf, process::Command};

const SECRET_NAMES: &[&str] = &[
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GROK_API_KEY",
    "XAI_API_KEY",
    "OPENROUTER_API_KEY",
    "ANTHROPIC_API_KEY",
    "AWS_SECRET_ACCESS_KEY",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "TAURI_SIGNING_PRIVATE_KEY",
    "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
    "CSC_KEY_PASSWORD",
    "AZURE_CLIENT_SECRET",
    "WRITER_UNKNOWN_FUTURE_SECRET",
    "PYTHONPATH",
    "NODE_OPTIONS",
    "GGML_VULKAN_DEVICE",
    "VK_ICD_FILENAMES",
];

#[test]
fn disposable_parent_does_not_pass_secrets_to_helper() {
    let mut parent = Command::new(std::env::current_exe().unwrap());
    parent.args([
        "--exact",
        "helper_security::tests::parent_probe",
        "--ignored",
    ]);
    for name in SECRET_NAMES {
        parent.env(name, "fake-security-sentinel");
    }
    let output = parent.output().unwrap();
    assert!(output.status.success(), "Isolated environment probe failed");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fake-security-sentinel"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("fake-security-sentinel"));
}

#[test]
#[ignore = "invoked in a disposable parent process by the environment test"]
fn parent_probe() {
    for name in SECRET_NAMES {
        assert!(std::env::var_os(name).is_some());
    }
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut child = crate::local_stt::process::owned_command(&std::env::current_exe().unwrap());
        child.args([
            "--exact",
            "helper_security::tests::child_probe",
            "--ignored",
        ]);
        let output = child.output().await.unwrap();
        assert!(output.status.success(), "Helper security probe failed");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("fake-security-sentinel"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("fake-security-sentinel"));
    });
}

#[test]
#[ignore = "invoked with the production helper command policy"]
fn child_probe() {
    for name in SECRET_NAMES {
        assert!(std::env::var_os(name).is_none());
    }
    for (name, _) in std::env::vars_os() {
        let allowed = if cfg!(windows) {
            ["SYSTEMROOT", "WINDIR", "TEMP", "TMP"]
                .contains(&name.to_string_lossy().to_uppercase().as_str())
        } else {
            ["PATH", "LANG", "TMPDIR"].contains(&name.to_string_lossy().as_ref())
        };
        // CoreFoundation initializes this inside the process even when exec's
        // environment contains only PATH/LANG. It is not inherited by our policy.
        let generated_by_os = cfg!(target_os = "macos") && name == "__CF_USER_TEXT_ENCODING";
        assert!(
            allowed || generated_by_os,
            "Unexpected environment variable name in helper: {name:?}"
        );
    }
    #[cfg(unix)]
    unsafe {
        let mut limit = libc::rlimit {
            rlim_cur: 1,
            rlim_max: 1,
        };
        assert_eq!(libc::getrlimit(libc::RLIMIT_CORE, &mut limit), 0);
        assert_eq!((limit.rlim_cur, limit.rlim_max), (0, 0));
        assert_eq!(libc::umask(0o077), 0o077);
    }
    #[cfg(target_os = "linux")]
    unsafe {
        assert_eq!(libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0), 1);
    }
}

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        // macOS /var and /tmp are OS aliases; explicit roots use their real path.
        Self(
            std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("writer-security-{}", uuid::Uuid::new_v4())),
        )
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn private_unicode_root_and_failure_paths() {
    let root = Root::new();
    let private = root.0.join("פרטי 日本語");
    protect_private_dir(&private).unwrap();
    protect_private_dir(&private).unwrap();
    std::fs::write(private.join("saved-audio"), b"synthetic retained audio").unwrap();
    assert!(protect_private_dir(&private.join("saved-audio")).is_err());
    assert!(protect_private_dir(&private.join("saved-audio/child")).is_err());
    // PathBuf::join normalizes `..` for Windows verbatim paths; construct the
    // caller-supplied spelling directly to test rejection before any access.
    let mut traversal = private.as_os_str().to_os_string();
    traversal.push(if cfg!(windows) {
        "\\..\\redirect"
    } else {
        "/../redirect"
    });
    assert!(protect_private_dir(&PathBuf::from(traversal)).is_err());
    assert!(protect_private_dir(std::path::Path::new("relative")).is_err());
    assert_eq!(
        std::fs::read(private.join("saved-audio")).unwrap(),
        b"synthetic retained audio"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&private).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o755)).unwrap();
        protect_private_dir(&private).unwrap();
        assert_eq!(
            std::fs::metadata(&private).unwrap().permissions().mode() & 0o777,
            0o700
        );
        if unsafe { libc::geteuid() } != 0 {
            assert!(protect_private_dir(std::path::Path::new("/")).is_err());
            let denied = root.0.join("denied");
            std::fs::create_dir(&denied).unwrap();
            std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o000)).unwrap();
            let result = protect_private_dir(&denied.join("child"));
            std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(result.is_err());
        }
    }
}

#[test]
fn refuses_directory_redirection_without_touching_target() {
    let root = Root::new();
    protect_private_dir(&root.0).unwrap();
    let target = root.0.join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("saved"), b"unchanged").unwrap();
    let link = root.0.join("redirect");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    #[cfg(windows)]
    {
        // Junctions do not require developer-mode symlink privileges.
        let status = Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }
    assert!(protect_private_dir(&link).is_err());
    assert!(protect_private_dir(&link.join("nested")).is_err());
    assert!(!target.join("nested").exists());
    assert_eq!(std::fs::read(target.join("saved")).unwrap(), b"unchanged");
    #[cfg(windows)]
    std::fs::remove_dir(&link).unwrap();
}

#[test]
fn legacy_diagnostics_removed_without_starting_a_worker() {
    use crate::local_stt::managed::{ManagedRuntime, RuntimeLayout};
    let root = Root::new();
    let worker = root.0.join("State/worker");
    protect_private_dir(&worker).unwrap();
    let log = worker.join("startup.log");
    let layout = RuntimeLayout {
        runtime_dir: None,
        ffmpeg: None,
        models_dir: root.0.join("Models"),
        state_dir: root.0.join("State"),
        startup_timeout: std::time::Duration::from_secs(1),
    };
    std::fs::write(&log, b"synthetic old diagnostic").unwrap();
    ManagedRuntime::open(layout.clone()).unwrap();
    assert!(!log.exists());
    #[cfg(unix)]
    {
        let target = root.0.join("retained-source");
        std::fs::write(&target, b"synthetic retained source").unwrap();
        std::os::unix::fs::symlink(&target, &log).unwrap();
        ManagedRuntime::open(layout.clone()).unwrap();
        assert!(std::fs::symlink_metadata(&log).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"synthetic retained source");
    }
    std::fs::create_dir(&log).unwrap();
    assert!(ManagedRuntime::open(layout).is_err());
}

#[cfg(windows)]
#[tokio::test]
async fn windows_job_restricts_helper_ui() {
    use tokio::io::AsyncWriteExt;
    let mut command = crate::local_stt::process::owned_command(&std::env::current_exe().unwrap());
    command
        .args(["--exact", "helper_security::tests::job_probe", "--ignored"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    let _job = crate::local_stt::worker_job::WorkerJob::attach(child.id().unwrap()).unwrap();
    child.stdin.take().unwrap().write_all(b"x").await.unwrap();
    assert!(child.wait().await.unwrap().success());
}

#[cfg(windows)]
#[test]
#[ignore = "invoked only after the parent assigns the private job"]
fn job_probe() {
    use std::io::Read;
    std::io::stdin().read_exact(&mut [0]).unwrap();
    #[link(name = "kernel32")]
    extern "system" {
        fn QueryInformationJobObject(
            job: *mut std::ffi::c_void,
            class: u32,
            information: *mut u32,
            size: u32,
            returned: *mut u32,
        ) -> i32;
    }
    let mut flags = 0;
    let ok = unsafe {
        QueryInformationJobObject(std::ptr::null_mut(), 4, &mut flags, 4, std::ptr::null_mut())
    };
    assert_ne!(ok, 0);
    assert_eq!(flags, 0x2 | 0x4 | 0x8 | 0x40 | 0x80);
}
