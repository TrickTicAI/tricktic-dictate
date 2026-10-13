import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  AccountAction, AccountErrorKind, AccountEvent, AccountLocalCopy, AccountLocalCopyChoice, AccountSnapshot, AccountTask,
  AccountTaskFailure, AccountTasks,
} from '@metrix-writer/contracts/desktop';
import { invoke, isTauri, listen } from '../desktop-session/runtime/native';
import { PRODUCT_NAME } from '../../product';

/*
 * Optional Google sign-in (MW-OUATH-3). The shell owns the whole flow: this
 * page asks it to start, cancel or sign out, and renders the ordered
 * `writer-account` events it publishes. No token ever reaches this code, and
 * nothing here can turn on sync — signing in alone uploads nothing.
 */

export const accountBridge = {
  snapshot: () => invoke<AccountSnapshot>('account_snapshot'),
  signIn: (attemptId: string) => invoke<void>('account_sign_in', { attemptId }),
  reauthenticate: (attemptId: string) => invoke<void>('account_reauthenticate', { attemptId }),
  cancel: (attemptId: string) => invoke<void>('account_cancel_sign_in', { attemptId }),
  signOut: (localCopy?: AccountLocalCopyChoice) => invoke<void>('account_sign_out', localCopy ? { localCopy } : undefined),
  switchAccount: (attemptId: string, localCopy?: AccountLocalCopyChoice) => invoke<void>('account_switch', { attemptId, localCopy }),
  localCopy: () => invoke<AccountLocalCopy>('account_local_copy'),
  tasks: () => invoke<AccountTasks>('account_tasks'),
  sensitive: (attemptId: string, action: AccountAction, localCopy?: AccountLocalCopyChoice) =>
    invoke<string>('account_sensitive', { attemptId, action, localCopy }),
};

const KINDS: AccountErrorKind[] = ['not_configured', 'cancelled', 'timed_out', 'offline', 'provider_rejected', 'invalid_response', 'reauth_required', 'unavailable'];

/** The fixed wording for each kind. Upstream text is never shown. */
export function accountMessage(kind: AccountErrorKind): string {
  switch (kind) {
    case 'not_configured': return 'This build has no Google sign-in configuration.';
    case 'cancelled': return 'Sign-in was cancelled. Nothing changed.';
    case 'timed_out': return `Google did not return to ${PRODUCT_NAME} within three minutes. Try again.`;
    case 'offline': return 'Google could not be reached. Check your connection and try again.';
    case 'provider_rejected': return 'Google or Firebase declined this sign-in.';
    case 'invalid_response': return 'The sign-in reply could not be verified, so nothing was saved.';
    case 'reauth_required': return 'Sign in again with the same Google account.';
    case 'unavailable': return 'Sign-in could not start. Close any open sign-in and try again.';
  }
}

const kindOf = (cause: unknown): AccountErrorKind =>
  KINDS.includes(cause as AccountErrorKind) ? cause as AccountErrorKind : 'unavailable';

export interface AccountView {
  snapshot: AccountSnapshot | null;
  /** The last attempt's result, until the next action. */
  notice: { tone: 'info' | 'alert'; text: string } | null;
  busy: boolean;
  signIn: () => void;
  reauthenticate: () => void;
  cancel: () => void;
  signOut: (localCopy?: AccountLocalCopyChoice) => void;
  switchAccount: (localCopy?: AccountLocalCopyChoice) => void;
}

/**
 * Follows the shell's account state. Events are applied strictly in sequence
 * order; the initial snapshot is used only if no newer snapshot event arrived
 * while it was being read.
 */
export function useAccount(enabled = isTauri()): AccountView {
  const [snapshot, setSnapshot] = useState<AccountSnapshot | null>(null);
  const [notice, setNotice] = useState<AccountView['notice']>(null);
  const [busy, setBusy] = useState(false);
  const sequence = useRef(0);
  const current = useRef<AccountSnapshot | null>(null);

  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let heard = false;
    const apply = (event: AccountEvent) => {
      if (disposed || event.sequence <= sequence.current) return;
      sequence.current = event.sequence;
      if (event.type === 'auth_snapshot') {
        heard = true;
        current.current = event.snapshot;
        setSnapshot(event.snapshot);
      } else if (event.type === 'sign_in_finished') {
        setNotice(event.outcome === 'success' ? null
          : event.outcome === 'cancelled' ? { tone: 'info', text: accountMessage('cancelled') }
          : { tone: 'alert', text: accountMessage(event.error ?? 'unavailable') });
      } else if (event.type === 'session_invalidated' && event.reason === 'reauth_required') {
        setNotice({ tone: 'alert', text: 'Google sign-in expired or was revoked. Dictation is unaffected; sign in again to reconnect this account.' });
      }
    };
    void Promise.resolve()
      .then(() => listen<AccountEvent>('writer-account', apply))
      .then((stop) => { if (disposed) stop(); else unlisten = stop; })
      .catch(() => { /* Older hosts have no account events; the snapshot still loads. */ })
      .then(() => accountBridge.snapshot())
      .then((loaded) => { if (!disposed && !heard) { current.current = loaded; setSnapshot(loaded); } })
      .catch(() => { if (!disposed && !heard) setSnapshot(null); });
    return () => { disposed = true; unlisten?.(); };
  }, [enabled]);

  const run = useCallback(async (action: () => Promise<void>, failure?: (kind: AccountErrorKind) => string) => {
    setBusy(true); setNotice(null);
    try { await action(); }
    catch (cause) {
      const kind = kindOf(cause);
      setNotice({ tone: 'alert', text: failure ? failure(kind) : accountMessage(kind) });
    } finally { setBusy(false); }
  }, []);

  return {
    snapshot,
    notice,
    busy,
    signIn: () => void run(() => accountBridge.signIn(crypto.randomUUID())),
    reauthenticate: () => void run(() => accountBridge.reauthenticate(crypto.randomUUID())),
    cancel: () => {
      const attempt = current.current?.attemptId;
      if (attempt) void run(() => accountBridge.cancel(attempt));
    },
    signOut: (localCopy) => void run(() => accountBridge.signOut(localCopy), () => 'Signed out, but the saved sign-in or a local copy could not be removed from this computer. Try again.'),
    switchAccount: (localCopy) => void run(() => accountBridge.switchAccount(crypto.randomUUID(), localCopy)),
  };
}

