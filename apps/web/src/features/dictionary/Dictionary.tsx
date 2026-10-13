import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { ArrowRight, BookA, Check, Copy, Ear, Info, Pencil, Plus, Power, PowerOff, Quote, Search, Trash2, Undo2, Wand2 } from 'lucide-react';
import type { VocabularyDocument } from '@metrix-writer/contracts';
import { StaleVocabulary, vocabularyApi, type VocabularyApi, type Versioned } from '../vocabulary/api';
import { useEnumParam } from '../shell/urlState';
import { copyText } from '../shell/clipboard';
import { openMenu } from '../shell/menu';
import { Dialog } from '../shell/Dialog';
import { Problem } from '../shell/Problem';
import { Mark } from '../shell/Mark';
import { modKey } from '../shell/TopBar';
import { Toggle } from '../settings/controls';
import { Snippets } from './Snippets';
import { isTauri, listen } from '../desktop-session/runtime/native';
import { PRODUCT_NAME } from '../../product';
import { ACTIVE_WORDS, LOCAL_WORD_BYTES, bytes, entryProblem, matches, type Subtab } from './rules';
import { adoptSavedIds, applyUnsavedChanges, describeEntry, unsavedChanges } from './changes';
import './dictionary.css';

const SUBTABS = ['words', 'corrections', 'snippets'] as const;

const MAX_ENTRIES = 200;
/** Typing settles before it is sent. Adding, deleting and switching never wait for it. */
const TYPING_SETTLE_MS = 500;
/** How long a deleted entry can be brought back. */
const UNDO_MS = 5000;

const KIND: Record<Subtab, string> = { words: 'word', corrections: 'correction', snippets: 'snippet' };
const LIST: Record<Subtab, 'hints' | 'corrections' | 'snippets'> = { words: 'hints', corrections: 'corrections', snippets: 'snippets' };

/*
 * Three genuinely different behaviours over one contract. The labels must keep
 * them apart: a hint that reads like a guaranteed replacement is a bug in the
 * copy, not a nicety.
 */
const EXPLAINER: Record<Subtab, ReactNode> = {
  words: (
    <>
      <strong>Words help the speech model listen for names and jargon.</strong> They are a hint, not a guarantee — if a
      word still comes out wrong, add a correction.
    </>
  ),
  corrections: (
    <>
      <strong>Corrections replace exactly what was heard with what you meant</strong>, after transcription. Matching is
      exact and case-sensitive, and what you said is always kept alongside.
    </>
  ),
  snippets: (
    <>
      <strong>Say a short phrase, and your exact text is inserted instead.</strong> Capitals and extra spaces do not
      matter. Snippets come before corrections; the longest phrase wins, then the first listed rule.
    </>
  ),
};

/** One entry of the open section, flattened so words and corrections draw from one shape. */
interface Entry {
  id: string;
  index: number;
  values: string[];
  fieldLabels: string[];
  /** Whether dictation actually applies this entry. A switched-off one is kept. */
  enabled: boolean;
  set(at: number, next: string): VocabularyDocument;
  toggle(): VocabularyDocument;
  remove(): VocabularyDocument;
}

type Ids = Record<'hints' | 'corrections' | 'snippets', string[]>;
let nextId = 0;
const freshIds = (value: VocabularyDocument): Ids => ({
  hints: value.hints.map(() => `e${++nextId}`),
  corrections: value.corrections.map(() => `e${++nextId}`),
  snippets: value.snippets.map(() => `e${++nextId}`),
});

interface RetainedDraft { base: VocabularyDocument; draft: VocabularyDocument }
// Keep a refused draft when the user visits another section in this window.
const retainedDrafts = new WeakMap<VocabularyApi, RetainedDraft>();

const replace = <T,>(list: T[], index: number, next: T) => list.map((item, i) => (i === index ? next : item));

/**
 * Words, corrections and snippets: what dictation listens for, fixes and
 * expands. Every change is saved as it is made; a deleted entry can be brought
 * back for a few seconds; the list is one search away.
 */
