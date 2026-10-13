import { useEffect, useState } from 'react';
import { ChevronLeft } from 'lucide-react';
import { invoke } from '../desktop-session/runtime/native';
import { PRODUCT_NAME } from '../../product';

/** One entry of notices/index.json, written by scripts/desktop/notices.mjs. */
export interface Notice { path: string; title: string; group: string; kind: 'text' | 'source' }

const GROUPS = ['Models', 'Application', 'Speech and text runtime'];

/**
 * Every notice the installer ships, read from the installed files through the
 * shell. Nothing is fetched, so the page works offline.
 */
export function Licenses({ onBack }: { onBack: () => void }) {
  const [notices, setNotices] = useState<Notice[] | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [text, setText] = useState<Record<string, string>>({});
  const [problem, setProblem] = useState('');
  const fail = (error: unknown) => setProblem(error instanceof Error ? error.message : String(error));

  useEffect(() => {
    invoke<Notice[]>('notices_index').then(setNotices).catch((error: unknown) => { setNotices([]); fail(error); });
  }, []);

  function toggle(path: string) {
    setProblem('');
    if (open === path) { setOpen(null); return; }
    setOpen(path);
    if (text[path] === undefined) {
      invoke<string>('notice_text', { path }).then((value) => setText((all) => ({ ...all, [path]: value }))).catch(fail);
    }
  }

  const groups = [...new Set([...GROUPS, ...(notices ?? []).map((notice) => notice.group)])];

  return (
    <div className="licenses">
      <button type="button" className="button ghost small" onClick={onBack}><ChevronLeft size={14} aria-hidden /> About</button>
      <h3>Open-source licenses</h3>
      <p className="faint">
        {PRODUCT_NAME} includes the components below. Their license texts are installed with the app, so this page
        works offline. Models are not installed with the app; each is downloaded only when you choose it.
      </p>
      {problem && <p className="notice alert" role="alert">{problem}</p>}
      {notices === null && <p className="faint">Reading the installed notices…</p>}
      {notices?.length === 0 && !problem && (
        <p className="notice">This build has no installed notices. Installed releases include them.</p>
      )}
      {groups.map((group) => {
        const entries = (notices ?? []).filter((notice) => notice.group === group);
        return entries.length > 0 && (
          <section key={group} className="licenses-group">
            <h4>{group}</h4>
            <ul>
              {entries.map((notice) => (
                <li key={notice.path}>
                  {notice.kind === 'source' ? (
                    <div className="licenses-source">
                      <span>{notice.title} · source code, installed in the notices folder</span>
                      <button type="button" className="button small" onClick={() => invoke('notices_folder').catch(fail)}>Show folder</button>
                    </div>
                  ) : (
                    <>
                      <button type="button" className="linklike" aria-expanded={open === notice.path} onClick={() => toggle(notice.path)}>
                        {notice.title}
                      </button>
                      {open === notice.path && (
                        <pre className="licenses-text" tabIndex={0} aria-label={`${notice.title} text`}>
                          {text[notice.path] ?? 'Reading…'}
                        </pre>
                      )}
                    </>
                  )}
                </li>
              ))}
            </ul>
          </section>
        );
      })}
    </div>
  );
}
