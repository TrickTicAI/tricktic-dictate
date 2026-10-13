import { useState } from 'react';
import type { AccountDataCategory, AccountLocalCopy, AccountLocalCopyChoice } from '@metrix-writer/contracts/desktop';
import { Dialog } from '../shell/Dialog';
import { Group, Row } from '../settings/controls';
import { isMac } from '../settings/platform';
import { accountBridge, taskMessage, type AccountTasksView, type AccountView } from './account';

/*
 * Leaving and deleting a Writer account (MW-OUATH-8). Every action here names
 * exactly what it changes: the cloud, this computer, or neither. Sensitive ones
 * ask Google to confirm it is the same account first; the shell does that and
 * sends the request, and this page only shows fixed progress words.
 */

const CATEGORIES: { id: AccountDataCategory; label: string }[] = [
  { id: 'settings', label: 'Settings' },
  { id: 'dictionary', label: 'Dictionary' },
  { id: 'transcripts', label: 'Transcript history' },
];

const device = () => (isMac() ? 'this Mac' : 'this PC');

/** Asks what happens to this account's downloaded copies, when it has any. */
function LocalCopyChoice({ copy, value, onChange }: {
  copy: AccountLocalCopy;
  value: AccountLocalCopyChoice;
  onChange: (value: AccountLocalCopyChoice) => void;
}) {
  const options: { id: AccountLocalCopyChoice; label: string; detail: string }[] = [
    { id: 'leave', label: 'Leave them for this account', detail: `Kept on ${device()} but unused until this account signs in again.` },
    { id: 'keep', label: 'Keep a copy without the account', detail: `Copies them into ${device()}'s own settings and dictionary.` },
    { id: 'remove', label: `Remove them from ${device()}`, detail: 'Removes only downloaded copies. Nothing in the cloud changes.' },
  ];
  return (
    <fieldset className="field">
      <legend>{copy.items > 0
        ? `This account has ${copy.items} synced ${copy.items === 1 ? 'item' : 'items'} on ${device()}`
        : `This account's synced copies on ${device()}`}</legend>
      {options.map((option) => (
        <label key={option.id} className="choice">
          <input type="radio" name="local-copy" checked={value === option.id} onChange={() => onChange(option.id)} />
          <span><strong>{option.label}</strong> — {option.detail}</span>
        </label>
      ))}
      {copy.unsynced > 0 && (
        <p className="dialog-note">{copy.unsynced} {copy.unsynced === 1 ? 'change has' : 'changes have'} not reached the cloud yet and stay on {device()} whatever you choose.</p>
      )}
    </fieldset>
  );
}

type Asking =
  | { kind: 'sign_out' | 'switch' | 'revoke' | 'delete_account'; copy: AccountLocalCopy }
  | { kind: 'delete_cloud' };

/** Loads the local-copy count, then either acts at once or asks. */
function useAsk(ask: (asking: Asking) => void) {
  return (kind: Exclude<Asking['kind'], 'delete_cloud'>, act: () => void, always = false) => {
    accountBridge.localCopy()
      .then((copy) => (copy.items > 0 || always ? ask({ kind, copy }) : act()))
      // An unknown count is not an empty one: ask, and say nothing about items.
      .catch(() => ask({ kind, copy: { items: 0, unsynced: 0 } }));
  };
}

