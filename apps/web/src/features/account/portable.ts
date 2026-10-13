import { useCallback, useEffect, useState } from 'react';
import { invoke, isTauri, listen } from '../desktop-session/runtime/native';
import { PRODUCT_NAME } from '../../product';

/*
 * Portable backup and optional sync (MW-OUATH-6). The shell owns the files,
 * the cloud transport and every token; this page previews and confirms. Errors
 * arrive as fixed kinds and are worded here, never shown raw.
 */

export type Category = 'settings' | 'dictionary';
export type Choice = 'incoming' | 'local';
export type Disposition = 'restore_once' | 'restore_and_sync';
export type SettingKey = 'ui_language' | 'input_language' | 'output_language' | 'transcription_model'
  | 'dictation_mode' | 'cleanup' | 'delivery' | 'theme' | 'polish_instructions';
export type Reason = 'unsupported' | 'missing_key' | 'model_not_installed' | 'unknown_model'
  | 'incompatible_language' | 'reasoning_not_supported' | 'no_matching_mode';
export type Outcome = { outcome: 'applied' | 'unchanged' } | { outcome: 'preserved' | 'partial'; reason: Reason };

export interface SettingChange { id: string; key: SettingKey; current: unknown; incoming: unknown; outcome: Outcome }
export interface EntryView { id: string; entry: DictionaryData }
export interface DictionaryData { kind: 'hint' | 'correction' | 'snippet'; enabled: boolean; text?: string; from?: string; to?: string; phrase?: string }
export interface Preview {
  import_id: string;
  export_id: string;
  mode: 'merge' | 'replace_selected';
  categories: Category[];
  disposition: Disposition;
  expected_local_revision: number;
  settings: SettingChange[];
  dictionary: { added: EntryView[]; unchanged: number; conflicts: { id: string; local: DictionaryData; incoming: DictionaryData }[]; removed: EntryView[]; overlapping: number; reordered: boolean };
  apply_ids: string[];
  preserve_ids: string[];
  conflict_ids: string[];
}
export type Receipt =
  | { state: 'applied'; import_id: string; local_revision: number; rollback_snapshot_id: string; outcomes: Partial<Record<SettingKey, Outcome>>; theme: { id: string; kind: string } | null }
  | { state: 'not_applied'; import_id: string; reason: 'invalid' | 'unsupported_version' | 'conflict' | 'stale_preview' | 'storage_failed' };
export interface SyncPreview { preview: Preview; cloud: Partial<Record<Category, number>>; uploads: Partial<Record<Category, number>>; enabled: Record<Category, boolean> }
export type RemoteError = 'offline' | 'busy' | 'permission_denied' | 'reauth_required' | 'superseded' | 'invalid_response' | 'storage_failed';
export interface Conflict { id: string; category: Category; key: SettingKey | null; local: unknown; remote: unknown }
export interface SyncStatus {
  joined: Category[];
  paused: boolean;
  consent: Record<Category, boolean> | null;
  pending: number;
  conflicts: Conflict[];
  last_success_ms: number | null;
  last_error: RemoteError | null;
  notices: ({ kind: 'category_stopped'; category: Category } | { kind: 'account_reset' })[];
}
export interface PortableStatus {
  sync_available: boolean;
  last_import: { import_id: string; rollback_snapshot_id: string; local_revision: number; current_revision: number; categories: Category[] } | null;
  kept: { key: SettingKey; value: unknown; reason: Reason }[];
  theme: { id: string; kind: string } | null;
  sync: SyncStatus | null;
}

export const portableBridge = {
  status: () => invoke<PortableStatus>('portable_status'),
  export: () => invoke<boolean>('portable_export'),
  preview: (categories: Category[], mode: Preview['mode']) => invoke<Preview | null>('portable_import_preview', { categories, mode }),
  apply: (importId: string, choices: Record<string, Choice>) => invoke<Receipt>('portable_import_apply', { importId, choices }),
  discard: (importId: string) => invoke<void>('portable_import_discard', { importId }),
  rollback: (importId: string, rollbackSnapshotId: string, expectedLocalRevision: number) =>
    invoke<Receipt>('portable_rollback', { importId, rollbackSnapshotId, expectedLocalRevision }),
  theme: (id: string, kind: string) => invoke<void>('portable_theme', { id, kind }),
  syncPreview: (categories: Category[], disposition: Disposition) => invoke<SyncPreview>('sync_preview', { categories, disposition }),
  syncConfirm: (importId: string, choices: Record<string, Choice>) => invoke<Receipt>('sync_confirm', { importId, choices }),
  syncControl: (action: 'now' | 'pause' | 'resume' | 'dismiss') => invoke<void>('sync_control', { action }),
  syncTurnOff: (category: Category) => invoke<void>('sync_turn_off', { category }),
  syncResolve: (id: string, choice: Choice) => invoke<void>('sync_resolve', { id, choice }),
};

export const SETTING_LABEL: Record<SettingKey, string> = {
  ui_language: 'Interface language',
  input_language: 'Spoken language',
  output_language: 'Output language',
  transcription_model: 'Speech model',
  dictation_mode: 'Default mode',
  cleanup: 'Cleanup',
  delivery: 'Pasted text',
  theme: 'Theme',
  polish_instructions: 'Polishing instructions',
};

