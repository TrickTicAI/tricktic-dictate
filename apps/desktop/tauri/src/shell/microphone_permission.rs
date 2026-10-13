//! Gate real capture on macOS TCC authorization before creating a journal.
//! Silence is valid audio and is never used as a permission signal.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Authorization {
    NotDetermined,
    Restricted,
    Denied,
    Authorized,
    Unknown,
}

#[cfg(any(target_os = "macos", test))]
fn require_authorized(status: Authorization) -> Result<(), String> {
    match status {
        Authorization::Authorized => Ok(()),
        Authorization::Denied => Err(format!("Microphone access is denied. Enable {} in System Settings → Privacy & Security → Microphone, then quit and reopen the app and try recording again.", product_name!())),
        Authorization::Restricted => Err("Microphone access is restricted by macOS. Ask your device administrator to allow microphone access, then quit and reopen the app and try recording again.".into()),
        Authorization::NotDetermined => Err("Microphone access has not been granted. Allow the macOS microphone permission request, then try recording again.".into()),
        Authorization::Unknown => Err("Cannot verify macOS microphone permission. Quit and reopen Writer, check System Settings → Privacy & Security → Microphone, then try recording again.".into()),
    }
}

#[cfg(any(target_os = "macos", test))]
fn authorize_then<T>(
    status: Authorization,
    request: impl FnOnce() -> Result<Authorization, String>,
    capture: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let status = if status == Authorization::NotDetermined {
        request()?
    } else {
        status
    };
    require_authorized(status)?;
    capture()
}

pub(super) fn with_access<T>(capture: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    #[cfg(target_os = "macos")]
    {
        authorize_then(native::status(), native::request, capture)
    }
    #[cfg(not(target_os = "macos"))]
    capture()
}

pub(super) fn check_current() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        require_authorized(native::status())
    }
    #[cfg(not(target_os = "macos"))]
    Ok(())
}

#[cfg(target_os = "macos")]
mod native {
    use super::Authorization;
    use objc2::{class, msg_send, runtime::Bool};
    use objc2_foundation::NSString;

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeAudio: *const NSString;
    }

    pub(super) fn status() -> Authorization {
        // AVAuthorizationStatus is an NSInteger; query the running app's TCC
        // identity, not a saved preference or another process's permissions.
        let value: isize = unsafe {
            msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio]
        };
        match value {
            0 => Authorization::NotDetermined,
            1 => Authorization::Restricted,
            2 => Authorization::Denied,
            3 => Authorization::Authorized,
            _ => Authorization::Unknown,
        }
    }

    pub(super) fn request() -> Result<Authorization, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let completion = block2::RcBlock::new(move |granted: Bool| {
            let _ = tx.send(granted.as_bool());
        });
        // Runs on the recorder thread; the main thread remains free to present
        // Apple's prompt. AVFoundation copies the block for its async callback.
        unsafe {
            let _: () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: AVMediaTypeAudio, completionHandler: &*completion];
        }
        // An unanswered prompt must not hang capture/quit indefinitely. No
        // journal exists yet; a later response is rechecked on the next start.
        rx.recv_timeout(std::time::Duration::from_secs(60)).map_err(|_| {
            "Microphone permission request was not completed. Respond to the macOS prompt, then try recording again.".to_owned()
        })?;
        Ok(status())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn denial_restriction_and_unknown_never_open_capture() {
        for status in [
            Authorization::Denied,
            Authorization::Restricted,
            Authorization::Unknown,
        ] {
            let result = authorize_then(
                status,
                || panic!("must not reprompt"),
                || panic!("must not open the device or create a journal"),
            );
            let result: Result<(), String> = result;
            let error = result.unwrap_err();
            assert!(error.contains("Microphone") || error.contains("microphone"));
            assert!(error.contains("try recording again"));
        }
    }

    #[test]
    fn first_use_waits_for_authorization_before_capture() {
        for answer in [
            Authorization::Denied,
            Authorization::NotDetermined,
            Authorization::Authorized,
        ] {
            let opened = Cell::new(false);
            let result = authorize_then(
                Authorization::NotDetermined,
                || Ok(answer),
                || {
                    opened.set(true);
                    Ok(())
                },
            );
            assert_eq!(result.is_ok(), answer == Authorization::Authorized);
            assert_eq!(opened.get(), answer == Authorization::Authorized);
        }
        assert!(authorize_then(
            Authorization::NotDetermined,
            || Err("prompt timeout".into()),
            || -> Result<(), String> { panic!("unanswered prompt cannot start capture") }
        )
        .is_err());
    }

    #[test]
    fn denied_then_allowed_recovers_without_cached_failure_and_accepts_silence() {
        assert!(authorize_then(
            Authorization::Denied,
            || unreachable!(),
            || Ok(vec![0_i16; 320])
        )
        .is_err());
        let audio = authorize_then(
            Authorization::Authorized,
            || unreachable!(),
            || Ok(vec![0_i16; 320]),
        )
        .unwrap();
        assert_eq!(audio, vec![0; 320]);
    }

    #[test]
    fn authorized_downstream_failure_is_returned_independently() {
        let result: Result<(), String> = authorize_then(
            Authorization::Authorized,
            || unreachable!(),
            || Err("disk unavailable".into()),
        );
        assert_eq!(result.unwrap_err(), "disk unavailable");
    }

    #[test]
    fn denial_creates_no_journal_and_preserves_existing_audio_and_raw_text() {
        use crate::storage::Store;
        use sha2::{Digest, Sha256};
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path().to_owned()).unwrap();
        let saved = uuid::Uuid::new_v4();
        let mut journal = store.create(saved, "en".into(), 16000, 1).unwrap();
        journal.append(&[1200; 1600]).unwrap();
        journal.finish(false).unwrap();
        let paths = [
            store.path(saved).join("audio.wav"),
            store.path(saved).join("raw-transcript.txt"),
        ];
        std::fs::write(&paths[1], "Synthetic independent raw transcript").unwrap();
        let hashes = || {
            paths
                .each_ref()
                .map(|p| Sha256::digest(std::fs::read(p).unwrap()))
        };
        let before = hashes();
        let denied = uuid::Uuid::new_v4();
        let result = authorize_then(
            Authorization::Denied,
            || unreachable!(),
            || store.create(denied, "en".into(), 16000, 1),
        );
        assert!(result.is_err());
        assert!(!store.path(denied).exists());
        assert_eq!(hashes(), before);
    }
}
