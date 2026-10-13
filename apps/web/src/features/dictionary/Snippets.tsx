import { currentMotion } from '../shell/motion';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { ArrowRight, BookA, Copy, Pencil, Play, PlayCircle, Power, PowerOff, Quote, Trash2 } from 'lucide-react';
import type { VocabularyDocument } from '@metrix-writer/contracts';
import { Dialog } from '../shell/Dialog';
import { Toggle } from '../settings/controls';
import { openMenu } from '../shell/menu';
import { copyText } from '../shell/clipboard';
import { entryProblem, matches } from './rules';

type Snippet = VocabularyDocument['snippets'][number];

/**
 * Snippets read as what they do: "When I say ‘my email’ → Insert exactly …".
 * Editing is a dialog with an explicit Save, because the inserted text can be
 * several lines long; a preview plays the replacement in place.
 */
export function Snippets({ value, query, ids, onChange, onAdd, onDelete, explainer, disabled = false, onEditingChange }: {
  value: VocabularyDocument;
  query: string;
  /** Stable row ids, one per snippet, kept by the Dictionary. */
  ids: string[];
  onChange: (next: VocabularyDocument) => void;
  onAdd: () => void;
  onDelete: (index: number) => void;
  explainer: ReactNode;
  disabled?: boolean;
  onEditingChange?: (editing: boolean) => void;
}) {
  const [video, setVideo] = useState(false);
  const [help, setHelp] = useState(false);
  const [editing, setEditing] = useState<{ index: number; phrase: string; text: string } | null>(null);
  useEffect(() => { onEditingChange?.(!!editing); return () => onEditingChange?.(false); }, [!!editing, onEditingChange]);
  const shown = value.snippets.map((snippet, index) => ({ snippet, index })).filter(({ snippet }) => matches(query, snippet.phrase, snippet.text));
  const edit = (index: number) => setEditing({ index, ...value.snippets[index] });
  const toggle = (index: number) => onChange({ ...value, snippets: value.snippets.map((s, i) => i === index ? { ...s, enabled: !s.enabled } : s) });
  const problem = editing && entryProblem('snippets', { ...value, snippets: value.snippets.filter((_, i) => i !== editing.index) }, editing.phrase, editing.text);
  return <>
    <div className="explain">
      <span className="explain-icon" aria-hidden><Quote size={18} /></span>
      <p className="dictionary-explainer">{explainer} <button type="button" className="row-link" onClick={() => setHelp(true)}>How matching works</button></p>
      <button type="button" className="button small ghost" onClick={() => setVideo(true)}><PlayCircle size={15} aria-hidden /> See how snippets work</button>
    </div>
    <section className="entry-list rules" aria-label="Saved snippets">
      {shown.map(({ snippet, index }) => (
        <SnippetRow disabled={disabled} key={ids[index] ?? index} snippet={snippet} index={index} onEdit={() => edit(index)} onToggle={() => toggle(index)} onDelete={() => onDelete(index)} />
      ))}
      {!value.snippets.length && (
        <div className="empty">
          <BookA size={24} aria-hidden />
          <strong>Your shortcuts start here</strong>
          <p>Save a phrase for an address, a signature or a message you use often.</p>
          <button type="button" className="button primary" onClick={onAdd}>Add your first snippet</button>
        </div>
      )}
      {!!value.snippets.length && !shown.length && <p className="statistics-note">No snippet matches “{query}”.</p>}
    </section>
    {editing && (
      <Dialog title="Edit snippet" onClose={() => setEditing(null)}>
        <form className="dialog-body" onSubmit={event => {
          event.preventDefault();
          if (problem) return;
          onChange({ ...value, snippets: value.snippets.map((s, i) => i === editing.index ? { ...s, phrase: editing.phrase.trim(), text: editing.text } : s) });
          setEditing(null);
        }}>
          <label className="field"><span>When I say</span><input dir="auto" aria-label={`Snippet ${editing.index + 1} spoken phrase`} value={editing.phrase} onChange={e => setEditing({ ...editing, phrase: e.target.value })} /></label>
          <label className="field"><span>Insert exactly</span><textarea dir="auto" rows={5} aria-label={`Snippet ${editing.index + 1} inserts exactly`} value={editing.text} onChange={e => setEditing({ ...editing, text: e.target.value })} /></label>
          {problem && <p className="entry-problem" role="status">{problem}</p>}
          <div className="dialog-actions"><button type="button" className="button" onClick={() => setEditing(null)}>Cancel</button><button className="button primary" type="submit" disabled={!!problem}>Save</button></div>
        </form>
      </Dialog>
    )}
    {help && <Dialog title="Snippet matching rules" onClose={() => setHelp(false)}><p className="dialog-note snippet-matching-help">Capitalization and extra spaces do not matter. Snippets take precedence over corrections. The longest matching phrase wins, then the first listed rule. Switching a snippet off keeps it saved for later.</p></Dialog>}
    {video && <SnippetVideo onClose={() => setVideo(false)} />}
  </>;
}

