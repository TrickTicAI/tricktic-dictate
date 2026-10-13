import { useLayoutEffect, useRef } from 'react';
import { Info } from 'lucide-react';
import { formatDuration } from '../../recorder';
import type { ModeKind } from '../modes/api';
import { Keys } from '../shell/Keys';
import { Liveness, Steps } from './Dock';
import type { Flow } from './useDictationFlow';

/**
 * While you dictate, the reader becomes the stage: the words arrive large with
 * the older ones fading, a wide waveform of the real level, and how to finish.
 * Afterwards the words shimmer while recognition and the mode finish, and the
 * finished dictation takes the reader back.
 *
 * Live words exist only where a local model drafts them while you speak. A
 * cloud model hears the recording once it is complete, and the stage says so
 * instead of promising words that will not come.
 *
 * The stage fills the reader's rectangle exactly and scrolls inside itself, so
 * nothing else on Home moves however long the dictation runs.
 */
export function LiveStage({ flow, text, levels, elapsedMs, shortcut, toggle, livePreview, model, kind, textModel, deliver }: {
  flow: Flow;
  text: string;
  levels: number[];
  elapsedMs: number;
  shortcut: string | null;
  toggle: boolean;
  /** A local model is drafting words while you speak. */
  livePreview: boolean;
  /** The speech model's display name. */
  model: string;
  kind?: ModeKind;
  textModel?: string | null;
  deliver: boolean;
}) {
  const recording = flow === 'recording';
  const working = flow === 'transcribing' || flow === 'processing';
  const viewport = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = viewport.current;
    if (!element) return;
    const follow = () => { element.scrollTop = element.scrollHeight; };
    follow();
    const observer = new ResizeObserver(follow);
    observer.observe(element);
    if (element.firstElementChild) observer.observe(element.firstElementChild);
    return () => observer.disconnect();
  }, [text]);

  const words = text.trim() ? text.trim().split(/(\s+)/) : [];
  // The newest few words at full strength, everything before them quieter.
  let strong = 0;
  let split = words.length;
  while (split > 0 && strong < 9) { split -= 1; if (!/^\s+$/.test(words[split])) strong += 1; }

  return (
    <section className="live-stage" aria-label="Dictation in progress" data-flow={flow}>
      <div className="live-top">
        {recording && (
          <>
            <span className="live-label"><span className="live-rec" aria-hidden /> Listening</span>
            <span className="live-timer" aria-hidden>{formatDuration(elapsedMs)}</span>
          </>
        )}
        {flow === 'opening' && <span className="live-label"><Liveness /> Opening the microphone…</span>}
        {working && <Steps flow={flow} kind={kind} textModel={textModel} deliver={deliver} long />}
        <span className="spacer" />
        {recording && shortcut && (
          <span className="live-release">
            {toggle ? <>Press <Keys chord={shortcut} /> again to finish</> : <>Release <Keys chord={shortcut} /> to finish</>}
          </span>
        )}
      </div>

      <div className="live-viewport" ref={viewport} tabIndex={0} aria-label="Live draft">
        <div className="live-body">
          {words.length
            ? (
              <p className={`live-text${working ? ' processing' : ''}`} dir="auto">
                <span className="live-old">{words.slice(0, split).join('')}</span>
                {words.slice(split).join('')}
                {recording && <span className="live-caret" aria-hidden />}
              </p>
            )
            : working
              ? <div className="skeleton live-skeleton" aria-label="Transcribing"><i style={{ width: '92%' }} /><i style={{ width: '84%' }} /><i style={{ width: '58%' }} /></div>
              : (
                <p className="live-text placeholder">
                  {flow === 'opening'
                    ? 'Your words will appear here as you speak.'
                    : livePreview
                      ? 'Listening… your words will appear here.'
                      : `Your words appear when you stop — ${model} transcribes the whole recording at once.`}
                </p>
              )}
        </div>
      </div>

      <div className="live-foot">
        {recording
          ? <div className="live-wave" aria-hidden>{levels.map((value, index) => <i key={index} className={value < 0.02 ? 'silent' : undefined} style={{ height: `${Math.max(4, Math.round(value * 100))}%` }} />)}</div>
          : (
            <p className="live-note">
              <Info size={15} aria-hidden />
              {working ? 'Live draft · the final text replaces it when transcription finishes.' : 'Your recording is saved as you speak.'}
            </p>
          )}
      </div>
    </section>
  );
}
