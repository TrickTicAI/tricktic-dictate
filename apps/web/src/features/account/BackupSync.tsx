import { useState } from 'react';
import { Dialog } from '../shell/Dialog';
import { Group, Row } from '../settings/controls';
import { isMac } from '../settings/platform';
import {
  REASON, SETTING_LABEL, describe, describeEntry, notAppliedMessage, portableBridge, portableMessage,
  type Category, type Choice, type Disposition, type Preview, type PortableStatus, type Receipt, type SyncPreview,
} from './portable';

const CATEGORY_LABEL: Record<Category, string> = { settings: 'Settings', dictionary: 'Dictionary' };
const both = (): Category[] => ['settings', 'dictionary'];

/**
 * Settings → Account → Backup. Export and import need no account and work in
 * every desktop build. Sync appears only for a signed-in account in a build
 * with sign-in, and starts only after this device confirms a preview.
 */
export function BackupSync({ status, signedIn, onChanged }: {
  status: PortableStatus | null;
  signedIn: boolean;
  onChanged: () => void;
}) {
  const device = isMac() ? 'this Mac' : 'this PC';
  const [note, setNote] = useState<{ tone?: 'alert'; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [importing, setImporting] = useState(false);
  const [syncing, setSyncing] = useState(false);
  if (!status) return null;
  const run = async (action: () => Promise<string | null>) => {
    setBusy(true); setNote(null);
    try { const text = await action(); if (text) setNote({ text }); }
    catch (cause) { setNote({ tone: 'alert', text: portableMessage(cause) }); }
    finally { setBusy(false); onChanged(); }
  };
  const last = status.last_import;
  const undoable = !!last && last.current_revision === last.local_revision;

  return (
    <>
      <Group title="Backup and restore" note={note?.text} noteTone={note?.tone}>
        <Row
          title="Export settings and dictionary"
          detail={`Saves a file with your languages, speech and cleanup model choices, default mode, theme and dictionary. Not included: provider keys, recordings, transcripts, microphone, shortcuts, panel placement and model files. The file is not encrypted.`}
        >
          <button type="button" className="button" disabled={busy}
            onClick={() => void run(async () => (await portableBridge.export()) ? 'Exported. Keep the file somewhere private; it contains your dictionary.' : null)}>
            Export…
          </button>
        </Row>
        <Row
          title="Import from a file"
          detail={`Shows exactly what would change before anything is applied, and nothing is uploaded. Models that are not set up on ${device} and missing provider keys are listed, not switched on.`}
        >
          <button type="button" className="button" disabled={busy} onClick={() => setImporting(true)}>Import…</button>
        </Row>
        {last && (
          <Row
            title="Undo the last import"
            detail={undoable
              ? `Puts back the ${last.categories.map(c => CATEGORY_LABEL[c].toLowerCase()).join(' and ')} you had before it.`
              : 'Settings or dictionary changed after the last import, so undoing it would overwrite those changes.'}
          >
            <button type="button" className="button" disabled={busy || !undoable}
              onClick={() => void run(async () => {
                const receipt = await portableBridge.rollback(last.import_id, last.rollback_snapshot_id, last.local_revision);
                return receipt.state === 'applied' ? 'The import was undone.' : notAppliedMessage(receipt.reason);
              })}>
              Undo import
            </button>
          </Row>
        )}
        {status.kept.length > 0 && (
          <Row
            title={`Not applied on ${device}`}
            detail={<>These restored choices are kept and travel with your backup and sync, but are not in use here:{' '}
              {status.kept.map(k => <span key={k.key} className="kept-setting"><br />{SETTING_LABEL[k.key]}: {describe(k.key, k.value)}. {REASON[k.reason]}</span>)}
            </>}
            stacked
          />
        )}
      </Group>

      {status.sync_available && signedIn && (
        <SyncGroup status={status} busy={busy} run={run} onSetup={() => setSyncing(true)} device={device} />
      )}

      {importing && <ImportDialog onClose={() => { setImporting(false); onChanged(); }} onDone={(text) => setNote({ text })} />}
      {syncing && <SyncDialog onClose={() => { setSyncing(false); onChanged(); }} onDone={(text) => setNote({ text })} />}
    </>
  );
}

function ago(ms: number) {
  const minutes = Math.round((Date.now() - ms) / 60000);
  return minutes < 1 ? 'just now' : minutes < 60 ? `${minutes} min ago` : new Date(ms).toLocaleString();
}

function SyncGroup({ status, busy, run, onSetup, device }: {
  status: PortableStatus;
  busy: boolean;
  run: (action: () => Promise<string | null>) => Promise<void>;
  onSetup: () => void;
  device: string;
}) {
  const sync = status.sync;
  const joined = sync?.joined ?? [];
  const [confirmOff, setConfirmOff] = useState(false);
  const notices = sync?.notices ?? [];
  return (
    <Group title="Sync between devices">
      {notices.length > 0 && (
        <Row
          title="Sync changed elsewhere"
          detail={notices.map(n => n.kind === 'account_reset'
            ? 'Your Writer account’s cloud data was reset or is being deleted. Nothing on this computer was removed.'
            : `${CATEGORY_LABEL[n.category]} sync was turned off, or its cloud copy deleted, on another device. ${CATEGORY_LABEL[n.category]} on ${device} stayed as it was.`).join(' ')}
        >
          <button type="button" className="button" disabled={busy} onClick={() => void run(async () => { await portableBridge.syncControl('dismiss'); return null; })}>Dismiss</button>
        </Row>
      )}
      {joined.length === 0 ? (
        <Row
          title="Settings and dictionary sync is off"
          detail={`Signing in uploaded nothing. Setting up sync first shows what is already in your account, then lets you restore it once or keep ${device} in sync. Synced settings and dictionary are stored in Google Firebase under your account; they are not end-to-end encrypted. History has its own consent below; recordings and provider keys are not synced.`}
        >
          <button type="button" className="button primary" disabled={busy} onClick={onSetup}>Set up sync…</button>
        </Row>
      ) : (
        <>
          <Row
            title={`Syncing ${joined.map(c => CATEGORY_LABEL[c].toLowerCase()).join(' and ')}${sync?.paused ? ' — paused on this device' : ''}`}
            detail={<>
              {sync?.last_success_ms ? `Last synced ${ago(sync.last_success_ms)}.` : 'Not synced yet.'}{' '}
              {sync?.pending ? `${sync.pending} change${sync.pending === 1 ? '' : 's'} waiting to upload.` : 'No changes waiting.'}{' '}
              {sync?.last_error && <span className="sync-error">{portableMessage(sync.last_error)}</span>}
              <br />Changes are saved on {device} first. Sync never runs a transcription, downloads a model or pastes text.
            </>}
          >
            <button type="button" className="button" disabled={busy || sync?.paused} onClick={() => void run(async () => { await portableBridge.syncControl('now'); return null; })}>Sync now</button>
            <button type="button" className="button" disabled={busy} onClick={() => void run(async () => { await portableBridge.syncControl(sync?.paused ? 'resume' : 'pause'); return null; })}>
              {sync?.paused ? 'Resume' : 'Pause on this device'}
            </button>
          </Row>
          {(sync?.conflicts ?? []).map(conflict => (
            <Row
              key={conflict.id}
              title={conflict.key ? `${SETTING_LABEL[conflict.key]} was changed on two devices` : 'A dictionary entry was changed on two devices'}
              detail={<>
                On {device}: {conflict.local === null ? 'deleted' : conflict.key ? describe(conflict.key, conflict.local) : describeEntry(conflict.local as never)}<br />
                In your account: {conflict.remote === null ? 'deleted' : conflict.key ? describe(conflict.key, conflict.remote) : describeEntry(conflict.remote as never)}
              </>}
            >
              <button type="button" className="button" disabled={busy} onClick={() => void run(async () => { await portableBridge.syncResolve(conflict.id, 'local'); return null; })}>Keep this device’s</button>
              <button type="button" className="button" disabled={busy} onClick={() => void run(async () => { await portableBridge.syncResolve(conflict.id, 'incoming'); return null; })}>Use the account’s</button>
            </Row>
          ))}
          <Row
            title="Turn off sync"
            detail="Stops syncing on all your devices. Nothing is deleted: what is already in your account stays there, and everything on this computer stays here."
            stacked={confirmOff}
          >
            {!confirmOff ? (
              <button type="button" className="button" disabled={busy} onClick={() => setConfirmOff(true)}>Turn off…</button>
            ) : (
              <div className="row">
                <button type="button" className="button" disabled={busy} onClick={() => setConfirmOff(false)}>Keep syncing</button>
                <button type="button" className="button danger" disabled={busy} onClick={() => void run(async () => {
                  for (const category of joined) await portableBridge.syncTurnOff(category);
                  setConfirmOff(false);
                  return 'Sync is off on all your devices.';
                })}>Turn off on all devices</button>
              </div>
            )}
          </Row>
        </>
      )}
    </Group>
  );
}

function Categories({ value, onChange }: { value: Category[]; onChange: (next: Category[]) => void }) {
  return (
    <div className="row" role="group" aria-label="What to include">
      {both().map(category => (
        <label key={category} className="checkbox">
          <input type="checkbox" checked={value.includes(category)}
            onChange={(e) => onChange(e.target.checked ? both().filter(c => c === category || value.includes(c)) : value.filter(c => c !== category))} />
          {CATEGORY_LABEL[category]}
        </label>
      ))}
    </div>
  );
}

/** What a preview would change, with a choice for every conflict and setting. */
function PreviewBody({ preview, choices, onChoice, cloud }: {
  preview: Preview;
  choices: Record<string, Choice>;
  onChoice: (id: string, choice: Choice) => void;
  cloud?: SyncPreview;
}) {
  const d = preview.dictionary;
  const source = cloud ? 'your account' : 'the file';
  const nothing = !preview.settings.length && !d.added.length && !d.conflicts.length && !d.removed.length;
  return (
    <div className="portable-preview">
      {cloud && (
        <p className="dialog-note">
          In your account: {cloud.cloud.settings ?? 0} settings and {cloud.cloud.dictionary ?? 0} dictionary entries.
          {(cloud.uploads.settings || cloud.uploads.dictionary) ? ` Keeping in sync would upload ${cloud.uploads.settings ?? 0} settings and ${cloud.uploads.dictionary ?? 0} entries that exist only here.` : ''}
        </p>
      )}
      {nothing && <p className="dialog-note">Nothing would change: everything already matches {source}.</p>}
      {preview.settings.length > 0 && (
        <section aria-label="Setting changes">
          <strong>Settings</strong>
          <ul className="preview-list">
            {preview.settings.map(change => (
              <li key={change.id}>
                <span>{SETTING_LABEL[change.key]}: {describe(change.key, change.current)} → {describe(change.key, change.incoming)}</span>
                {change.outcome.outcome === 'preserved' || change.outcome.outcome === 'partial'
                  ? <span className="faint small"> {REASON[change.outcome.reason]}</span> : null}
                <label className="checkbox small">
                  <input type="checkbox" checked={choices[change.id] !== 'local'}
                    onChange={(e) => onChoice(change.id, e.target.checked ? 'incoming' : 'local')} />
                  Use {cloud ? 'the account’s' : 'the file’s'} value
                </label>
              </li>
            ))}
          </ul>
        </section>
      )}
      {(d.added.length > 0 || d.conflicts.length > 0 || d.removed.length > 0) && (
        <section aria-label="Dictionary changes">
          <strong>Dictionary</strong>
          <p className="dialog-note">
            {d.added.length} to add, {d.unchanged} already here{d.removed.length ? `, ${d.removed.length} to remove` : ''}.
            {d.overlapping ? ` ${d.overlapping} incoming ${d.overlapping === 1 ? 'entry has' : 'entries have'} the same words as one you already have; both are kept.` : ''}
            {d.reordered ? ' Entries are put in a fixed order so every device applies overlapping rules alike.' : ''}
          </p>
          {d.removed.length > 0 && (
            <ul className="preview-list" aria-label="Entries to remove">
              {d.removed.map(e => <li key={e.id}>Remove: {describeEntry(e.entry)}</li>)}
            </ul>
          )}
          {d.conflicts.length > 0 && (
            <ul className="preview-list" aria-label="Entries that differ">
              {d.conflicts.map(c => (
                <li key={c.id}>
                  <span className="badge warn">Differs</span> Here: {describeEntry(c.local)} · {cloud ? 'Account' : 'File'}: {describeEntry(c.incoming)}
                  <span className="row">
                    <label className="checkbox small"><input type="radio" name={c.id} checked={choices[c.id] === 'local'} onChange={() => onChoice(c.id, 'local')} />Keep mine</label>
                    <label className="checkbox small"><input type="radio" name={c.id} checked={choices[c.id] === 'incoming'} onChange={() => onChoice(c.id, 'incoming')} />Use {cloud ? 'the account’s' : 'the file’s'}</label>
                  </span>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}
    </div>
  );
}

function outcomeText(receipt: Receipt, what: string): string {
  if (receipt.state !== 'applied') return notAppliedMessage(receipt.reason);
  const kept = Object.values(receipt.outcomes).filter(o => o && (o.outcome === 'preserved' || o.outcome === 'partial')).length;
  return `${what}${kept ? ` ${kept} setting${kept === 1 ? ' was' : 's were'} kept but not applied; see “Not applied on this device”.` : ''} You can undo it below.`;
}

function ImportDialog({ onClose, onDone }: { onClose: () => void; onDone: (text: string) => void }) {
  const [categories, setCategories] = useState<Category[]>(both());
  const [replace, setReplace] = useState(false);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [choices, setChoices] = useState<Record<string, Choice>>({});
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const close = () => { if (preview) void portableBridge.discard(preview.import_id).catch(() => {}); onClose(); };
  const choose = async () => {
    setBusy(true); setError('');
    try {
      const next = await portableBridge.preview(categories, replace ? 'replace_selected' : 'merge');
      if (next) { setPreview(next); setChoices({}); }
    } catch (cause) { setError(portableMessage(cause)); }
    finally { setBusy(false); }
  };
  const apply = async () => {
    if (!preview) return;
    setBusy(true); setError('');
    try {
      const receipt = await portableBridge.apply(preview.import_id, choices);
      if (receipt.state === 'applied') { onDone(outcomeText(receipt, 'Imported.')); onClose(); }
      else setError(notAppliedMessage(receipt.reason));
    } catch (cause) { setError(portableMessage(cause)); }
    finally { setBusy(false); }
  };
  const unresolved = preview?.conflict_ids.some(id => !choices[id]) ?? false;
  return (
    <Dialog title="Import settings and dictionary" onClose={close}>
      <div className="dialog-body">
        {!preview ? (
          <>
            <p className="dialog-note">Choose what to import, then pick the file. Nothing changes until you confirm.</p>
            <Categories value={categories} onChange={setCategories} />
            {categories.includes('dictionary') && (
              <label className="checkbox">
                <input type="checkbox" checked={replace} onChange={(e) => setReplace(e.target.checked)} />
                Replace my dictionary instead of adding to it (removed entries are listed before you confirm)
              </label>
            )}
            {error && <p className="notice alert" role="alert">{error}</p>}
            <div className="dialog-actions">
              <button type="button" className="button" onClick={close}>Cancel</button>
              <button type="button" className="button primary" disabled={busy || !categories.length} onClick={() => void choose()}>Choose file…</button>
            </div>
          </>
        ) : (
          <>
            <PreviewBody preview={preview} choices={choices} onChoice={(id, c) => setChoices(prev => ({ ...prev, [id]: c }))} />
            {error && <p className="notice alert" role="alert">{error}</p>}
            <div className="dialog-actions">
              <button type="button" className="button" onClick={close}>Cancel</button>
              <button type="button" className="button primary" disabled={busy || unresolved} onClick={() => void apply()}>Import</button>
            </div>
          </>
        )}
      </div>
    </Dialog>
  );
}

function SyncDialog({ onClose, onDone }: { onClose: () => void; onDone: (text: string) => void }) {
  const [categories, setCategories] = useState<Category[]>(both());
  const [disposition, setDisposition] = useState<Disposition>('restore_and_sync');
  const [preview, setPreview] = useState<SyncPreview | null>(null);
  const [choices, setChoices] = useState<Record<string, Choice>>({});
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const compare = async () => {
    setBusy(true); setError('');
    try { setPreview(await portableBridge.syncPreview(categories, disposition)); setChoices({}); }
    catch (cause) { setError(portableMessage(cause)); }
    finally { setBusy(false); }
  };
  const confirm = async () => {
    if (!preview) return;
    setBusy(true); setError('');
    try {
      const receipt = await portableBridge.syncConfirm(preview.preview.import_id, choices);
      if (receipt.state === 'applied') {
        onDone(outcomeText(receipt, disposition === 'restore_and_sync' ? 'Sync is on for this device.' : 'Restored from your account. Sync stays off.'));
        onClose();
      } else setError(notAppliedMessage(receipt.reason));
    } catch (cause) { setError(portableMessage(cause)); }
    finally { setBusy(false); }
  };
  const unresolved = preview?.preview.conflict_ids.some(id => !choices[id]) ?? false;
  return (
    <Dialog title="Set up sync" onClose={onClose}>
      <div className="dialog-body">
        {!preview ? (
          <>
            <p className="dialog-note">Recommended: settings and dictionary. This selection excludes transcripts, recordings and provider keys.</p>
            <Categories value={categories} onChange={setCategories} />
            <div role="radiogroup" aria-label="How to use your account">
              <label className="checkbox">
                <input type="radio" name="disposition" checked={disposition === 'restore_and_sync'} onChange={() => setDisposition('restore_and_sync')} />
                Restore and keep syncing — this device joins sync; its own additions are uploaded
              </label>
              <label className="checkbox">
                <input type="radio" name="disposition" checked={disposition === 'restore_once'} onChange={() => setDisposition('restore_once')} />
                Restore once — copy what is in your account to this device; nothing is uploaded and sync stays off
              </label>
            </div>
            {error && <p className="notice alert" role="alert">{error}</p>}
            <div className="dialog-actions">
              <button type="button" className="button" onClick={onClose}>Cancel</button>
              <button type="button" className="button primary" disabled={busy || !categories.length} onClick={() => void compare()}>{busy ? 'Checking your account…' : 'Compare'}</button>
            </div>
          </>
        ) : (
          <>
            <PreviewBody preview={preview.preview} cloud={preview} choices={choices} onChoice={(id, c) => setChoices(prev => ({ ...prev, [id]: c }))} />
            {error && <p className="notice alert" role="alert">{error}</p>}
            <div className="dialog-actions">
              <button type="button" className="button" onClick={onClose}>Cancel</button>
              <button type="button" className="button primary" disabled={busy || unresolved} onClick={() => void confirm()}>
                {disposition === 'restore_once' ? 'Restore once' : 'Restore and keep syncing'}
              </button>
            </div>
          </>
        )}
      </div>
    </Dialog>
  );
}
