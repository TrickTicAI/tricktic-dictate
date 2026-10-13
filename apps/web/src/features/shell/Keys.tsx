import { chordTokens } from '../desktop-preferences/chord';

/** A shortcut drawn as keycaps, in this platform's notation (⌘ ⌥ ⇧ on a Mac). */
export function Keys({ chord, large, title }: { chord: string; large?: boolean; title?: string }) {
  return (
    <span className={`kbd${large ? ' large' : ''}`} title={title}>
      {chordTokens(chord).map((token, index) => <kbd key={`${token}-${index}`}>{token}</kbd>)}
    </span>
  );
}