export function Dictionary({ api = vocabularyApi, onSaved }: { api?: VocabularyApi; onSaved?: (value: VocabularyDocument) => void }) {
  const [tab, setTab] = useEnumParam<Subtab>('sub', SUBTABS, 'words');
  const [value, setValue] = useState<VocabularyDocument | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [settled, setSettled] = useState(false);
  const [error, setError] = useState('');
  /** A save was refused because the stored dictionary changed meanwhile. */
  const [retained, setRetained] = useState<RetainedDraft | null>(() => retainedDrafts.get(api) ?? null);
  const [reviewing, setReviewing] = useState(false);
  const [choices, setChoices] = useState<Record<string, string>>({});
  const [reload, setReload] = useState(0);
  const [draft, setDraft] = useState({ a: '', b: '' });
  /** The add dialog, open for the section currently shown. */
  const [adding, setAdding] = useState(false);
  /** The entry currently unlocked for typing. A saved entry is read until asked. */
  const [editing, setEditing] = useState('');
  const [snippetEditing, setSnippetEditing] = useState(false);
  const editorOpen = useRef(false);
  editorOpen.current = !!editing || adding || snippetEditing || reviewing;
  const [query, setQuery] = useState('');
  /** The last deletion, while it can still be undone. */
  const [undo, setUndo] = useState<{ id: string; tab: Subtab; index: number; label: string; restore: (current: VocabularyDocument) => VocabularyDocument } | null>(null);
  const search = useRef<HTMLInputElement>(null);
  /*
   * These are UI row identities, separate from the stored sync IDs,
   * kept beside each list, so a deleted, restored or edited row is the same row
   * to React rather than whichever row happens to sit at its old position.
   */
  const ids = useRef<Ids>({ hints: [], corrections: [], snippets: [] });

  /*
   * Entries are saved as they are made. This section is unmounted whenever the
   * window moves to another tab, so anything still held only in component
   * state at that moment is gone; `pending` is what has not been confirmed
   * stored yet, and it survives a failed save so retrying resends exactly what
   * was typed rather than a reconstruction of it.
   */
  const pending = useRef<VocabularyDocument | null>(null);
  const base = useRef<VocabularyDocument | null>(null);
  const currentApi = useRef(api);
  currentApi.current = api;
  function retain(next: RetainedDraft | null) {
    if (next) retainedDrafts.set(currentApi.current, next);
    else retainedDrafts.delete(currentApi.current);
    setRetained(next);
  }
  const writing = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  /** The stored version this copy was loaded or last saved as; saves are conditional on it. */
  const version = useRef<string | null>(null);
  const save = useRef(api.save);
  save.current = api.save;
  const saveIf = useRef(api.saveIf);
  saveIf.current = api.saveIf;
  const notify = useRef(onSaved);
  notify.current = onSaved;

  const flush = useCallback(async () => {
    clearTimeout(timer.current);
    if (writing.current) return;
    writing.current = true;
    setSaving(true);
    try {
      while (pending.current) {
        const target = pending.current;
        const sentRows: Ids = { hints: [...ids.current.hints], corrections: [...ids.current.corrections], snippets: [...ids.current.snippets] };
        try {
          const stored: Versioned = saveIf.current
            ? await saveIf.current(target, version.current)
            : { document: await save.current(target), version: null };
          version.current = stored.version;
          // Anything typed while this request was in flight is saved next.
          if (pending.current === target) {
            pending.current = null;
            setValue(stored.document);
          } else if (pending.current) {
            // Do not put an older response over typing/deletions made in flight.
            pending.current = adoptSavedIds(pending.current, ids.current, sentRows, stored.document);
            setValue(pending.current);
          }
          base.current = stored.document;
          retain(null);
          setError('');
          setSettled(true);
          notify.current?.(stored.document);
        } catch (cause) {
          if (cause instanceof StaleVocabulary) {
            // Never write this copy over a newer one: show what is stored now.
            retain({ base: base.current!, draft: pending.current ?? target });
            pending.current = null;
            setError('');
            setLoading(true);
            setEditing(''); setAdding(false);
            setReload((n) => n + 1);
            return;
          }
          setError(cause instanceof Error ? cause.message : 'Could not save. Your edits are still here; try again.');
          return;
        }
      }
    } finally {
      writing.current = false;
      setSaving(false);
    }
  }, []);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setLoadFailed(false);
    (api.loadVersioned ? api.loadVersioned() : api.load().then((document): Versioned => ({ document, version: null })))
      .then((loaded) => {
        if (!active) return;
        ids.current = freshIds(loaded.document);
        base.current = loaded.document;
        setValue(loaded.document); version.current = loaded.version; pending.current = null; setLoading(false); setError(''); setSettled(false); setUndo(null);
      })
      .catch(() => { if (active) { setLoadFailed(true); setLoading(false); } });
    return () => { active = false; };
  }, [api, reload]);

  // An import or a synced change rewrote the stored dictionary. Reload it,
  // unless an edit here is still waiting to be saved: that save must not be
  // replaced by what was read before it.
  useEffect(() => {
    if (!isTauri()) return;
    let stop: (() => void) | undefined;
    let disposed = false;
    void Promise.resolve()
      .then(() => listen<{ kind: string }>('writer-sync', (event) => {
        if (event?.kind === 'applied' && !pending.current && !writing.current && !editorOpen.current) setReload((n) => n + 1);
      }))
      .then((unlisten) => { if (disposed) unlisten(); else stop = unlisten; })
      .catch(() => { /* Older hosts publish no sync events. */ });
    return () => { disposed = true; stop?.(); };
  }, []);

  // Leaving the section must not discard a debounced edit.
  useEffect(() => () => { clearTimeout(timer.current); if (pending.current) void flush(); }, [flush]);

  useEffect(() => {
    if (!undo) return;
    const timeout = setTimeout(() => setUndo((current) => current === undo ? null : current), UNDO_MS);
    return () => clearTimeout(timeout);
  }, [undo]);

  /** An edit that is finished the moment it is made: adding, switching, deleting. */
  function commit(next: VocabularyDocument) {
    setValue(next);
    pending.current = next;
    setSettled(false);
    void flush();
  }

  /** An edit still being typed. Sent once it stops changing, or on blur. */
  function edit(next: VocabularyDocument) {
    setValue(next);
    pending.current = next;
    setSettled(false);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => { void flush(); }, TYPING_SETTLE_MS);
  }

  const status = loading ? 'Loading…' : saving ? 'Saving…' : error || retained ? 'Not saved' : settled ? 'Saved' : '';

  const counts = {
    words: value?.hints.length ?? 0,
    corrections: value?.corrections.length ?? 0,
    snippets: value?.snippets.length ?? 0,
  };
  const activeWords = value?.hints.filter((hint) => hint.enabled) ?? [];
  const activeBytes = activeWords.reduce((sum, hint) => sum + bytes(hint.text), 0);
  const full = counts[tab] >= MAX_ENTRIES;
  const kind = KIND[tab];
  const headings = tab === 'words' ? ['Word or phrase'] : tab === 'corrections' ? ['When I hear', 'Write'] : ['When I say', 'Insert exactly'];
  const needle = query.trim();
  const found = value ? {
    words: value.hints.filter((hint) => matches(needle, hint.text)).length,
    corrections: value.corrections.filter((rule) => matches(needle, rule.from, rule.to)).length,
    snippets: value.snippets.filter((rule) => matches(needle, rule.phrase, rule.text)).length,
  } : counts;

  const entries = useMemo<Entry[]>(() => {
    if (!value || tab === 'snippets') return [];
    const list = LIST[tab];
    if (ids.current[list].length !== value[list].length) ids.current[list] = value[list].map(() => `e${++nextId}`);
    if (tab === 'words') {
      return value.hints.map((hint, index): Entry => ({
        id: ids.current.hints[index],
        index,
        values: [hint.text],
        fieldLabels: [`Word ${index + 1}`],
        enabled: hint.enabled,
        set: (_at, next) => ({ ...value, hints: replace(value.hints, index, { ...hint, text: next }) }),
        toggle: () => ({ ...value, hints: replace(value.hints, index, { ...hint, enabled: !hint.enabled }) }),
        remove: () => ({ ...value, hints: value.hints.filter((_, i) => i !== index) }),
      })).filter((entry) => matches(needle, ...entry.values));
    }
    return value.corrections.map((rule, index): Entry => ({
      id: ids.current.corrections[index],
      index,
      values: [rule.from, rule.to],
      fieldLabels: [`Correction ${index + 1} heard as`, `Correction ${index + 1} replace with`],
      enabled: rule.enabled,
      set: (at, next) => ({
        ...value,
        corrections: replace(value.corrections, index, at === 0 ? { ...rule, from: next } : { ...rule, to: next }),
      }),
      toggle: () => ({ ...value, corrections: replace(value.corrections, index, { ...rule, enabled: !rule.enabled }) }),
      remove: () => ({ ...value, corrections: value.corrections.filter((_, i) => i !== index) }),
    })).filter((entry) => matches(needle, ...entry.values));
  }, [value, tab, needle]);

  const problem = value && adding ? entryProblem(tab, value, draft.a, draft.b) : null;

  function addEntry() {
    if (!value || full || problem) return;
    const a = draft.a.trim();
    const b = tab === 'snippets' ? draft.b : draft.b.trim();
    const list = LIST[tab];
    ids.current[list] = [...ids.current[list], `e${++nextId}`];
    if (tab === 'words') commit({ ...value, hints: [...value.hints, { text: a, enabled: true }] });
    else if (tab === 'corrections') commit({ ...value, corrections: [...value.corrections, { from: a, to: b, enabled: true }] });
    else commit({ ...value, snippets: [...value.snippets, { phrase: a, text: b, enabled: true }] });
    setDraft({ a: '', b: '' });
    setAdding(false);
  }

  /** Deletes at once; the entry can be put back where it was for a few seconds. */
  function removeAt(section: Subtab, index: number) {
    if (!value) return;
    const list = LIST[section];
    const id = ids.current[list][index];
    if (editing === id) setEditing('');
    const item = value[list][index];
    const label = section === 'words' ? value.hints[index].text : section === 'corrections' ? `${value.corrections[index].from} → ${value.corrections[index].to}` : value.snippets[index].phrase;
    ids.current[list] = ids.current[list].filter((_, i) => i !== index);
    commit({ ...value, [list]: value[list].filter((_, i) => i !== index) });
    setUndo({
      id,
      tab: section,
      index,
      label,
      restore: (current) => ({ ...current, [list]: [...current[list].slice(0, index), item, ...current[list].slice(index)] }),
    });
  }

  const restore = useCallback(() => {
    if (!undo || !value) return;
    const list = LIST[undo.tab];
    ids.current[list] = [...ids.current[list].slice(0, undo.index), undo.id, ...ids.current[list].slice(undo.index)];
    commit(undo.restore(value));
    setTab(undo.tab);
    setUndo(null);
  }, [undo, value]);

  const openAdd = useCallback(() => { if (value && !full && !retained && !loading && !loadFailed) { setDraft({ a: '', b: '' }); setAdding(true); } }, [value, full, retained, loading, loadFailed]);

  // N adds, / searches, Ctrl/⌘ Z brings back the last deletion.
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target?.closest('input, textarea, select, [contenteditable="true"]') || document.querySelector('[aria-modal="true"]')) return;
      if ((event.ctrlKey || event.metaKey) && !event.shiftKey && event.key.toLowerCase() === 'z' && undo) { event.preventDefault(); restore(); return; }
      if (event.ctrlKey || event.metaKey || event.altKey) return;
      if (event.key === '/') { event.preventDefault(); search.current?.focus(); }
      else if (event.key.toLowerCase() === 'n') { event.preventDefault(); openAdd(); }
    };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, [undo, restore, openAdd]);

  const changes = retained && value ? unsavedChanges(retained.base, retained.draft, value) : [];
  const wordsTab = tab === 'words';

  function cell(entry: Entry, at: number, open: boolean, invalid: boolean) {
    const text = entry.values[at];
    return (
      <div className="entry-cell">
        {!wordsTab && <small className="side-label">{headings[at]}{at === 1 && !entry.enabled ? ' · off' : ''}</small>}
        {open ? (
          at === 0 ? (
            <input
              type="text"
              dir="auto"
              autoFocus
              aria-label={entry.fieldLabels[at]}
              aria-invalid={invalid}
              value={text}
              onBlur={() => void flush()}
              onKeyDown={(event) => { if (event.key === 'Escape' || event.key === 'Enter') { setEditing(''); void flush(); } }}
              onChange={(event) => edit(entry.set(at, event.target.value))}
            />
          ) : (
            <textarea
              dir="auto"
              rows={1}
              aria-label={entry.fieldLabels[at]}
              value={text}
              onBlur={() => void flush()}
              onKeyDown={(event) => { if (event.key === 'Escape') { setEditing(''); void flush(); } }}
              onChange={(event) => edit(entry.set(at, event.target.value))}
            />
          )
        ) : (
          // A Hebrew word inside an English list keeps its own direction
          // without pulling the chip's layout along with it.
          <span className="entry-value" dir="auto"><bdi dir="auto">{text || <i className="muted">empty</i>}</bdi></span>
        )}
      </div>
    );
  }
  const elsewhere = needle && !found[tab] ? SUBTABS.filter((id) => id !== tab && found[id] > 0) : [];

  return (
    <div className="page dictionary-page">
      {reviewing && retained && value && (
        <Dialog title="Review unsaved dictionary changes" onClose={() => setReviewing(false)}>
          <div className="dialog-body">
            <p>Only the entries below will change. Other saved entries are kept.</p>
            {!changes.length && <p>Your changes are already present in the saved dictionary.</p>}
            {changes.map((change) => <div className="field" key={change.key}>
              <strong dir="auto">{change.label}</strong>
              <p dir="auto">Saved: {describeEntry(change.current)}</p>
              <p dir="auto">Your change: {describeEntry(change.desired)}</p>
              {change.conflict && <label>Both copies changed this entry.
                <select aria-label={`Resolve ${change.label}`} value={choices[change.key] ?? ''} onChange={(event) => setChoices((old) => ({ ...old, [change.key]: event.target.value }))}>
                  <option value="">Choose which version to keep</option>
                  <option value="current">Keep saved entry</option>
                  <option value="mine">Use my change</option>
                </select>
              </label>}
            </div>)}
            <div className="dialog-actions">
              <button type="button" className="button" onClick={() => setReviewing(false)}>Cancel</button>
              <button type="button" className="button primary" disabled={loading || loadFailed || saving || changes.some((change) => change.conflict && !choices[change.key])} onClick={() => {
                const next = applyUnsavedChanges(value, changes, choices);
                // Retain the old draft until the conditional write succeeds.
                setReviewing(false);
                if (!changes.length) retain(null);
                else { ids.current = freshIds(next); commit(next); }
              }}>Apply reviewed changes</button>
            </div>
          </div>
        </Dialog>
      )}
      <div className="scroller dictionary-content">
        <div className="dictionary-inner">
          <header className="dictionary-head">
            <div>
              <h1><Mark size={22} /> Dictionary</h1>
              <p>Teach {PRODUCT_NAME} your names, fix what it mishears, and say short phrases that expand into text.</p>
            </div>
            <span className="spacer" />
            <span className="dictionary-status small" role="status">{status}</span>
            {value && (
              <>
                <label className="search-field dictionary-search">
                  <Search size={16} aria-hidden />
                  <span className="visually-hidden">Search dictionary</span>
                  <input ref={search} type="search" placeholder="Search all rules" aria-keyshortcuts="/" value={query} onChange={(event) => setQuery(event.target.value)} />
                  <span className="kbd" aria-hidden><kbd>/</kbd></span>
                </label>
                <button
                  type="button"
                  className="button primary"
                  disabled={full || !!retained || loading || loadFailed}
                  aria-keyshortcuts="N"
                  title={full ? `The list is at its ${MAX_ENTRIES}-entry limit. Delete an entry to add another.` : undefined}
                  onClick={openAdd}
                >
                  <Plus size={15} aria-hidden /> Add {kind}
                </button>
              </>
            )}
          </header>

          <div className="dictionary-tabs" role="tablist" aria-label="Dictionary sections">
            {SUBTABS.map((id) => {
              const Icon = id === 'words' ? Ear : id === 'corrections' ? Wand2 : Quote;
              const off = id === 'words' && value ? counts.words - activeWords.length : 0;
              return (
                <button key={id} type="button" role="tab" aria-selected={tab === id} className="dictionary-tab" onClick={() => { setTab(id); setEditing(''); }}>
                  <Icon size={16} aria-hidden />
                  {id === 'words' ? 'Words' : id === 'corrections' ? 'Corrections' : 'Snippets'}
                  <span className="count">{needle ? found[id] : id === 'words' && value ? activeWords.length : counts[id]}</span>
                  {!needle && off > 0 && <span className="tab-extra">{off} off</span>}
                </button>
              );
            })}
          </div>

          {/* A list that could not load must not be drawn as an empty list. Until
              it loads, the failure is the whole view; the detail says why you
              should not start editing on top of an unread list. */}
          {(!value || loadFailed) && (
            loadFailed
              ? <Problem
                  title="Saved vocabulary could not be loaded"
                  detail="Editing now would save a list that was never read, replacing what is stored."
                  keeps="Nothing has been changed or deleted. Your transcripts still use the entries already saved."
                  onRetry={() => setReload((n) => n + 1)}
                />
              : <p className="notice" role="status">Loading…</p>
          )}

          {retained && value && (
            <p className="notice alert" role="alert">
              The saved dictionary changed while you were editing. Your last change was not saved, and your unsaved changes are kept for review.
              {loading || loadFailed ? 'The current version has not loaded yet.' : 'The current version is shown.'}{' '}
              <button type="button" className="button small" disabled={loading || loadFailed} onClick={() => { setChoices({}); setReviewing(true); }}>Review unsaved changes</button>{' '}
              <button type="button" className="button small" onClick={() => retain(null)}>Discard unsaved changes</button>
            </p>
          )}
          {error && value && (
            <p className="notice alert" role="alert">
              {error}{' '}
              <button type="button" className="button small" disabled={saving} onClick={() => void flush()}>Retry saving</button>
            </p>
          )}

          {value && tab !== 'snippets' && (
            <div className="explain">
              <span className="explain-icon" aria-hidden>{wordsTab ? <Ear size={18} /> : <Wand2 size={18} />}</span>
              <p className="dictionary-explainer">{EXPLAINER[tab]}</p>
              {wordsTab && <Capacity active={activeWords.length} bytes={activeBytes} />}
            </div>
          )}

          {elsewhere.length > 0 && (
            <p className="notice">
              Nothing matches “{needle}” here.{' '}
              {elsewhere.map((id) => (
                <button key={id} type="button" className="row-link" onClick={() => setTab(id)}>{found[id]} in {id === 'words' ? 'Words' : id === 'corrections' ? 'Corrections' : 'Snippets'}</button>
              ))}
            </p>
          )}

          {value && tab === 'snippets' && (
            <Snippets
              value={value}
              query={needle}
              ids={ids.current.snippets}
              disabled={!!retained || loading || loadFailed}
              onEditingChange={setSnippetEditing}
              onChange={commit}
              onAdd={openAdd}
              onDelete={(index) => removeAt('snippets', index)}
              explainer={EXPLAINER.snippets}
            />
          )}

          {value && tab !== 'snippets' && (
            <section className={`entry-list ${wordsTab ? 'tokens' : 'rules'}`} aria-label={wordsTab ? 'Words' : 'Corrections'}>
              {!entries.length && !needle && (
                <div className="empty">
                  <BookA size={26} aria-hidden />
                  <strong>Nothing here yet</strong>
                  <p>
                    {wordsTab
                      ? 'Add a name or a piece of jargon and dictation will start listening for it.'
                      : 'Add what you keep hearing back and what it should say instead. It applies to transcripts made after you add it.'}
                  </p>
                  <button type="button" className="button primary" onClick={openAdd}>
                    <Plus size={13} aria-hidden /> Add your first {kind}
                  </button>
                </div>
              )}

              {entries.map((entry) => {
                const open = editing === entry.id;
                const number = entry.index + 1;
                const others = value && (wordsTab ? entryProblem('words', { ...value, hints: value.hints.filter((_, i) => i !== entry.index) }, entry.values[0], '')
                  : entryProblem('corrections', { ...value, corrections: value.corrections.filter((_, i) => i !== entry.index) }, entry.values[0], entry.values[1]));
                return (
                  <div
                    className={`entry-row ${wordsTab ? 'token single' : 'rule'} ${open ? 'editing' : ''} ${entry.enabled ? '' : 'off'}`}
                    key={entry.id}
                    onContextMenu={(event) => {
                      if (retained || loading || loadFailed) return;
                      // Inside one of this row's fields the four editing commands
                      // are what the menu is for; the document handler draws them.
                      if (event.target instanceof HTMLElement && event.target.closest('input, textarea')) return;
                      openMenu(event, [
                        { label: open ? 'Finish editing' : 'Edit', icon: open ? Check : Pencil, run: () => setEditing(open ? '' : entry.id) },
                        { label: entry.enabled ? 'Turn off' : 'Turn on', icon: entry.enabled ? PowerOff : Power, run: () => commit(entry.toggle()) },
                        { label: 'Copy', icon: Copy, disabled: !entry.values.join(''), run: () => void copyText(entry.values.filter(Boolean).join(' → ')) },
                        { kind: 'separator' },
                        { label: 'Delete', icon: Trash2, danger: true, run: () => removeAt(tab, entry.index) },
                      ]);
                    }}
                  >
                    {/* Switched off is not deleted. A rule you want back next week
                        stays where it is, in the order its precedence depends on,
                        and stops being applied. */}
                    {!wordsTab && <Toggle label={`Use ${kind} ${number}`} disabled={!!retained || loading || loadFailed} checked={entry.enabled} onChange={() => commit(entry.toggle())} />}
                    <div className={wordsTab ? 'token-body' : 'pair'}>
                      {cell(entry, 0, open, !!others)}
                      {!wordsTab && <span className="entry-arrow" aria-hidden><ArrowRight size={18} /></span>}
                      {!wordsTab && cell(entry, 1, open, false)}
                      {open && others && <p className="entry-problem" role="status">{others}</p>}
                    </div>
                    <span className="row-actions">
                      {wordsTab && <Toggle label={`Use ${kind} ${number}`} disabled={!!retained || loading || loadFailed} checked={entry.enabled} onChange={() => commit(entry.toggle())} />}
                      {/* Saved entries are read until you say otherwise, so a stray
                          click in a list of two hundred rules cannot rewrite one. */}
                      <span className={wordsTab ? 'token-tools' : 'rule-tools'}>
                        <button
                          type="button"
                          className="icon-button"
                          disabled={!!retained || loading || loadFailed}
                          aria-label={`${open ? 'Finish editing' : 'Edit'} ${kind} ${number}`}
                          title={open ? 'Finish editing' : 'Edit'}
                          onClick={() => { setEditing(open ? '' : entry.id); if (open) void flush(); }}
                        >
                          {open ? <Check size={15} aria-hidden /> : <Pencil size={15} aria-hidden />}
                        </button>
                        <button
                          type="button"
                          className="icon-button danger"
                          disabled={!!retained || loading || loadFailed}
                          aria-label={`Delete ${kind} ${number}`}
                          title="Delete"
                          onClick={() => removeAt(tab, entry.index)}
                        >
                          <Trash2 size={15} aria-hidden />
                        </button>
                      </span>
                    </span>
                  </div>
                );
              })}
              {wordsTab && !!entries.length && (
                <button type="button" className="token add-token" aria-label="Add another word" disabled={full || !!retained || loading || loadFailed} onClick={openAdd}><Plus size={16} aria-hidden /> Add word</button>
              )}
              {needle && !entries.length && !elsewhere.length && <p className="statistics-note">Nothing matches “{needle}”.</p>}
            </section>
          )}

          {value && (
            <p className="dictionary-keys">
              <Info size={14} aria-hidden /> Changes apply to your next dictation. Press <span className="kbd"><kbd>N</kbd></span> to add,
              {' '}<span className="kbd"><kbd>/</kbd></span> to search, <span className="kbd"><kbd>{modKey()}</kbd><kbd>Z</kbd></span> to undo a delete.
            </p>
          )}
        </div>
      </div>

      {undo && (
        <div className="toast dictionary-toast" role="status">
          <Trash2 size={16} aria-hidden />
          <span>Deleted “<bdi dir="auto">{undo.label}</bdi>”</span>
          <button type="button" className="toast-action" onClick={restore}><Undo2 size={14} aria-hidden /> Undo</button>
        </div>
      )}

      {adding && value && (
        <Dialog title={`Add a ${kind}`} onClose={() => setAdding(false)}>
          <form className="dialog-body" onSubmit={(event) => { event.preventDefault(); addEntry(); }}>
            <label className="field">
              <span>{headings[0]}</span>
              <input
                type="text"
                dir="auto"
                aria-label={headings[0]}
                aria-invalid={!!problem && !!draft.a.trim()}
                placeholder={wordsTab ? 'A name or term, e.g. Kubernetes' : tab === 'corrections' ? 'tail scale' : 'my address'}
                value={draft.a}
                onChange={(event) => setDraft({ ...draft, a: event.target.value })}
              />
            </label>
            {!wordsTab && (
              <label className="field">
                <span>{headings[1]}</span>
                {tab === 'snippets'
                  ? <textarea
                      dir="auto"
                      rows={3}
                      aria-label={headings[1]}
                      placeholder="12 Rothschild Blvd, Tel Aviv-Yafo"
                      value={draft.b}
                      onChange={(event) => setDraft({ ...draft, b: event.target.value })}
                    />
                  : <input
                      type="text"
                      dir="auto"
                      aria-label={headings[1]}
                      placeholder="Tailscale"
                      value={draft.b}
                      onChange={(event) => setDraft({ ...draft, b: event.target.value })}
                    />}
              </label>
            )}
            {/* Blank is simply not ready yet; a duplicate or an unusable word is
                said as it is typed, not after Add. */}
            {problem && (draft.a.trim() || draft.b.trim()) && <p className="entry-problem" role="status">{problem}</p>}
            <p className="dialog-note">{EXPLAINER[tab]}</p>
            <div className="dialog-actions">
              <button type="button" className="button" onClick={() => setAdding(false)}>Cancel</button>
              <button type="submit" className="button primary" disabled={!!problem}>
                <Plus size={13} aria-hidden /> Add
              </button>
            </div>
          </form>
        </Dialog>
      )}
    </div>
  );
}

/** "11 of 128 active", warning before transcription would refuse the list. */
function Capacity({ active, bytes: size }: { active: number; bytes: number }) {
  const over = active > ACTIVE_WORDS;
  const near = !over && active >= ACTIVE_WORDS - 12;
  const heavy = size > LOCAL_WORD_BYTES;
  return (
    <div className={`capacity${over ? ' over' : near || heavy ? ' near' : ''}`}>
      <span className="capacity-figure">{active} of {ACTIVE_WORDS} active</span>
      <span className="progress" aria-hidden><i style={{ width: `${Math.min(100, (active / ACTIVE_WORDS) * 100)}%` }} /></span>
      {over && <span className="capacity-note" role="alert">Over {ACTIVE_WORDS} active words, every transcription fails. Turn some off.</span>}
      {!over && heavy && <span className="capacity-note" role="status">Over {LOCAL_WORD_BYTES.toLocaleString()} bytes of words: local models refuse the list.</span>}
    </div>
  );
}
