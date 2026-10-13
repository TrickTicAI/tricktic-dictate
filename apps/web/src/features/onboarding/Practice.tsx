import { isMac } from '../settings/platform';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Mic, RefreshCw, Shuffle } from 'lucide-react';
import type { InputLanguage } from '@metrix-writer/contracts';
import type { SessionState } from '../desktop-session/contracts/protocol';
import { compareSpoken } from './compare';
import { Keycaps } from './Chord';
import { PRODUCT_NAME } from '../../product';

/**
 * Short, ordinary sentences rather than tongue twisters: the point is to prove
 * the path works end to end, not to score the model. Each set is written in the
 * language it is offered for, including the mixed one.
 */
export const PROMPTS: Record<InputLanguage, string[]> = {
  en: [
    `${PRODUCT_NAME} keeps my audio even when the model fails.`,
    'Send the meeting notes to the team before four o’clock.',
    'Remind me to review the pull request tomorrow morning.',
  ],
  he: [
    'רייטר שומר את ההקלטה שלי גם אם המודל נכשל.',
    'שלח לצוות את סיכום הפגישה לפני ארבע.',
    'תזכיר לי לעבור על הקוד מחר בבוקר.',
  ],
  mixed: [
    'Send the סיכום פגישה to the team before four o’clock.',
    'רייטר keeps my audio גם אם המודל נכשל.',
  ],
};

// `saved` is included so the strip stays continuous while the recording is
// handed to the model. A session that stops there because no model can run is
// explained by the availability line, which is shown throughout.
const WORKING: SessionState[] = ['requesting', 'capturing', 'saving', 'saved', 'queued', 'transcribing'];

export interface PracticeProps {
  language: InputLanguage;
  /** What is about to do the transcribing, in the words the rest of the app uses. */
  engine: string;
  /** Null when the shell's shortcut settings could not be read. */
  chord: string | null;
  toggle: boolean;
  /** The record chord is physically down right now. */
  held: boolean;
  state: SessionState;
  sessionId: string | null;
  /** Raw transcript of the current session, once it has one. */
  transcript: string | null;
  /** Why a dictation could not run or did not finish. */
  problem: string | null;
  levels: number[];
  onStart: () => void;
  onStop: () => void;
  onOutcome: (outcome: 'passed' | 'failed') => void;
}

/**
 * One real dictation, end to end, before the workspace opens.
 *
 * The sentence is a prompt, not a target: what comes back replaces it verbatim,
 * and the word comparison underneath says only which words of that one reading
 * came back. It is not a measurement of the model, and it says so.
 */
