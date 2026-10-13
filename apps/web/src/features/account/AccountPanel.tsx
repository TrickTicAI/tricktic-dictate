import { isMac } from '../settings/platform';
import { Group, Row } from '../settings/controls';
import type { AccountView } from './account';

/**
 * Settings → Account when this build offers Google sign-in. Every state keeps
 * dictation fully usable; signing in stores an identity on this computer and
 * nothing more. Sync is a separate, later choice.
 */
export function GoogleAccount({ view }: { view: AccountView }) {
  const { snapshot, notice, busy } = view;
  if (!snapshot) return null;
  const device = isMac() ? 'this Mac' : 'this PC';
  const who = snapshot.display?.name || snapshot.display?.email || 'Google account';
  const email = snapshot.display?.name && snapshot.display?.email ? snapshot.display.email : null;
  const note = notice?.text;
  const tone = notice?.tone === 'alert' ? 'alert' as const : undefined;

  if (snapshot.availability === 'not_configured') {
    return (
      <Group title="Google account">
        <Row title="Sign in with Google" detail="This build has no Google sign-in configuration. Everything else works without an account.">
          <button type="button" className="button" disabled>Sign in with Google</button>
        </Row>
      </Group>
    );
  }

  switch (snapshot.state) {
    case 'guest':
      return (
        <Group title="Google account" note={note} noteTone={tone}>
          <Row
            title="Sign in with Google (optional)"
            detail={`Opens Google in your browser. Signing in uploads nothing: your settings, dictionary, recordings and transcripts stay on ${device}. Sync is a separate choice you make after signing in.`}
          >
            <button type="button" className="button primary" disabled={busy} onClick={view.signIn}>Sign in with Google</button>
          </Row>
        </Group>
      );
    case 'signing_in':
      return (
        <Group title="Google account" note={note} noteTone={tone}>
          <Row
            title={snapshot.reauthenticating ? `Signing in again as ${who}` : 'Waiting for Google'}
            detail="Finish in the browser tab that opened, then come back here. Dictation keeps working meanwhile. The attempt expires after three minutes."
          >
            <button type="button" className="button" disabled={busy || !snapshot.attemptId} onClick={view.cancel}>Cancel sign-in</button>
          </Row>
        </Group>
      );
    case 'signed_in':
      return (
        <Group title="Google account" note={note} noteTone={tone}>
          <Row
            title={who}
            detail={<>{email && <>{email}<br /></>}Signed in with Google. Signing in uploaded nothing; syncing settings and dictionary is a separate choice below.</>}
          >
            <span className="badge success"><span className="dot" aria-hidden />Signed in</span>
          </Row>
        </Group>
      );
    case 'reauth_required':
      return (
        <Group title="Google account" note={note} noteTone={tone}>
          <Row
            title={who}
            detail={<>{email && <>{email}<br /></>}This sign-in expired or was revoked. Dictation is unaffected. Sign in again with the same Google account, or sign out below.</>}
          >
            <span className="badge warn"><span className="dot" aria-hidden />Sign-in needed</span>
            <button type="button" className="button primary" disabled={busy} onClick={view.reauthenticate}>Sign in again</button>
          </Row>
        </Group>
      );
  }
}
