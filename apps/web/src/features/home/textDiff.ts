export interface TextChange { kind: 'same' | 'removed' | 'added'; text: string }

const tokens = (text: string) => text.match(/[\p{L}\p{N}\p{M}_]+|\s+|[^\s]/gu) ?? [];

/** Ordered, exact diff: punctuation, repeated words and both scripts matter.
 * Large unrelated texts fall back to whole changed spans, bounding UI work.
 * Filtering out additions/deletions reconstructs either original byte-for-byte.
 */
export function textDiff(before: string, after: string): TextChange[] {
  const a = tokens(before), b = tokens(after);
  const result: TextChange[] = [];
  const append = (kind: TextChange['kind'], text: string) => {
    if (!text) return;
    const last = result.at(-1);
    if (last?.kind === kind) last.text += text;
    else result.push({ kind, text });
  };
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length, endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) { endA--; endB--; }
  append('same', a.slice(0, start).join(''));
  const n = endA - start, m = endB - start;
  if ((n + 1) * (m + 1) > 1_000_000) {
    append('removed', a.slice(start, endA).join(''));
    append('added', b.slice(start, endB).join(''));
  } else {
    const width = m + 1;
    const lengths = new Uint32Array((n + 1) * width);
    for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) {
      lengths[i * width + j] = a[start + i] === b[start + j]
        ? 1 + lengths[(i + 1) * width + j + 1]
        : Math.max(lengths[(i + 1) * width + j], lengths[i * width + j + 1]);
    }
    let i = 0, j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && a[start + i] === b[start + j]) {
        append('same', a[start + i]); i++; j++;
      } else if (i < n && (j === m || lengths[(i + 1) * width + j] >= lengths[i * width + j + 1])) {
        append('removed', a[start + i++]);
      } else append('added', b[start + j++]);
    }
  }
  append('same', a.slice(endA).join(''));
  return result;
}