export function Practice(props: PracticeProps) {
  const { state, sessionId, transcript, problem, onOutcome } = props;
  const prompts = PROMPTS[props.language];
  const [index, setIndex] = useState(0);
  const prompt = prompts[index % prompts.length];
  const [attempt, setAttempt] = useState<string | null>(null);
  const [result, setResult] = useState<{ session: string; text: string } | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const reported = useRef('');

  // Only the dictation started from this screen counts. A session restored from
  // history must never be presented as something that just happened.
  useEffect(() => {
    if (sessionId && (state === 'requesting' || state === 'capturing') && sessionId !== attempt) {
      setAttempt(sessionId); setResult(null); setFailure(null);
    }
  }, [sessionId, state, attempt]);

  useEffect(() => {
    if (!attempt || sessionId !== attempt || reported.current === `${attempt}:${state}`) return;
    if (state === 'success' && transcript?.trim()) {
      reported.current = `${attempt}:${state}`;
      setResult({ session: attempt, text: transcript });
      onOutcome('passed');
    } else if (state === 'error') {
      reported.current = `${attempt}:${state}`;
      setFailure(problem ?? 'The dictation did not finish. Your audio is saved.');
      onOutcome('failed');
    }
  }, [attempt, sessionId, state, transcript, problem, onOutcome]);

  const comparison = useMemo(() => (result ? compareSpoken(prompt, result.text) : null), [result, prompt]);
  const working = WORKING.includes(state) && sessionId === attempt;
  const level = props.levels[props.levels.length - 1] ?? 0;

  return (
    <div className="ob-step">
      <div className="ob-head">
        <span className="ob-glyph"><Mic size={20} aria-hidden /></span>
        <div>
          <h1>Now say something</h1>
          <p>
            {props.chord
              ? `${props.toggle ? 'Press' : 'Hold'} the shortcut and read the sentence out loud${props.toggle ? ', then press it again' : ', then let go'}.`
              : 'Start the recording below and read the sentence out loud.'}
            {' '}Your words replace it. Transcribing with {props.engine}.
          </p>
        </div>
      </div>

      <div className="ob-stage">
        <div className="row" style={{ justifyContent: 'space-between', marginBottom: 12 }}>
          <div className="row">
            {props.chord
              ? <>
                  <Keycaps chord={props.chord} held={props.held} />
                  <span className="small faint">{props.held ? 'Got it — keep talking' : props.toggle ? 'Press to start' : 'Hold and speak'}</span>
                </>
              : <span className="small faint">Your shortcut settings could not be read. Use the button below instead.</span>}
          </div>
          <span className={`ob-hold ${props.held || state === 'capturing' ? 'on' : ''}`} aria-hidden>
            <b>{props.held || state === 'capturing' ? 'Live' : 'Ready'}</b>
          </span>
        </div>

        <p
          className={`ob-prompt ${result ? 'spoken' : 'waiting'}`}
          dir="auto"
          aria-live="polite"
          aria-label={result ? 'What you said' : 'Sentence to read aloud'}
        >
          {result ? result.text : prompt}
        </p>

        <div className="ob-phase" aria-live="polite">
          {state === 'requesting' && sessionId === attempt && <><span className="ob-dot" /><span>Opening the microphone…</span></>}
          {state === 'capturing' && sessionId === attempt && (
            <>
              <span className="ob-ring" style={{ ['--ob-level' as string]: level }}><Mic size={16} aria-hidden /></span>
              <span>Listening — your audio is already being written to disk.</span>
              <span className="waveform" aria-hidden>
                {props.levels.map((value, position) => <i key={position} style={{ height: `${Math.max(2, Math.round(value * 22))}px` }} />)}
              </span>
            </>
          )}
          {state === 'saving' && sessionId === attempt && <><span className="ob-dot" /><span>Saving the recording…</span></>}
          {state === 'saved' && sessionId === attempt && !result && <><span className="ob-dot" /><span>Audio saved. Handing it to {props.engine}…</span></>}
          {(state === 'queued' || state === 'transcribing') && sessionId === attempt && (
            <><span className="ob-dot" /><span>Transcribing with {props.engine}…</span></>
          )}
          {result && (
            <>
              <svg className="ob-check" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
                <path d="M20 6 9 17l-5-5" />
              </svg>
              <span>{isMac() ? `That is what ${PRODUCT_NAME} heard. It is on the clipboard too. When started from another app with Accessibility enabled, ${PRODUCT_NAME} sends a paste there. You can always copy it from history.` : `That is what ${PRODUCT_NAME} heard. It is on the clipboard too; started from another application, it types the text in for you.`}</span>
            </>
          )}
        </div>

        {(state === 'queued' || state === 'transcribing') && sessionId === attempt && (
          <div className="ob-progress indeterminate"><i /></div>
        )}

        {comparison && (
          <>
            <div className="ob-words" aria-label="Word comparison">
              {comparison.cells.map((cell, position) => (
                <span key={`${cell.text}-${position}`} className="ob-word" data-status={cell.status} dir="auto">{cell.text}</span>
              ))}
            </div>
            <p className="ob-source">
              {comparison.matched} of {comparison.expected} words from the prompt came back, ignoring
              punctuation and capitalisation. One sentence read once is not a measure of a model —
              judge it on your own dictation, and change model any time in Settings → Speech engine.
            </p>
          </>
        )}

        <div className="row" style={{ marginTop: 14 }}>
          {working
            ? <button type="button" className="button" onClick={props.onStop}>Stop now</button>
            : <button type="button" className="button primary" onClick={() => { setResult(null); setFailure(null); props.onStart(); }}>
                <Mic size={13} aria-hidden /> {result || failure ? 'Try again' : 'Record without the shortcut'}
              </button>}
          {!working && (result || failure) && (
            <button type="button" className="button ghost small" onClick={() => { setIndex((value) => value + 1); setResult(null); setFailure(null); }}>
              <Shuffle size={12} aria-hidden /> Different sentence
            </button>
          )}
          {!working && !result && !failure && props.problem && (
            <button type="button" className="button ghost small" onClick={props.onStart}><RefreshCw size={12} aria-hidden /> Retry</button>
          )}
        </div>
      </div>

      {failure && <p className="notice alert" role="alert">{failure} Your audio was kept, so nothing was lost.</p>}
      {/* Shown throughout, not only when idle: a dictation that stops at
          "saved" because no model can run has to say so while it is happening. */}
      {!failure && !result && props.problem && <p className="notice alert" role="alert">{props.problem}</p>}
    </div>
  );
}
