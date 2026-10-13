import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { evaluate, referenceState } from './account-sync.reference.mjs';
import { checkedUid, initialAccount, nextConsent, paths, planOperation } from './account-sync.firestore.mjs';
import { remoteRecords, seedDocuments } from './account-sync.firestore.fixtures.mjs';

const scenarios = JSON.parse(readFileSync(new URL('../../docs/testing/account-sync/conflicts.json',
  import.meta.url), 'utf8'));

// The planner is the Firestore-shaped form of the oracle: same outcomes, same
// resulting records, reading only the documents the rules can also see.
for (const scenario of scenarios) {
  test(`planner matches oracle: ${scenario.name}`, async () => {
    const uid = scenario.steps[0].operation.uid;
    const category = scenario.steps[0].operation.category;
    const documents = seedDocuments(scenario.initial, uid, category);
    const oracle = referenceState(scenario.initial);
    for (const { operation, expected, authority_patch } of scenario.steps) {
      if (authority_patch) {
        Object.assign(oracle, authority_patch);
        documents.set(paths.account(uid), seedDocuments(oracle, uid, category).get(paths.account(uid)));
      }
      const outcome = await planOperation({ authUid: scenario.initial.uid, op: operation,
        read: async path => structuredClone(documents.get(path) ?? null),
        attemptEnrolled: scenario.initial.history_enrolled });
      assert.deepEqual(evaluate(oracle, operation), expected);
      const { status, revision } = outcome;
      assert.deepEqual(revision === undefined ? { status } : { status, revision }, expected);
      if (status !== 'acknowledged') assert.deepEqual(outcome.writes, []);
      for (const { path, data } of outcome.writes) documents.set(path, data);
    }
    assert.deepEqual(remoteRecords(documents, uid),
      oracle.records.toSorted((a, b) => a.id.localeCompare(b.id)));
  });
}

test('consent changes advance generations; the client never selects one', () => {
  const account = initialAccount('synthetic-user-a');
  const enrolled = { mode: 'future_only', enrollment_id: '11111111-1111-4111-8111-000000000060' };
  const on = nextConsent(account.consent, 'transcripts', true, enrolled);
  assert.deepEqual([on.revision, on.transcripts.generation, on.settings.generation], [2, 2, 1]);
  const same = nextConsent(on, 'transcripts', true, enrolled);
  assert.equal(same.transcripts.generation, 2);
  const reselected = nextConsent(on, 'transcripts', true, { ...enrolled, mode: 'include_existing' });
  assert.equal(reselected.transcripts.generation, 3);
  const off = nextConsent(on, 'transcripts', false);
  assert.deepEqual(off.transcripts, { enabled: false, generation: 3, history: { mode: 'off' } });
});

test('paths refuse UIDs and IDs that could escape the per-user root', () => {
  for (const uid of ['', 'a/b', '..', '.', '__admin__', 'x'.repeat(129), null]) {
    assert.throws(() => checkedUid(uid));
  }
  assert.throws(() => paths.record('synthetic-user-a', '../receipts/x'));
  assert.equal(paths.record('synthetic-user-a', '00000000-0000-4000-8000-000000000001'),
    'users/synthetic-user-a/records/00000000-0000-4000-8000-000000000001');
});
