import type { Section } from './SettingsModal';
import { PRODUCT_NAME } from '../../product';

/**
 * Settings search.
 *
 * Search covers everything Settings shows: titles, descriptions, options,
 * buttons, notes, and the names of themes, models and providers. While a
 * search is active the dialog mounts every section out of sight, and `harvest`
 * reads their text into one chunk per setting. It reads the rendered page,
 * so there is nothing to keep in step: a setting the current client does not
 * show (a desktop control in the browser, a Windows control on a Mac) is simply
 * not there to be found.
 *
 * `SYNONYMS` adds the words people use that the interface does not, such as
 * "hotkey" for a shortcut. `tests/search.spec.ts` checks each still names a
 * title its section renders.
 */

export const SYNONYMS: { section: Section; title: string; keywords: string }[] = [
  { section: 'general', title: 'Speech language', keywords: 'dictate spoken input accent' },
  { section: 'general', title: 'Output', keywords: 'translate translation' },
  { section: 'general', title: 'Interface language', keywords: 'ui display rtl right to left' },
  { section: 'general', title: 'Animations', keywords: 'motion accessibility reduced motion reduce animation' },
  { section: 'general', title: 'Cleanup pass', keywords: 'clean up polish tidy format ai' },
  { section: 'general', title: `Closing the window keeps ${PRODUCT_NAME} running`, keywords: 'background minimize' },
  { section: 'general', title: `Launch ${PRODUCT_NAME} at login`, keywords: 'startup start automatically boot autostart' },
  { section: 'general', title: 'Run the setup guide again', keywords: 'onboarding tutorial wizard' },
  { section: 'general', title: `Quit ${PRODUCT_NAME}`, keywords: 'exit close shut down' },
  { section: 'appearance', title: 'Theme', keywords: 'dark light mode colour color palette look skin' },
  { section: 'shortcuts', title: 'Record and stop', keywords: 'hotkey keyboard shortcut key binding push to talk' },
  { section: 'shortcuts', title: 'Discard dictation', keywords: 'hotkey keyboard shortcut cancel abort' },
  { section: 'shortcuts', title: 'Record and stop dictation', keywords: 'hotkey keyboard shortcut key binding push to talk' },
  { section: 'shortcuts', title: 'How the shortcut behaves', keywords: 'push to talk hands free toggle' },
  { section: 'shortcuts', title: 'Where the text goes', keywords: 'paste insert cursor caret' },
  { section: 'shortcuts', title: 'Preserve clipboard after dictation', keywords: 'copy paste restore' },
  { section: 'shortcuts', title: 'Private clipboard delivery', keywords: 'privacy clipboard manager' },
  { section: 'microphone', title: 'Microphone device', keywords: 'mic headset input source audio' },
  { section: 'microphone', title: 'Dictation and notification sounds', keywords: 'audio beep chime tone' },
  { section: 'microphone', title: 'Reduce other audio while dictating', keywords: 'duck mute music' },
  { section: 'panel', title: 'When to show the panel', keywords: 'overlay pill indicator visibility floating' },
  { section: 'panel', title: 'Placement', keywords: 'overlay position location corner' },
  { section: 'panel', title: 'Panel size', keywords: 'overlay scale bigger smaller zoom' },
  { section: 'panel', title: 'Colours', keywords: 'colors overlay waveform' },
  { section: 'panel', title: 'Dismiss saved-audio reminders automatically', keywords: 'notification' },
  { section: 'panel', title: 'Saved-audio reminder duration', keywords: 'notification time how long' },
  { section: 'engine', title: 'Model', keywords: 'speech recognition stt engine transcription' },
  { section: 'engine', title: 'Keys stay on your server', keywords: 'api key token credentials' },
  { section: 'engine', title: 'Change engine', keywords: 'speech recognition stt model download install' },
  { section: 'engine', title: 'Processor for speech recognition', keywords: 'gpu cpu hardware acceleration' },
  { section: 'engine', title: 'Models folder', keywords: 'storage directory disk' },
  { section: 'text', title: 'Text models for Super and Command', keywords: 'llm ai api key' },
  { section: 'text', title: 'Processor for text processing', keywords: 'gpu cpu hardware acceleration' },
  { section: 'modes', title: 'Default recording mode', keywords: 'profile' },
  { section: 'modes', title: 'Text model', keywords: 'llm ai' },
  { section: 'modes', title: 'Fallback model', keywords: 'backup slow timeout' },
  { section: 'modes', title: 'Rewrite strength', keywords: 'polish style tone' },
  { section: 'modes', title: 'Instructions', keywords: 'prompt system prompt' },
  { section: 'modes', title: 'Model speed', keywords: 'reasoning thinking effort' },
  { section: 'modes', title: 'Mode shortcut', keywords: 'hotkey keyboard' },
  { section: 'account', title: 'No account needed', keywords: 'sign in login google sync profile user sign out logout switch account delete account cloud data' },
  { section: 'keys', title: 'API keys', keywords: 'api key keys manage keys token secret credentials password openai gemini grok openrouter add replace remove' },
  { section: 'data', title: 'Provider keys', keywords: 'credentials security storage keychain dpapi' },
  { section: 'data', title: 'Delete all audio and transcripts', keywords: 'erase wipe clear reset' },
  { section: 'data', title: 'Storage', keywords: 'space disk usage size quota limit models gigabytes recordings audio' },
  { section: 'data', title: 'Free up space', keywords: 'delete clear old audio recordings transcripts yesterday today erase wipe history storage' },
  { section: 'data', title: 'Report a suspected vulnerability privately', keywords: 'security bug' },
  { section: 'about', title: PRODUCT_NAME, keywords: 'version update changelog' },
  { section: 'about', title: 'Open-source licenses', keywords: 'license licence notices attribution third party legal credits' },
  { section: 'about', title: 'Privacy policy', keywords: 'legal data gdpr' },
  { section: 'about', title: 'Terms of use', keywords: 'legal terms of service tos eula' },
  { section: 'about', title: 'Support', keywords: 'help contact email' },
];

