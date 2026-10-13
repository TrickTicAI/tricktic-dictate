/**
 * What a formatted result kept of what was said.
 *
 * A dictation has no correct answer to compare against, so these count only
 * things that can be checked against the transcript itself: words that are gone,
 * questions that are gone, and fillers that were left in. Removing "um" is the
 * job; losing the question at the end is not.
 */
const FILLERS = new Set('um uh uhm erm hmm like so okay ok yeah you know i mean basically actually just kind of sort right well אה אמ אממ כאילו בעצם יעני טוב אז נו'.split(' '));
const NOISE = new Set(['um', 'uh', 'uhm', 'erm', 'אה', 'אמ', 'אממ']);
const words = (text: string): string[] => text.toLowerCase().replace(/’/g, "'").match(/[\p{L}\p{N}']+/gu) ?? [];
const sentences = (text: string) => text.trim().split(/(?<=[.?!])\s+/).filter(part => words(part).length);

export interface Score {
  /** Share of the transcript's meaningful words missing from the result, 0 to 1. */
  lost: number;
  /** The transcript asked something the result no longer asks. */
  lostQuestion: boolean;
  /** Most of the last sentence is gone. */
  lostEnding: boolean;
  /** Fillers and repeated words the result still carries. */
  leftovers: number;
  /** The result changed script, so something was probably translated. */
  changedLanguage: boolean;
}
export function score(input: string, output: string): Score {
  const spoken = words(input).filter(word => !FILLERS.has(word));
  const produced = words(output);
  const counts = new Map<string, number>();
  for (const word of produced) counts.set(word, (counts.get(word) ?? 0) + 1);
  let missing = 0;
  const left = new Map(counts);
  for (const word of spoken) {
    const remaining = left.get(word) ?? 0;
    if (remaining > 0) left.set(word, remaining - 1); else missing++;
  }
  const last = sentences(input).at(-1);
  const tail = last ? words(last).filter(word => !FILLERS.has(word) && word.length > 2) : [];
  const hebrew = (text: string) => /[֐-׿]/.test(text);
  return {
    lost: spoken.length ? missing / spoken.length : 0,
    lostQuestion: (input.match(/\?/g)?.length ?? 0) > (output.match(/\?/g)?.length ?? 0),
    lostEnding: tail.length > 0 && tail.filter(word => !produced.includes(word)).length / tail.length >= 0.6,
    leftovers: produced.filter(word => NOISE.has(word)).length + produced.filter((word, i) => i > 0 && word === produced[i - 1]).length,
    changedLanguage: hebrew(input) !== hebrew(output),
  };
}
export const median = (values: number[]) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted.length ? sorted[Math.max(0, Math.ceil(sorted.length / 2) - 1)] : null;
};
