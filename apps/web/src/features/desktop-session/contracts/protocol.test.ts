import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { acceptsLevel, canTransition, parseMessage } from './protocol.ts';
import type { Level, Snapshot, SessionState } from './protocol.ts';
const fixtures = JSON.parse(readFileSync(new URL('./fixtures.json', import.meta.url), 'utf8'));
for (const fixture of fixtures.messages) test(`wire: ${fixture.name}`, () => {
  if (!fixture.valid) assert.throws(() => parseMessage(fixture.json));
  else assert.deepEqual(parseMessage(JSON.stringify(parseMessage(fixture.json))), JSON.parse(fixture.json));
});
test('all phase transitions agree with the frozen matrix', () => {
  for (const f of fixtures.transitions) assert.equal(canTransition(f.before as SessionState, f.after as SessionState), f.allowed, `${f.before} -> ${f.after}`);
});
test('telemetry cannot cross sessions, epochs, phase revisions or move backwards', () => {
  const snapshot = fixtures.snapshot as Snapshot, level = fixtures.level as Level;
  assert.equal(acceptsLevel(snapshot, level, 0), true);
  assert.equal(acceptsLevel(snapshot, level, 1), false);
  assert.equal(acceptsLevel({ ...snapshot, state: 'saving' }, level, 0), false);
  for (const change of [{ epoch: '44444444-4444-4444-8444-444444444444' }, { session_id: '44444444-4444-4444-8444-444444444444' }, { revision: 1 }]) {
    assert.equal(acceptsLevel(snapshot, { ...level, ...change }, 0), false);
  }
});