export const SECTION_LABELS: Record<Section, string> = {
  engine: 'Speech engine',
  shortcuts: 'Shortcuts & pasting',
  microphone: 'Microphone & sound',
  panel: 'Floating panel',
  modes: 'Modes',
  text: 'Text models',
  keys: 'API keys',
  general: 'General',
  appearance: 'Appearance',
  account: 'Account & sync',
  data: 'Privacy & data',
  about: 'About & updates',
};

/** One searchable place in Settings: a row, or a titled block such as a model or a group note. */
export interface Chunk {
  section: Section;
  /** The row or heading title; empty for text directly under the section. */
  title: string;
  group?: string;
  texts: string[];
}

const clean = (text: string | null | undefined) => (text ?? '').replace(/\s+/g, ' ').trim();

function rowTitle(row: Element) {
  return clean(row.querySelector('.setting-text strong')?.textContent);
}

/** The title an element gives the block it heads: a direct heading, or its own accessible name. */
function headingOf(element: Element): string | undefined {
  const heading = [...element.children].find((child) => child.matches('h3, h4'));
  if (heading) return clean(heading.textContent);
  if (element.matches('li[aria-label], section[aria-label]')) return clean(element.getAttribute('aria-label'));
  return undefined;
}

/** The setting an element belongs to: its row, else the nearest titled block, else the section. */
function anchorOf(element: Element, root: Element): { element: Element; title: string } {
  const row = element.closest('.setting-row');
  if (row && root.contains(row)) return { element: row, title: rowTitle(row) };
  // A heading names the block around it, not a block of its own.
  let current: Element | null = element.matches('h3, h4') ? element.parentElement : element;
  for (; current && current !== root; current = current.parentElement) {
    const title = headingOf(current);
    if (title) return { element: current, title };
  }
  return { element: root, title: '' };
}

/**
 * Reads every mounted section under `container` — each wrapped in an element
 * with `data-section` — into chunks. Text includes accessible names and
 * placeholders, so icon buttons and empty fields are found by what they say.
 * Typed values are never read.
 */
