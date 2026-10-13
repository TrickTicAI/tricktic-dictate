import type { VocabularyDocument } from '@metrix-writer/contracts';

export type Section = keyof VocabularyDocument;
export type RowIds = Record<Section, string[]>;
type Entry = VocabularyDocument[Section][number];
const sections: Section[] = ['hints', 'corrections', 'snippets'];
const equal = (a: Entry | undefined, b: Entry | undefined) => {
  const canonical = (entry: Entry | undefined) => entry && Object.entries(entry).sort(([a], [b]) => a.localeCompare(b));
  return JSON.stringify(canonical(a)) === JSON.stringify(canonical(b));
};

/** Adopt server-assigned IDs by the editor's stable row identity, keeping newer typing/deletions. */
export function adoptSavedIds(value: VocabularyDocument, rows: RowIds, sentRows: RowIds, saved: VocabularyDocument): VocabularyDocument {
  let result = value;
  for (const section of sections) {
    const assigned = new Map(sentRows[section].map((row, index) => [row, saved[section][index]?.id]));
    result = { ...result, [section]: value[section].map((entry, index) => {
      const id = assigned.get(rows[section][index]);
      return !entry.id && id ? { ...entry, id } : entry;
    }) };
  }
  return result;
}

export interface UnsavedChange {
  key: string;
  section: Section;
  id: string | null;
  before: Entry | undefined;
  desired: Entry | undefined;
  current: Entry | undefined;
  label: string;
  conflict: boolean;
}
export function describeEntry(entry: Entry | undefined): string {
  if (!entry) return 'Deleted';
  const text = 'from' in entry ? `${entry.from} → ${entry.to}` : 'phrase' in entry ? `${entry.phrase} → ${entry.text}` : entry.text;
  return `${text}${entry.enabled ? '' : ' (off)'}`;
}

/** Entry-level three-way comparison. Unrelated saved changes remain untouched. */
export function unsavedChanges(base: VocabularyDocument, draft: VocabularyDocument, current: VocabularyDocument): UnsavedChange[] {
  const changes: UnsavedChange[] = [];
  for (const section of sections) {
    const key = (entry: Entry, index: number) => entry.id ?? `${section}:${index}`;
    const before = new Map(base[section].map((entry, index) => [key(entry, index), entry]));
    const desired = new Map(draft[section].map((entry, index) => [key(entry, index), entry]));
    const saved = new Map(current[section].map((entry, index) => [key(entry, index), entry]));
    for (const identity of new Set([...before.keys(), ...desired.keys()])) {
      const old = before.get(identity), next = desired.get(identity), now = saved.get(identity);
      if (equal(old, next) || equal(next, now)) continue;
      const entry = next ?? old!;
      changes.push({ key: `${section}:${identity}`, section, id: entry.id ?? null, before: old, desired: next, current: now,
        label: String('from' in entry ? entry.from : 'phrase' in entry ? entry.phrase : entry.text),
        conflict: !equal(old, now) });
    }
  }
  return changes;
}

/** Called only after the user reviews the retained changes and chooses every overlap. */
export function applyUnsavedChanges(current: VocabularyDocument, changes: UnsavedChange[], choices: Record<string, string>): VocabularyDocument {
  let result = current;
  for (const change of changes) {
    if (change.conflict && choices[change.key] !== 'mine') continue;
    const entries: Entry[] = result[change.section];
    const index = change.id ? entries.findIndex((entry) => entry.id === change.id) : change.current ? entries.indexOf(change.current) : -1;
    // A deletion already synced is a tombstone. Explicitly recreating the entry needs a fresh identity.
    const desired = index < 0 && change.before && change.desired ? { ...change.desired, id: crypto.randomUUID() } : change.desired;
    const next = index < 0 ? (desired ? [...entries, desired] : entries)
      : entries.flatMap((entry, at) => at !== index ? [entry] : desired ? [desired] : []);
    result = { ...result, [change.section]: next };
  }
  return result;
}
