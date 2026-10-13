/*
 * Desktop settings bridge, hand-written.
 *
 * Unlike `index.d.ts`, these shapes are not generated from `schema.json`: they
 * are not HTTP contracts. They describe the one typed channel between the web
 * UI and the Tauri shell that carries shortcut chords, overlay preferences and
 * the resolved theme colours the floating panel paints with.
 *
 * The Rust counterpart is `apps/desktop/tauri/src/shell/preferences.rs`, whose
 * `Preferences` struct is `deny_unknown_fields`. Every field below must exist
 * there under the same name, and nothing may be sent that is not listed.
 *
 * Audio, transcript bodies and provider credentials never travel on this
 * channel. API keys have no HTTP write route and no web representation at all;
 * they are entered through the native dialog only.
 */

/** Where the floating panel sits, when it is not being dragged. */
export type OverlayPlacement = 'bottom' | 'top' | 'remembered';

/**
 * When the floating panel is on screen. `recording` shows it for the duration
 * of a dictation; `always` keeps an idle panel up that can start one.
 */
export type OverlayVisibility = 'off' | 'recording' | 'always';

/**
 * Which window a finished dictation's text is pasted into: the one in front when
 * it stopped (the default), or the one it was started in. Command mode always
 * uses the window it was started in, where its selection was captured.
 */
export type PasteInto = 'stop' | 'start';

/** Mirrors the Rust `Preferences` struct exactly, including field order-independence. */
export interface DesktopPreferences {
  /** Record/stop chord, e.g. `Ctrl+Shift+F9`. Layout-independent virtual keys. */
  recording: string;
  /** Discard chord: throws the dictation away. May not share its final key with `recording`. */
  cancel: string;
  /** False means hold-to-dictate; true means press once to start, again to stop. */
  toggle: boolean;
  /** The shell always sends it; a document without it means `stop`. */
  paste_into?: PasteInto;
  /** Windows/macOS restoration. Missing in older documents means false. */
  preserve_clipboard?: boolean;
  /** Opt-in automatic-delivery history/sync exclusion; explicit Copy is unchanged. */
  private_clipboard?: boolean;
  visibility: OverlayVisibility;
  placement: OverlayPlacement;
  /**
   * Physical screen position, only meaningful with `placement: 'remembered'`.
   * Stored absolute; the shell reinterprets it as the same relative place
   * inside another monitor's work area when the panel moves screens.
   */
  position: [number, number] | null;
  /** Multiplies the panel's base 112x44 logical size. Clamped to 0.75-2.5. */
  scale: number;
  reduced_motion: boolean;
  /** How long a finished session stays on screen. The shell clamps to 300–60000. */
  completion_hold_ms: number;
  auto_dismiss: boolean;
  /**
   * The panel's own colours. `null` follows the app theme, which is the default
   * and what every document written before this field existed means.
   */
  panel_colors: PanelColors | null;
}

/**
 * A floating panel painted in colours of its own. Both values must be a literal
 * `#rrggbb`: the shell refuses anything else, because the panel writes them
 * straight into its CSS.
 */
export interface PanelColors {
  /** The pill's fill. */
  background: string;
  /** The voice animation — level bars and processing dots — and the panel's glyphs. */
  wave: string;
}

/**
 * Resolved colours for the floating panel, derived from the active theme by the
 * web layer so the shell never has to know what a theme is. Sent whenever the
 * theme changes and once on startup.
 */
export interface OverlayThemeMessage {
  kind: 'dark' | 'light';
  bg: string;
  border: string;
  text: string;
  text_muted: string;
  accent: string;
  accent_text: string;
  surface: string;
  danger: string;
  /** The page's own background, painted behind it by the window before the page loads. */
  window?: string;
}

/** Commands the main window may invoke on the shell. */
export interface DesktopSettingsBridge {
  preferences_get(): Promise<DesktopPreferences>;
  preferences_save(args: { preferences: DesktopPreferences }): Promise<void>;
  preferences_theme(args: { theme: OverlayThemeMessage }): Promise<void>;
  import_model(args: { model: string }): Promise<void>;
  credential_prompt(args: { provider: 'openai' | 'gemini' }): Promise<void>;
  credential_remove(args: { provider: 'openai' | 'gemini' }): Promise<void>;
  open_data_folder(): Promise<void>;
  window_command(args: { command: 'minimize' | 'toggle-maximize' | 'close' }): Promise<void>;
  window_state(): Promise<{ maximized: boolean }>;
}