export function AccountData({ view, tasks }: { view: AccountView; tasks: AccountTasksView }) {
  const [asking, setAsking] = useState<Asking | null>(null);
  const [choice, setChoice] = useState<AccountLocalCopyChoice>('leave');
  const [categories, setCategories] = useState<AccountDataCategory[]>([]);
  const [everywhere, setEverywhere] = useState(false);
  const ask = useAsk((next) => { setChoice('leave'); setAsking(next); });
  const snapshot = view.snapshot;
  if (!snapshot || (snapshot.state !== 'signed_in' && snapshot.state !== 'reauth_required')) return null;

  const busy = view.busy || tasks.busy;
  const unconfigured = !tasks.configured;
  const status = tasks.refused
    ? { tone: 'alert' as const, text: tasks.refused }
    : tasks.latest ? taskMessage(tasks.latest) : null;
  const close = () => setAsking(null);
  const copyChoice = asking && asking.kind !== 'delete_cloud' && asking.copy.items > 0 ? choice : undefined;

  return (
    <>
      <Group title="Switch or leave">
        <Row
          title="Switch account"
          detail={`Signs this account out of ${device()}, then opens Google to choose another. Nothing from this account is used while the other one is signed in.`}
        >
          <button type="button" className="button" disabled={busy} onClick={() => ask('switch', () => view.switchAccount())}>Switch account</button>
        </Row>
        <Row
          title="Sign out"
          detail={`Removes the saved sign-in from ${device()}. Recordings, transcripts, settings, dictionary and provider keys stay.`}
        >
          <button type="button" className="button" disabled={busy} onClick={() => ask('sign_out', () => view.signOut())}>Sign out</button>
        </Row>
      </Group>

      <Group
        title="Sessions and cloud data"
        note={unconfigured ? 'This build has no account service configured, so these actions are unavailable.' : status?.text}
        noteTone={!unconfigured && status?.tone === 'alert' ? 'alert' : undefined}
      >
        <Row
          title="Sign out on all devices"
          detail="Ends this Google account's Writer sign-in on every device, including this one. Google asks you to confirm it is you first."
        >
          <button type="button" className="button" disabled={busy || unconfigured} onClick={() => ask('revoke', () => undefined, true)}>Sign out everywhere</button>
        </Row>
        <Row
          title="Delete cloud data"
          detail="Removes the chosen categories from Writer's cloud and turns their sync off. You choose whether other devices keep their copies."
        >
          <button type="button" className="button" disabled={busy || unconfigured} onClick={() => { setCategories([]); setEverywhere(false); setAsking({ kind: 'delete_cloud' }); }}>Delete cloud data…</button>
        </Row>
        <Row
          title="Delete Writer account"
          detail="Deletes this Writer account and everything it stored in the cloud. Your Google account is not deleted."
        >
          <button type="button" className="button danger" disabled={busy || unconfigured} onClick={() => ask('delete_account', () => undefined, true)}>Delete account…</button>
        </Row>
        {tasks.unfinished.map((task) => (
          <Row
            key={task.operationId}
            title={task.action.kind === 'delete_account' ? 'Unfinished account deletion' : 'Unfinished cloud deletion'}
            detail={task.state === 'sending'
              ? 'The request may not have reached the service. Retrying sends the same request, so nothing is deleted twice.'
              : taskMessage(task).text}
          >
            <button type="button" className="button" disabled={busy || unconfigured} onClick={() => tasks.start({ kind: 'retry', operationId: task.operationId })}>Retry</button>
          </Row>
        ))}
      </Group>

      {asking && (asking.kind === 'sign_out' || asking.kind === 'switch') && (
        <Dialog title={asking.kind === 'switch' ? 'Switch account' : 'Sign out'} onClose={close}>
          <form className="dialog-body" onSubmit={(event) => {
            event.preventDefault(); close();
            if (asking.kind === 'switch') view.switchAccount(choice); else view.signOut(choice);
          }}>
            <LocalCopyChoice copy={asking.copy} value={choice} onChange={setChoice} />
            <div className="dialog-actions">
              <button type="button" className="button" onClick={close}>Cancel</button>
              <button type="submit" className="button primary">{asking.kind === 'switch' ? 'Switch account' : 'Sign out'}</button>
            </div>
          </form>
        </Dialog>
      )}

      {asking?.kind === 'revoke' && (
        <Dialog title="Sign out on all devices" onClose={close}>
          <form className="dialog-body" onSubmit={(event) => { event.preventDefault(); close(); tasks.start({ kind: 'revoke_sessions' }, copyChoice); }}>
            <p className="dialog-note">
              Every device signed in to this Writer account must sign in again. {device()[0].toUpperCase() + device().slice(1)} is signed out right away;
              other devices lose access within an hour, when their sign-in next refreshes. Cloud data and everything stored on your devices stay.
            </p>
            {asking.copy.items > 0 && <LocalCopyChoice copy={asking.copy} value={choice} onChange={setChoice} />}
            <div className="dialog-actions">
              <button type="button" className="button" onClick={close}>Cancel</button>
              <button type="submit" className="button primary">Confirm with Google</button>
            </div>
          </form>
        </Dialog>
      )}

      {asking?.kind === 'delete_cloud' && (
        <Dialog title="Delete cloud data" onClose={close}>
          <form className="dialog-body" onSubmit={(event) => {
            event.preventDefault(); close();
            tasks.start({ kind: 'delete_cloud', categories, everywhere });
          }}>
            <fieldset className="field">
              <legend>Delete from the cloud</legend>
              {CATEGORIES.map((category) => (
                <label key={category.id} className="choice">
                  <input
                    type="checkbox"
                    checked={categories.includes(category.id)}
                    onChange={(event) => setCategories(event.target.checked
                      ? [...categories, category.id]
                      : categories.filter((id) => id !== category.id))}
                  />
                  <span>{category.label}</span>
                </label>
              ))}
            </fieldset>
            <fieldset className="field">
              <legend>Your devices</legend>
              <label className="choice">
                <input type="radio" name="effect" checked={!everywhere} onChange={() => setEverywhere(false)} />
                <span><strong>Delete cloud data only</strong> — every device keeps its copies, and nothing is uploaded again unless you turn sync back on.</span>
              </label>
              <label className="choice">
                <input type="radio" name="effect" checked={everywhere} onChange={() => setEverywhere(true)} />
                <span><strong>Delete everywhere</strong> — each device also removes its synced copies when it next connects.</span>
              </label>
            </fieldset>
            <p className="dialog-note">
              Recordings and transcripts that were never uploaded stay on the device that made them. Files you exported are not affected.
              Google asks you to confirm it is you first.
            </p>
            <div className="dialog-actions">
              <button type="button" className="button" onClick={close}>Cancel</button>
              <button type="submit" className="button danger" disabled={!categories.length}>Confirm with Google</button>
            </div>
          </form>
        </Dialog>
      )}

      {asking?.kind === 'delete_account' && (
        <Dialog title="Delete Writer account" onClose={close}>
          <form className="dialog-body" onSubmit={(event) => { event.preventDefault(); close(); tasks.start({ kind: 'delete_account' }, copyChoice); }}>
            <p className="dialog-note">
              This deletes your Writer account and all its settings, dictionary and transcripts in the cloud, then signs out every device.
              <strong> Your Google account is not deleted.</strong> Signing in with Google later creates a new, empty Writer account.
            </p>
            <p className="dialog-note">
              Recordings, transcripts, settings and dictionary saved on {device()} stay unless you remove this account's synced copies below.
            </p>
            {asking.copy.items > 0 && <LocalCopyChoice copy={asking.copy} value={choice} onChange={setChoice} />}
            <div className="dialog-actions">
              <button type="button" className="button" onClick={close}>Cancel</button>
              <button type="submit" className="button danger">Confirm with Google and delete</button>
            </div>
          </form>
        </Dialog>
      )}
    </>
  );
}