function SnippetRow({ snippet, index, onEdit, onToggle, onDelete, disabled }: { disabled: boolean; snippet: Snippet; index: number; onEdit: () => void; onToggle: () => void; onDelete: () => void }) {
  const [phase, setPhase] = useState<'idle' | 'out' | 'result'>('idle');
  const [replay, setReplay] = useState(0);
  useEffect(() => {
    if (phase === 'idle') return;
    const timer = setTimeout(() => setPhase(phase === 'out' ? 'result' : 'idle'), phase === 'out' ? 220 : 2200);
    return () => clearTimeout(timer);
  }, [phase, replay]);
  function preview() { setReplay(n => n + 1); setPhase(currentMotion() === 'reduced' ? 'result' : 'out'); }
  return (
    <div className={`entry-row rule compact-snippet-row ${snippet.enabled ? '' : 'off'}`} onContextMenu={event => !disabled && openMenu(event, [
      { label: 'Edit', icon: Pencil, run: onEdit }, { label: 'Preview', icon: Play, run: preview },
      { label: snippet.enabled ? 'Turn off' : 'Turn on', icon: snippet.enabled ? PowerOff : Power, run: onToggle },
      { label: 'Copy', icon: Copy, run: () => void copyText(snippet.text) },
      { kind: 'separator' }, { label: 'Delete', icon: Trash2, danger: true, run: onDelete },
    ])}>
      <Toggle disabled={disabled} label={`Use snippet ${index + 1}`} checked={snippet.enabled} onChange={onToggle} />
      <div className="pair">
        <div className="entry-cell snippet-phrase-cell" data-phase={phase}>
          <small className="side-label">When I say</small>
          <span className="entry-value" dir="auto"><bdi dir="auto">{phase === 'result' ? snippet.text : `“${snippet.phrase}”`}</bdi></span>
        </div>
        <span className="entry-arrow" aria-hidden><ArrowRight size={18} /></span>
        <div className="entry-cell">
          <small className="side-label">Insert exactly{snippet.enabled ? '' : ' · off'}</small>
          <span className="entry-value snippet-text" dir="auto" title={snippet.text}>{snippet.text}</span>
        </div>
      </div>
      <span className="row-actions">
        <button type="button" className="icon-button" aria-label={`Preview snippet ${index + 1}`} title="Preview the replacement" onClick={preview}><Play size={15} aria-hidden /></button>
        <button type="button" className="icon-button" disabled={disabled} aria-label={`Edit snippet ${index + 1}`} title="Edit snippet" onClick={onEdit}><Pencil size={15} aria-hidden /></button>
        <button type="button" className="icon-button danger" disabled={disabled} aria-label={`Delete snippet ${index + 1}`} title="Delete snippet" onClick={onDelete}><Trash2 size={15} aria-hidden /></button>
      </span>
      <span className="visually-hidden" role="status">{phase === 'result' ? `Preview: ${snippet.text}` : ''}</span>
    </div>
  );
}

function SnippetVideo({ onClose }: { onClose: () => void }) {
  const video = useRef<HTMLVideoElement>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const element = video.current;
    if (currentMotion() !== 'reduced') void element?.play().catch(() => {});
    return () => { element?.pause(); };
  }, []);
  return <Dialog title="How snippets work" onClose={onClose}><div className="snippet-video-player">
    <video ref={video} tabIndex={0} src="/media/snippets-transformation.mp4" poster="/media/snippets-transformation.webp" controls muted loop playsInline preload="metadata" aria-label="A spoken phrase expands into saved text" onError={() => setFailed(true)} />
    <p>{failed ? 'The video could not play. ' : 'Example: “first name” becomes your own name inside a message. '}{failed && <a href="/media/snippets-transformation.mp4" download>Download video</a>}</p>
  </div></Dialog>;
}