export function harvest(container: Element): Chunk[] {
  const chunks = new Map<Element, Chunk>();
  for (const root of container.querySelectorAll<HTMLElement>('[data-section]')) {
    const section = root.dataset.section as Section;
    const add = (element: Element, text: string) => {
      text = clean(text);
      if (!text) return;
      const anchor = anchorOf(element, root);
      let chunk = chunks.get(anchor.element);
      if (!chunk) {
        const group = clean(anchor.element.closest('.settings-group')?.querySelector(':scope > h3')?.textContent);
        chunk = { section, title: anchor.title, group: group && group !== anchor.title ? group : undefined, texts: [] };
        chunks.set(anchor.element, chunk);
      }
      if (!chunk.texts.includes(text)) chunk.texts.push(text);
    };
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const parent = node.parentElement;
      if (parent && !parent.closest('style, script, textarea')) add(parent, node.textContent ?? '');
    }
    for (const element of root.querySelectorAll('[aria-label], [placeholder], [title]')) {
      for (const name of ['aria-label', 'placeholder', 'title']) add(element, element.getAttribute(name) ?? '');
    }
  }
  return [...chunks.values()];
}

/** Lowercased words with accents folded; letters in any script are kept. */
export function words(text: string): string[] {
  return text.toLowerCase().normalize('NFKD').replace(/[̀-ͯ]/g, '')
    .split(/[^\p{L}\p{N}]+/u).filter(Boolean);
}

/** Optimal string alignment distance: edits, counting a swap of neighbours as one. */
function distance(a: string, b: string): number {
  const rows = Array.from({ length: a.length + 1 }, (_, i) => [i, ...Array<number>(b.length).fill(0)]);
  for (let j = 1; j <= b.length; j++) rows[0][j] = j;
  for (let i = 1; i <= a.length; i++) {
    for (let j = 1; j <= b.length; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      rows[i][j] = Math.min(rows[i - 1][j] + 1, rows[i][j - 1] + 1, rows[i - 1][j - 1] + cost);
      if (i > 1 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) rows[i][j] = Math.min(rows[i][j], rows[i - 2][j - 2] + 1);
    }
  }
  return rows[a.length][b.length];
}

/**
 * How well one typed word matches one indexed word, from 0 (not at all) to 3.
 * Misspellings are allowed in proportion to length, so "cleeboard" finds
 * "clipboard" while "key" does not find "kit".
 */
function wordMatch(query: string, word: string): number {
  if (word === query) return 3;
  if (word.startsWith(query)) return 2.5;
  if (query.length >= 3 && word.includes(query)) return 2;
  const allowed = query.length <= 3 ? 0 : query.length <= 5 ? 1 : 2;
  if (!allowed) return 0;
  if (Math.abs(word.length - query.length) <= allowed && distance(query, word) <= allowed) return 1.5;
  // Still typing a long word: compare with its beginning.
  if (word.length > query.length && distance(query, word.slice(0, query.length)) <= allowed) return 1;
  return 0;
}

const best = (term: string, list: string[]) => list.reduce((top, word) => Math.max(top, wordMatch(term, word)), 0);

export interface SearchResult {
  section: Section;
  sectionLabel: string;
  /** The row or block to reveal; empty to open the section at its top. */
  title: string;
  group?: string;
  /** The text that matched, when the title alone did not. */
  snippet?: string;
}

/** Shortens a matched sentence around its first matching word. */
function excerpt(text: string, terms: string[]): string {
  if (text.length <= 110) return text;
  const lower = text.toLowerCase();
  const at = Math.max(0, ...terms.map((term) => lower.indexOf(term)).filter((index) => index >= 0).slice(0, 1));
  const start = Math.max(0, Math.min(at - 40, text.length - 110));
  return `${start > 0 ? '…' : ''}${text.slice(start, start + 110).trim()}${start + 110 < text.length ? '…' : ''}`;
}

/**
 * Chunks matching every word of the query, best first. A word can match the
 * title, the group or section, a synonym, or any text in the chunk; the title
 * counts most.
 */