/**
 * Optional Google sign-in (MW-OUATH-3), the desktop side of
 * `infra/firebase/auth-adapter-contract.md` v1. Only fixed states and kinds,
 * the Firebase account key and display metadata cross to the page; tokens never
 * do. Signing in alone never enables sync: nothing here carries consent.
 */
export type AccountState = 'guest' | 'signing_in' | 'signed_in' | 'reauth_required';
export type AccountErrorKind =
  | 'not_configured'
  | 'cancelled'
  | 'timed_out'
  | 'offline'
  | 'provider_rejected'
  | 'invalid_response'
  | 'reauth_required'
  | 'unavailable';
/** `unavailable`: built without sign-in (public releases until MW-RELEASE-12). */
export type AccountAvailability = 'unavailable' | 'not_configured' | 'ready';
export interface AccountKey {
  projectId: string;
  /** Firebase UID. Identity is this key, never the email. */
  uid: string;
}
export interface AccountDisplay {
  email: string | null;
  name: string | null;
}
export interface AccountSnapshot {
  state: AccountState;
  availability: AccountAvailability;
  generation: number;
  accountKey?: AccountKey;
  display?: AccountDisplay;
  /** The sign-in attempt in progress, so a reloaded page can still cancel it. */
  attemptId?: string;
  reauthenticating: boolean;
}
interface AccountEventBase {
  version: 1;
  /** Process-local order; ignore an event at or below the last one applied. */
  sequence: number;
  /** Advances whenever a session or pending sign-in is invalidated. */
  generation: number;
}
/** Payload of the `writer-account` event. */
export type AccountEvent = AccountEventBase & (
  | { type: 'auth_snapshot'; snapshot: AccountSnapshot }
  | { type: 'sign_in_started'; attemptId: string; reauthenticate: boolean }
  | { type: 'sign_in_finished'; attemptId: string; outcome: 'success' | 'cancelled' | 'error'; error?: AccountErrorKind }
  | { type: 'session_refreshed'; accountKey: AccountKey }
  | { type: 'session_invalidated'; accountKey: AccountKey; reason: 'signed_out' | 'reauth_required' }
);
/** Main-window account commands. Each rejects with an `AccountErrorKind` string. */
export interface DesktopAccountBridge {
  account_snapshot(): Promise<AccountSnapshot>;
  account_sign_in(args: { attemptId: string }): Promise<void>;
  account_reauthenticate(args: { attemptId: string }): Promise<void>;
  account_cancel_sign_in(args: { attemptId: string }): Promise<void>;
  account_sign_out(args?: { localCopy?: AccountLocalCopyChoice }): Promise<void>;
  account_switch(args: { attemptId: string; localCopy?: AccountLocalCopyChoice }): Promise<void>;
  account_local_copy(): Promise<AccountLocalCopy>;
  account_tasks(): Promise<AccountTasks>;
  /** Resolves with the operation ID; progress arrives as `writer-account-task`. */
  account_sensitive(args: { attemptId: string; action: AccountAction; localCopy?: AccountLocalCopyChoice }): Promise<string>;
}

/** MW-OUATH-8 account lifecycle. See docs/testing/account-sync/account-lifecycle.md. */
export type AccountLocalCopyChoice = 'leave' | 'keep' | 'remove';
export interface AccountLocalCopy { items: number; unsynced: number }
export type AccountDataCategory = 'settings' | 'dictionary' | 'transcripts';
export type AccountAction =
  | { kind: 'revoke_sessions' }
  | { kind: 'delete_cloud'; categories: AccountDataCategory[]; everywhere: boolean }
  | { kind: 'delete_account' }
  | { kind: 'retry'; operationId: string };
export type AccountTaskFailure =
  | 'not_configured' | 'reauth_required' | 'cancelled' | 'offline' | 'unavailable' | 'stale'
  | 'account_unavailable' | 'rate_limited' | 'refused' | 'invalid_response';
export type AccountTaskState =
  | 'reauthenticating' | 'sending' | 'pending' | 'running' | 'failed_retryable' | 'completed' | 'failed';
/** Payload of the `writer-account-task` event. Never a token, UID or service text. */
export interface AccountTask {
  version: 1;
  operationId: string;
  action: Exclude<AccountAction, { kind: 'retry' }>;
  state: AccountTaskState;
  error?: AccountTaskFailure;
}
export interface AccountTasks { configured: boolean; unfinished: AccountTask[] }
