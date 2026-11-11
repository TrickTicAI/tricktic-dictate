import { useState } from 'react';
import { BookA, Check, Ear, X } from 'lucide-react';
import { learningApi, suggestionKey, type LearningApi, type Suggestion } from '../vocabulary/learning';

/**
 * What a saved correction can teach: new words for the dictionary, and pairs
 * of what recognition heard and what you meant, which the polish is given from
 * the next dictation on. Each is accepted or declined on its own; a declined
 * one is never proposed again.
 */
export function LearnFromCorrection({ suggestions, onDecided, api = learningApi }: {
  suggestions: Suggestion[];
  onDecided: (suggestion: Suggestion) => void;
  api?: LearningApi;
}) {
  const [busy, setBusy] = useState('');
  const [error, setError] = useState('');
  if (!suggestions.length) return null;

  async function decide(suggestion: Suggestion, accept: boolean) {
    setBusy(suggestionKey(suggestion)); setError('');
    try { await api.decide(suggestion, accept); onDecided(suggestion); }
    catch (failure) { setError(`${(failure as Error).message} Your correction is saved.`); }
    finally { setBusy(''); }
  }

  return (
    <section className="learn" aria-label="Learn from this correction">
      <p className="small muted">Learn from this correction?</p>
      <ul>
        {suggestions.map((suggestion) => {
          const key = suggestionKey(suggestion);
          const term = suggestion.kind === 'term';
          return (
            <li key={key}>
              {term
                ? <span><BookA size={14} aria-hidden /> Add “<bdi>{suggestion.text}</bdi>” to your words</span>
                : <span><Ear size={14} aria-hidden /> “<bdi>{suggestion.heard}</bdi>” is often “<bdi>{suggestion.meant}</bdi>”</span>}
              <span className="learn-actions">
                <button type="button" className="button small" disabled={!!busy} onClick={() => void decide(suggestion, true)}>
                  <Check size={14} aria-hidden /> {term ? 'Add' : 'Learn'}
                </button>
                <button type="button" className="button small ghost" disabled={!!busy} aria-label={`Not this: ${term ? suggestion.text : `${suggestion.heard} as ${suggestion.meant}`}`} onClick={() => void decide(suggestion, false)}>
                  <X size={14} aria-hidden /> No
                </button>
              </span>
            </li>
          );
        })}
      </ul>
      {error && <p className="notice alert" role="alert">{error}</p>}
    </section>
  );
}
