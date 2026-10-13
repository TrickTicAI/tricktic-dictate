import { expect, test } from '@playwright/test';
import { textDiff } from '../textDiff';

for (const [before, after] of [
  ['Do not send it.', 'Do send it.'],
  ['לא לשלוח את ה-report.', 'לשלוח את ה-report.'],
  ['The change is -5.', 'The change is 5.'],
  ['one one two', 'one two'],
  ['alpha beta', 'beta alpha'],
  ['1:30', '1.30'],
  ['Hello,\nעולם!', 'hello עולם.'],
  ['', 'שלום'], ['שלום', ''], ['a\u0301 😀', 'a 😀'],
  ['same '.repeat(3000) + 'no '.repeat(2000), 'same '.repeat(3000) + 'yes '.repeat(2000)],
]) test(`exact reconstruction: ${before.slice(0, 24)} → ${after.slice(0, 24)}`, () => {
  const changes = textDiff(before, after);
  expect(changes.filter(c => c.kind !== 'added').map(c => c.text).join('')).toBe(before);
  expect(changes.filter(c => c.kind !== 'removed').map(c => c.text).join('')).toBe(after);
  expect(changes.some(c => c.kind !== 'same')).toBe(true);
});

test('deletions are visible even when every remaining word was already present', () => {
  expect(textDiff('Do not send it.', 'Do send it.').filter(c => c.kind === 'removed').map(c => c.text).join('')).toBe('not ');
  expect(textDiff('The change is -5.', 'The change is 5.').filter(c => c.kind === 'removed').map(c => c.text).join('')).toBe('-');
  expect(textDiff('unchanged שלום', 'unchanged שלום')).toEqual([{ kind: 'same', text: 'unchanged שלום' }]);
});
