//! Immutable targets for delayed OS actions. Never rebind an old action to the
//! current recording after its validation has completed.
#[derive(Debug, PartialEq)]
pub struct CommandTarget {
    pub epoch: String,
    pub session: String,
}
pub fn command_target(
    expected: Option<(&str, &str)>,
    current_epoch: Option<&str>,
    current_session: Option<&str>,
) -> Option<CommandTarget> {
    let (epoch, session) = (current_epoch?, current_session?);
    let (epoch, session) = match expected {
        Some((expected_epoch, expected_session))
            if expected_epoch == epoch && expected_session == session =>
        {
            (expected_epoch, expected_session)
        }
        Some(_) => return None,
        None => (epoch, session),
    };
    Some(CommandTarget {
        epoch: epoch.to_owned(),
        session: session.to_owned(),
    })
}
#[cfg(any(windows, target_os = "macos", test))]
pub fn clipboard_unchanged<T: Default + PartialEq>(written: T, current: T) -> bool {
    written != T::default() && written == current
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delayed_release_is_rejected_or_keeps_its_original_command_target() {
        let expected = Some(("owner-1", "recording-1"));
        assert!(command_target(expected, Some("owner-1"), Some("recording-2")).is_none());
        assert!(command_target(expected, Some("owner-2"), Some("recording-1")).is_none());
        assert!(command_target(expected, Some("owner-1"), None).is_none());
        // A new start after validation cannot mutate the already selected
        // command. Its old epoch/session will be rejected by the session owner.
        let mut pending = String::from("recording-1");
        let command = command_target(expected, Some("owner-1"), Some(&pending)).unwrap();
        pending = String::from("recording-2");
        assert_eq!(
            command,
            CommandTarget {
                epoch: "owner-1".into(),
                session: "recording-1".into()
            }
        );
        assert_ne!(command.session, pending);
    }
    #[cfg(any(windows, target_os = "macos", test))]
    #[test]
    fn external_copy_during_modifier_wait_invalidates_pending_paste() {
        assert!(clipboard_unchanged(42, 42));
        assert!(!clipboard_unchanged(42, 43));
        assert!(!clipboard_unchanged(0, 0));
    }
}