/** Whether this build offers sign-in at all. Public releases do not. */
export const offersSignIn = (snapshot: AccountSnapshot | null) =>
  !!snapshot && snapshot.availability !== 'unavailable';

/** The fixed wording for each sensitive-operation failure. */
export function taskFailure(kind: AccountTaskFailure): string {
  switch (kind) {
    case 'not_configured': return 'This build has no account service configured.';
    case 'reauth_required': return 'Google needs you to confirm it is you again. Try again.';
    case 'cancelled': return 'Stopped: the Google confirmation was cancelled or the signed-in account changed.';
    case 'offline': return `${PRODUCT_NAME}'s account service could not be reached. Check your connection and try again.`;
    case 'unavailable': return 'The account service is unavailable right now. Try again later.';
    case 'stale': return 'Your account changed on another device. Review your choice and try again.';
    case 'account_unavailable': return 'This Writer account is already being deleted.';
    case 'rate_limited': return 'Too many requests. Wait a minute and try again.';
    case 'refused': return 'The account service refused this request. Nothing was changed.';
    case 'invalid_response': return 'The account service sent an unexpected reply, so nothing was assumed. Try again.';
  }
}

/** What an operation's current state means, in fixed words. */
export function taskMessage(task: AccountTask): { tone: 'info' | 'alert'; text: string } {
  const { action, state } = task;
  switch (state) {
    case 'reauthenticating': return { tone: 'info', text: 'Confirm it is you in the Google tab that opened. Dictation keeps working meanwhile.' };
    case 'sending': return { tone: 'info', text: 'Sending the request…' };
    case 'pending':
    case 'running': return { tone: 'info', text: 'Deleting from the cloud. This can take a few minutes; you can keep dictating.' };
    case 'failed_retryable': return { tone: 'alert', text: 'Deletion stopped partway. The service retries it automatically, and nothing is uploaded again meanwhile. You can also retry now.' };
    case 'failed': return { tone: 'alert', text: taskFailure(task.error ?? 'unavailable') };
    case 'completed':
      if (action.kind === 'revoke_sessions') return { tone: 'info', text: 'Signed out on every device. This computer is signed out now; other devices lose access when their sign-in next refreshes, within an hour.' };
      if (action.kind === 'delete_account') return { tone: 'info', text: 'Your Writer account and its cloud data are deleted. Your Google account is unchanged.' };
      return { tone: 'info', text: action.everywhere
        ? 'Cloud data deleted and sync turned off. Other devices remove their synced copies when they next connect.'
        : 'Cloud data deleted and sync turned off. Copies on your devices were kept.' };
  }
}

export interface AccountTasksView {
  configured: boolean;
  /** The latest operation's state, until the next one starts. */
  latest: AccountTask | null;
  unfinished: AccountTask[];
  busy: boolean;
  /** Set when the command itself was refused before any progress. */
  refused: string | null;
  start: (action: AccountAction, localCopy?: AccountLocalCopyChoice) => void;
}

const settled = (task: AccountTask) => task.state === 'completed' || task.state === 'failed' || task.state === 'failed_retryable';

/** Follows `writer-account-task` for the Account page. Sensitive work runs in the shell. */
export function useAccountTasks(enabled: boolean): AccountTasksView {
  const [configured, setConfigured] = useState(false);
  const [latest, setLatest] = useState<AccountTask | null>(null);
  const [unfinished, setUnfinished] = useState<AccountTask[]>([]);
  const [refused, setRefused] = useState<string | null>(null);
  const reload = useCallback(() => {
    void accountBridge.tasks().then((tasks) => { setConfigured(tasks.configured); setUnfinished(tasks.unfinished); }).catch(() => undefined);
  }, []);

  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<AccountTask>('writer-account-task', (task) => {
      if (disposed) return;
      setLatest(task);
      if (settled(task)) reload();
    }).then((stop) => { if (disposed) stop(); else unlisten = stop; }).catch(() => undefined);
    reload();
    return () => { disposed = true; unlisten?.(); };
  }, [enabled, reload]);

  return {
    configured,
    latest,
    unfinished,
    busy: !!latest && !settled(latest),
    refused,
    start: (action, localCopy) => {
      setRefused(null);
      accountBridge.sensitive(crypto.randomUUID(), action, localCopy)
        .catch((cause) => setRefused(accountMessage(kindOf(cause))));
    },
  };
}
