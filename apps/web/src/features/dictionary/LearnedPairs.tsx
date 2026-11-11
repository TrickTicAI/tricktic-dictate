import { useEffect, useState } from 'react';
import { ArrowRight, Ear, Trash2 } from 'lucide-react';
import { learningApi, type LearningApi, type Mishearing } from '../vocabulary/learning';

/**
 * Pairs learned from your corrections: what recognition heard and what you
 * meant. Unlike corrections they replace nothing by themselves; the polish is
 * given them and decides from the sentence whether the term was meant.
 */
export function LearnedPairs({ api = learningApi }: { api?: LearningApi }) {
  const [pairs, setPairs] = useState<Mishearing[] | null>(null);
  const [error, setError] = useState('');
  useEffect(() => {
    let active = true;
    // Learning is optional: when it cannot be read the dictionary works as before.
    api.load().then((learning) => { if (active) setPairs(learning.mishearings); }, () => { if (active) setPairs([]); });
    return () => { active = false; };
  }, [api]);

  async function remove(index: number) {
    if (!pairs) return;
    setError('');
    try { setPairs((await api.setMishearings(pairs.filter((_, i) => i !== index))).mishearings); }
    catch (failure) { setError((failure as Error).message); }
  }

  if (!pairs?.length && !error) return null;
  return (
    <section className="entry-list rules learned-pairs" aria-label="Learned from corrections">
      <p className="learned-head"><Ear size={15} aria-hidden /> <strong>Learned from your corrections</strong> <span className="muted">The polish gets these as hints and changes a word only when the sentence fits.</span></p>
      {pairs?.map((pair, index) => (
        <div key={`${pair.heard}→${pair.meant}`} className="entry rule">
          <span className="rule-from"><bdi dir="auto">{pair.heard}</bdi></span>
          <ArrowRight size={14} aria-hidden />
          <span className="rule-to"><bdi dir="auto">{pair.meant}</bdi>{pair.when && <span className="muted"> ({pair.when})</span>}</span>
          <button type="button" className="icon-button" aria-label={`Forget ${pair.heard} as ${pair.meant}`} onClick={() => void remove(index)}>
            <Trash2 size={15} aria-hidden />
          </button>
        </div>
      ))}
      {error && <p className="notice alert" role="alert">{error}</p>}
    </section>
  );
}