export function searchSettings(query: string, chunks: Chunk[]): SearchResult[] {
  const terms = words(query);
  if (!terms.length) return [];
  const scored: { result: SearchResult; score: number; order: number }[] = [];
  const seen = new Set<string>();
  chunks.forEach((chunk, order) => {
    const key = `${chunk.section}/${chunk.title}`;
    const sectionLabel = SECTION_LABELS[chunk.section];
    const title = words(chunk.title);
    const place = words(`${chunk.group ?? ''} ${sectionLabel}`);
    const synonyms = words(SYNONYMS.filter((entry) => entry.section === chunk.section && entry.title === chunk.title).map((entry) => entry.keywords).join(' '));
    const texts = chunk.texts.filter((text) => text !== chunk.title && text !== chunk.group).map((text) => ({ text, words: words(text) }));
    let score = 0;
    for (const term of terms) {
      const top = Math.max(best(term, title) * 3, best(term, place) * 2, best(term, synonyms) * 1.5, ...texts.map((text) => best(term, text.words)));
      if (!top) return;
      score += top;
    }
    if (seen.has(key)) return;
    seen.add(key);
    // Show the text that matched when the title does not explain the result.
    const titled = terms.every((term) => best(term, title) || best(term, place));
    const matched = titled ? undefined : texts.find((text) => terms.some((term) => best(term, text.words)));
    scored.push({
      result: { section: chunk.section, sectionLabel, title: chunk.title, group: chunk.group, snippet: matched && excerpt(matched.text, terms) },
      score,
      order,
    });
  });
  return scored.sort((a, b) => b.score - a.score || a.order - b.order).map(({ result }) => result);
}

/** Opens every collapsed `<details>` around an element, so a found setting can be shown. */
function unfold(element: HTMLElement) {
  for (let parent = element.parentElement; parent; parent = parent.parentElement) {
    if (parent instanceof HTMLDetailsElement && !parent.open) parent.open = true;
  }
}

/** The visible element a search result names within `container`. */
function findTarget(container: HTMLElement, title: string): HTMLElement | undefined {
  // A setting folded away in a collapsed group or list is still the setting.
  for (const row of container.querySelectorAll<HTMLElement>('details:not([open]) .setting-row, details:not([open]) h3, details:not([open]) h4, details:not([open]) li[aria-label], details:not([open]) section[aria-label]')) {
    const block = row.matches('h3, h4') ? row.parentElement : row;
    const name = row.matches('.setting-row') ? rowTitle(row) : block ? headingOf(block) : undefined;
    if (block && name === title && block.closest('[data-section]:not([hidden])')) unfold(block);
  }
  const visible = (element: HTMLElement) => element.offsetParent !== null;
  for (const row of container.querySelectorAll<HTMLElement>('.setting-row')) {
    if (visible(row) && rowTitle(row) === title) return row;
  }
  for (const element of container.querySelectorAll<HTMLElement>('h3, h4, li[aria-label], section[aria-label]')) {
    const block = element.matches('h3, h4') ? element.parentElement : element;
    if (block && visible(block) && headingOf(block) === title) return block;
  }
  return undefined;
}

/**
 * Scrolls a setting found by search into view and marks it briefly. The
 * section may still be loading, so this keeps looking for a short while and
 * gives up quietly: the section is open either way.
 */
export function revealSetting(container: HTMLElement, title: string): () => void {
  let frame = 0;
  let timer = 0;
  let marked: HTMLElement | undefined;
  const deadline = performance.now() + 3000;
  const find = () => {
    const target = findTarget(container, title);
    if (!target) {
      if (performance.now() < deadline) frame = requestAnimationFrame(find);
      return;
    }
    target.scrollIntoView({ block: 'center' });
    target.classList.remove('search-hit');
    void target.offsetWidth;
    target.classList.add('search-hit');
    marked = target;
    timer = window.setTimeout(() => target.classList.remove('search-hit'), 2400);
  };
  if (title) frame = requestAnimationFrame(find);
  return () => { cancelAnimationFrame(frame); window.clearTimeout(timer); marked?.classList.remove('search-hit'); };
}