export const REASON: Record<Reason, string> = {
  unsupported: `${PRODUCT_NAME} on this computer has no such option.`,
  missing_key: 'Add this provider’s key on this computer to use it.',
  model_not_installed: 'Download or import this model on this computer to use it.',
  unknown_model: 'This model is not available here.',
  incompatible_language: 'This language and model combination is not supported here.',
  reasoning_not_supported: 'The model was applied; its reasoning level has no desktop setting.',
  no_matching_mode: 'No mode of that kind exists here.',
};

const LANGUAGE: Record<string, string> = { en: 'English', he: 'Hebrew', mixed: 'Mixed Hebrew and English' };
/** A portable setting value in words. */
export function describe(key: SettingKey, value: unknown): string {
  if (value === null || value === undefined) return 'Not set';
  if (typeof value === 'string') {
    if (key === 'ui_language' || key === 'input_language') return LANGUAGE[value] ?? value;
    return value.charAt(0).toUpperCase() + value.slice(1);
  }
  const v = value as { mode?: string; language?: string; provider?: string; model?: string; selection?: { provider: string; model: string }; reasoning?: string };
  if (key === 'output_language') return v.mode === 'translate' ? `Translate to ${LANGUAGE[v.language ?? ''] ?? v.language}` : 'Keep the spoken language';
  if (key === 'transcription_model') return `${v.model} (${v.provider})`;
  if (key === 'cleanup') return v.mode === 'off' ? 'Off' : `${v.selection?.model} (${v.selection?.provider})${v.reasoning && v.reasoning !== 'provider_default' ? `, ${v.reasoning} reasoning` : ''}`;
  if (key === 'polish_instructions') {
    const polish = value as { instructions?: string; formatting?: { rewrite?: string } };
    const text = polish.instructions ?? '';
    const first = text.split('\n')[0].slice(0, 80);
    return `${first}${text.length > 80 ? '…' : ''} (${polish.formatting?.rewrite ?? 'polished'})`;
  }
  return JSON.stringify(value);
}
export function describeEntry(entry: DictionaryData): string {
  if (entry.kind === 'hint') return entry.text ?? '';
  if (entry.kind === 'correction') return `${entry.from} → ${entry.to}`;
  return `“${entry.phrase}” → ${entry.text}`;
}

/** The fixed wording for a refused command. */
export function portableMessage(cause: unknown): string {
  const kind = typeof cause === 'string' ? cause : '';
  switch (kind) {
    case 'invalid_file': return 'That file is not a Writer settings file, or it is damaged. Nothing was changed.';
    case 'unsupported_version': return `That file was made by a newer version of ${PRODUCT_NAME}. Update this app to import it. Nothing was changed.`;
    case 'not_signed_in': return 'Sign in with Google first.';
    case 'offline': return 'Your account could not be reached. Check your connection; everything on this computer keeps working.';
    case 'busy': return 'The sync service is busy. It will try again automatically.';
    case 'permission_denied': return 'Your account refused this change. Open sync setup again to see its current state.';
    case 'reauth_required': return 'Sign in again with the same Google account to continue syncing.';
    case 'superseded': return 'The signed-in account changed while this was running. Nothing was applied.';
    case 'consent_changed': return 'Sync was changed on another device meanwhile. Start again to see the current state.';
    case 'not_previewed': return 'This preview is no longer current. Start again.';
    case 'storage_failed': return 'History could not be saved. Check free space and folder permissions; original recordings are retained.';
    case 'invalid_response': return 'The sync service sent an unexpected reply. Nothing was applied.';
    case 'unavailable': return 'Backup and sync are unavailable right now.';
    default: return kind && kind.length < 300 ? kind : 'That did not work. Nothing was changed.';
  }
}
export function notAppliedMessage(reason: Extract<Receipt, { state: 'not_applied' }>['reason']): string {
  switch (reason) {
    case 'stale_preview': return 'Your settings changed after this preview. Nothing was applied; start again to see a fresh preview.';
    case 'conflict': return 'Choose which version to keep for every entry marked “Differs”.';
    case 'storage_failed': return 'The change could not be saved, so everything was left as it was.';
    case 'unsupported_version': return `This came from a newer version of ${PRODUCT_NAME}. Nothing was applied.`;
    case 'invalid': return 'The result would exceed the dictionary’s limits (200 entries of each kind). Nothing was applied.';
  }
}

/** Follows the shell's backup/sync status and its change events. */
export function usePortable(enabled = isTauri()) {
  const [status, setStatus] = useState<PortableStatus | null>(null);
  const [failed, setFailed] = useState(false);
  const reload = useCallback(() => {
    if (!enabled) return;
    void portableBridge.status().then((value) => { setStatus(value); setFailed(false); }).catch(() => setFailed(true));
  }, [enabled]);
  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let stop: (() => void) | undefined;
    reload();
    void Promise.resolve()
      .then(() => listen<{ kind: string }>('writer-sync', () => reload()))
      .then((unlisten) => { if (disposed) unlisten(); else stop = unlisten; })
      .catch(() => { /* Older hosts publish no sync events. */ });
    return () => { disposed = true; stop?.(); };
  }, [enabled, reload]);
  return { status, failed, reload };
}
