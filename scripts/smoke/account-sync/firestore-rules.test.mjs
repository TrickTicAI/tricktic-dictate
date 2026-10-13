// Firestore emulator rules and protocol tests (MW-OUATH-5). Run through
// firestore-rules.mjs. Honest clients use the reference planner in a
// transaction; "forced" writes bypass it to prove the rules hold on their own.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { beforeEach, test } from 'node:test';
import { planCleanup } from '../../../packages/contracts/account-sync.cleanup.mjs';
import { accountDocument, seedDocuments } from '../../../packages/contracts/account-sync.firestore.fixtures.mjs';
import { initialAccount, nextConsent, paths, planOperation, recordDocument }
  from '../../../packages/contracts/account-sync.firestore.mjs';
import { evaluate, referenceState } from '../../../packages/contracts/account-sync.reference.mjs';
import { settingIds } from '../../../packages/contracts/account-sync.schema.mjs';
import { clear, commit, Denied, getDocument, list, projectId, seed, transact } from './firestore-rest.mjs';

const scenarios = JSON.parse(readFileSync(new URL('../../../docs/testing/account-sync/conflicts.json',
  import.meta.url), 'utf8'));
const A = 'synthetic-user-a';
const B = 'synthetic-user-b';
const id = n => `11111111-1111-4111-8111-${String(n).padStart(12, '0')}`;
const DEVICE = id(1);
const ENROLLMENT = id(60);
const model = { provider: 'openai', model: 'gpt-4o-transcribe' };
let nextOperation = 1000;

const denied = (promise, message) =>
  assert.rejects(promise, error => error instanceof Denied && error.status === 403, message);
const owner = path => getDocument(path, 'owner');
const storedRecords = async uid => (await list(paths.account(uid), 'records', 'owner', 500))
  .map(({ operation_id, consent_generation, ...record }) => record).sort((a, b) => a.id.localeCompare(b.id));
const submit = (op, auth = A, attemptEnrolled = true) =>
  transact(auth, read => planOperation({ authUid: auth, op, read, attemptEnrolled }));

// The writes an acknowledgement would need, built without any of the planner's checks
// (including its path validation).
const raw = (uid, collection, key) => `users/${uid}/${collection}/${key}`;
async function forcedWrites(uid, op) {
  const revision = op.base_revision + 1;
  const writes = [{ path: raw(uid, 'records', op.record_id), data: recordDocument(op, revision) },
    { path: raw(uid, 'receipts', op.operation_id), data: { operation: op, revision } }];
  const data = op.content.state === 'live' ? op.content.data : null;
  if (op.category !== 'transcripts') return writes;
  if (data?.kind === 'original') {
    writes.push({ path: raw(uid, 'attempts', data.attempt_id), data: { original_record_id: op.record_id } },
      { path: raw(uid, 'families', op.record_id), data: { member_ids: [], deleted: false } });
  } else if (data) {
    const family = await owner(raw(uid, 'families', data.original_record_id));
    if (family) writes.push({ path: raw(uid, 'families', data.original_record_id),
      data: { member_ids: [...family.member_ids, op.record_id], deleted: false } });
  } else {
    const family = await owner(raw(uid, 'families', op.record_id));
    if (family) writes.push({ path: raw(uid, 'families', op.record_id), data: { ...family, deleted: true } });
  }
  return writes;
}
const force = async (op, auth = A, uid = A) => commit(await forcedWrites(uid, op), auth);

function operation(category, recordId, content, base = 0, overrides = {}) {
  return { schema_version: 1, uid: A, account_epoch: 1, consent_generation: 1, category,
    operation_id: id(nextOperation++), device_id: DEVICE, record_id: recordId, origin_device_id: DEVICE,
    base_revision: base, ...(category === 'transcripts' && { history_enrollment_id: ENROLLMENT }),
    content, ...overrides };
}
const live = data => ({ state: 'live', data });
const deleted = { state: 'deleted' };
const hint = text => live({ kind: 'hint', enabled: true, text });
const setting = (key, value) => live({ kind: 'setting', key, value });
const original = (attempt, text = 'שלום world') => live({ kind: 'original', attempt_id: attempt,
  recorded_at_ms: 1_790_000_000_000, input_language: 'mixed', output_language: { mode: 'preserve' },
  selection: model, text });
const derived = (kind, originalId, parentId, text) => live({ kind, original_record_id: originalId,
  parent_record_id: parentId, created_at_ms: 1_790_000_000_001, text });

function activeAccount(uid = A) {
  const account = initialAccount(uid);
  for (const category of ['settings', 'dictionary']) account.consent[category] = { enabled: true, generation: 1 };
  account.consent.transcripts = { enabled: true, generation: 1,
    history: { mode: 'future_only', enrollment_id: ENROLLMENT } };
  return account;
}
const seedActive = (uid = A) => seed(new Map([[paths.account(uid), activeAccount(uid)]]));

beforeEach(clear);

for (const scenario of scenarios) {
  test(`vector: ${scenario.name}`, async () => {
    const { initial } = scenario;
    const uid = scenario.steps[0].operation.uid;
    const category = scenario.steps[0].operation.category;
    await seed(seedDocuments(initial, uid, category));
    const oracle = referenceState(initial);
    // Device history enrollment is a client fact Firestore cannot observe.
    const deviceOnly = !initial.history_enrolled;
    for (const { operation: op, expected, authority_patch } of scenario.steps) {
      if (authority_patch) {
        Object.assign(oracle, authority_patch);
        await seed(new Map([[paths.account(uid), accountDocument(oracle, uid, category)]]));
      }
      assert.deepEqual(evaluate(oracle, op), expected);
      const outcome = await submit(op, initial.uid, initial.history_enrolled);
      const { status, revision } = outcome;
      assert.deepEqual(revision === undefined ? { status } : { status, revision }, expected);
      if (outcome.writes.length) continue;
      if (deviceOnly) await force(op, initial.uid, uid);
      else await denied(force(op, initial.uid, uid));
    }
    if (!deviceOnly) {
      assert.deepEqual(await storedRecords(uid), oracle.records.toSorted((a, b) => a.id.localeCompare(b.id)));
    }
  });
}

test('unauthenticated requests are denied for every path', async () => {
  await seedActive();
  const op = operation('dictionary', id(10), hint('word'));
  await submit(op);
  for (const path of [paths.account(A), paths.record(A, id(10)), paths.receipt(A, op.operation_id),
    paths.cleanupJob(A, id(50)), paths.device(A, DEVICE)]) await denied(getDocument(path, null));
  await denied(list(paths.account(A), 'records', null, 10));
  await denied(commit([{ path: paths.account('synthetic-user-c'), data: initialAccount('synthetic-user-c') }], null));
  await denied(force(operation('dictionary', id(11), hint('word')), null));
});

test('another signed-in user cannot read or write the account', async () => {
  await seedActive();
  const op = operation('dictionary', id(10), hint('word'));
  await submit(op);
  for (const path of [paths.account(A), paths.record(A, id(10)), paths.receipt(A, op.operation_id),
    paths.family(A, id(20)), paths.attempt(A, id(30)), paths.cleanupJob(A, id(50))]) {
    await denied(getDocument(path, B));
  }
  for (const collection of ['records', 'families', 'devices']) await denied(list(paths.account(A), collection, B, 10));
  await denied(force(operation('dictionary', id(11), hint('word')), B));
  // Envelope identity is not authentication: B cannot store A's envelope in B's tree either.
  await seedActive(B);
  await denied(force(operation('dictionary', id(12), hint('word')), B, B));
  await denied(commit([{ path: paths.account('synthetic-user-c'), data: initialAccount('synthetic-user-c') }], B));
});

test('account creation is all-off and consent never lets a client pick a generation or epoch', async () => {
  await denied(commit([{ path: paths.account(A), data: activeAccount() }], A));
  await denied(commit([{ path: paths.account(A), data: { ...initialAccount(A), resets: {} } }], A));
  await commit([{ path: paths.account(A), data: initialAccount(A) }], A);
  const update = consent => commit([{ path: paths.account(A),
    data: { ...initialAccount(A), consent } }], A);
  const start = initialAccount(A).consent;
  const on = nextConsent(start, 'dictionary', true);
  await denied(update({ ...on, dictionary: { enabled: true, generation: 1 } }));
  await denied(update({ ...on, dictionary: { enabled: true, generation: 3 } }));
  await denied(update({ ...on, revision: 3 }));
  await denied(update({ ...on, uid: B }));
  await denied(update({ ...on, account_epoch: 2 }));
  await denied(update({ ...on, transcripts: { enabled: true, generation: 2, history: { mode: 'off' } } }));
  await denied(commit([{ path: paths.account(A), data: { ...initialAccount(A), consent: on, account_epoch: 2 } }], A));
  await denied(commit([{ path: paths.account(A), data: { ...initialAccount(A), consent: on, account_state: 'deleting' } }], A));
  await denied(commit([{ path: paths.account(A), data: { ...initialAccount(A), consent: on, resets: {} } }], A));
  await update(on);
  const history = { mode: 'future_only', enrollment_id: ENROLLMENT };
  const withHistory = nextConsent(on, 'transcripts', true, history);
  await update(withHistory);
  // A changed history selection needs a fresh enrollment ID.
  const reused = nextConsent(withHistory, 'transcripts', true, { ...history, mode: 'include_existing' });
  await denied(update(reused));
  await update({ ...reused, transcripts: { ...reused.transcripts, history: { ...reused.transcripts.history,
    enrollment_id: id(61) } } });
  assert.equal((await owner(paths.account(A))).consent.transcripts.generation, 3);
});

test('records and receipts commit only together, once, and are never deleted by clients', async () => {
  await seedActive();
  const op = operation('dictionary', id(10), hint('word'));
  const [record, receipt] = await forcedWrites(A, op);
  await denied(commit([record], A));
  await denied(commit([receipt], A));
  await denied(commit([record, { ...receipt, data: { ...receipt.data, revision: 2 } }], A));
  await denied(commit([{ ...record, data: { ...record.data, revision: 2 } },
    { ...receipt, data: { ...receipt.data, revision: 2 } }], A));
  await commit([record, receipt], A);
  await denied(commit([receipt], A));
  await denied(commit([{ path: record.path, remove: true }], A));
  await denied(commit([{ path: receipt.path, remove: true }], A));
  // A later write must bring its own fresh receipt.
  await denied(commit([{ path: record.path, data: { ...record.data, revision: 2, content: hint('new') } }], A));
  assert.equal((await owner(record.path)).revision, 1);
});

test('closed shapes reject sensitive, malformed and unknown-version data', async () => {
  await seedActive();
  const settingOp = (key, value, recordId = settingIds[key]) => operation('settings', recordId, setting(key, value));
  const polish = { instructions: 'Synthetic polish instructions.', formatting: { rewrite: 'light',
    layout: 'single_line', lists: false, remove_fillers: true, self_corrections: false, spoken_punctuation: true } };
  const mutated = [
    operation('dictionary', id(10), live({ kind: 'hint', enabled: true, text: 'word', api_key: 'synthetic' })),
    { ...operation('dictionary', id(10), hint('word')), audio: 'synthetic' },
    operation('dictionary', id(10), hint('word'), 0, { schema_version: 2 }),
    operation('dictionary', id(10), { state: 'pending', data: { kind: 'hint', enabled: true, text: 'word' } }),
    operation('dictionary', id(10), live({ kind: 'hint', enabled: 'yes', text: 'word' })),
    operation('dictionary', id(10), hint('   ')),
    operation('dictionary', id(10), hint(' 　 ')),
    operation('dictionary', id(10), hint('')),
    operation('dictionary', id(10), hint('a\u0000b')),
    operation('dictionary', settingIds.theme, hint('word')),
    operation('dictionary', 'ABCDEF00-1111-4111-8111-000000000010', hint('word')),
    operation('dictionary', id(10), hint('word'), 0, { device_id: 'laptop-hostname' }),
    operation('dictionary', id(10), hint('word'), 0, { history_enrollment_id: ENROLLMENT }),
    settingOp('theme', 'dark', settingIds.ui_language),
    settingOp('theme', 'dark', id(10)),
    settingOp('ui_language', 'fr'),
    settingOp('transcription_model', { provider: 'openai', model: '../x' }),
    settingOp('transcription_model', { provider: 'openai', model: 'a//b' }),
    settingOp('transcription_model', { provider: 'openai', model: 'a/' }),
    settingOp('transcription_model', { provider: 'azure', model: 'x' }),
    settingOp('cleanup', { mode: 'enabled', selection: model, reasoning: 'max' }),
    settingOp('output_language', { mode: 'translate', language: 'fr' }),
    settingOp('polish_instructions', { ...polish, instructions: '' }),
    settingOp('polish_instructions', { ...polish, instructions: '   ' }),
    settingOp('polish_instructions', { ...polish, instructions: 'ok' + String.fromCharCode(0) }),
    settingOp('polish_instructions', { ...polish, formatting: { ...polish.formatting, rewrite: 'flowery' } }),
    settingOp('polish_instructions', { ...polish, formatting: { ...polish.formatting, lists: 'yes' } }),
    settingOp('polish_instructions', { ...polish,
      formatting: (({ layout, ...rest }) => rest)(polish.formatting) }),
    settingOp('polish_instructions', { ...polish, prompt_versions: [] }),
    settingOp('polish_instructions', polish, settingIds.theme),
    (({ history_enrollment_id, ...op }) => op)(operation('transcripts', id(20), original(id(30)))),
    operation('transcripts', id(20), live({ ...original(id(30)).data, audio_uri: 'file:///synthetic' })),
  ];
  for (const op of mutated) await denied(force(op), JSON.stringify(op));
  const [record, receipt] = await forcedWrites(A, operation('dictionary', id(10), hint('word')));
  await denied(commit([{ ...record, data: { ...record.data, audio: 'synthetic' } }, receipt], A));
  await denied(commit([record, { ...receipt, data: { ...receipt.data, access_token: 'synthetic' } }], A));
  // Controls: the same shapes are accepted when valid, including Hebrew and bidi text.
  for (const op of [operation('dictionary', id(10), hint('‏עברית‎')),
    settingOp('transcription_model', model), settingOp('polish_instructions', polish),
    settingOp('output_language', { mode: 'translate', language: 'he' }),
    settingOp('cleanup', { mode: 'enabled', selection: { provider: 'local', model: 'whisper/large-v3' },
      reasoning: 'low' }),
    operation('transcripts', id(20), original(id(30), 'é / é 😀\nעברית'))]) {
    assert.equal((await submit(op)).status, 'acknowledged', JSON.stringify(op));
  }
});

test('size limits: UTF-8 bytes exactly, code points bounded, no truncation', async () => {
  await seedActive();
  assert.equal((await submit(operation('dictionary', id(10), hint('א'.repeat(4096))))).status, 'acknowledged');
  await denied(force(operation('dictionary', id(11), hint('א'.repeat(4097)))));
  await denied(force(operation('dictionary', id(11), live({ kind: 'correction', enabled: true,
    from: 'x', to: 'x'.repeat(8193) }))));
  // 131072 astral code points = 524288 UTF-8 bytes: the contract maximum.
  const emoji = '😀'.repeat(131072);
  assert.equal((await submit(operation('transcripts', id(20), original(id(30), emoji)))).status, 'acknowledged');
  await denied(force(operation('transcripts', id(21), original(id(31), `${emoji}a`))));
  await denied(force(operation('transcripts', id(21), original(id(31), 'a'.repeat(262145)))));
  await denied(force(operation('transcripts', id(22), derived('edited', id(20), id(20), `${emoji}a`))));
  assert.equal((await owner(paths.record(A, id(20)))).content.data.text, emoji);
});

test('conditional revisions: concurrent writers at one base, exactly one wins', async () => {
  await seedActive();
  assert.equal((await submit(operation('dictionary', id(10), hint('v1')))).status, 'acknowledged');
  const first = operation('dictionary', id(10), hint('from laptop'), 1);
  const second = operation('dictionary', id(10), hint('from phone'), 1);
  const results = await Promise.allSettled([force(first), force(second)]);
  assert.deepEqual(results.map(result => result.status).sort(), ['fulfilled', 'rejected']);
  const loser = results[0].status === 'rejected' ? first.content : second.content;
  assert(results.find(result => result.status === 'rejected').reason instanceof Denied);
  const stored = await owner(paths.record(A, id(10)));
  assert.equal(stored.revision, 2);
  assert.notDeepEqual(stored.content, loser);
  // The honest loser sees a conflict carrying the current record; its local value stays with it.
  const retry = await submit(operation('dictionary', id(10), hint('retry'), 1));
  assert.equal(retry.status, 'conflict');
  assert.deepEqual(retry.current.content, stored.content);
  await denied(force(operation('dictionary', id(12), hint('x'), 3)));
});

test('tombstones are final; a fixed setting returns only in a newer consent generation', async () => {
  await seedActive();
  const ui = settingIds.ui_language;
  await submit(operation('settings', ui, setting('ui_language', 'he')));
  await submit(operation('dictionary', id(10), hint('word')));
  assert.equal((await submit(operation('settings', ui, deleted, 1))).status, 'acknowledged');
  assert.equal((await submit(operation('dictionary', id(10), deleted, 1))).status, 'acknowledged');
  for (const [op, status] of [[operation('settings', ui, setting('ui_language', 'en'), 2), 'deleted'],
    [operation('settings', ui, deleted, 2), 'deleted'], [operation('dictionary', id(10), hint('back'), 2), 'deleted']]) {
    assert.equal((await submit(op)).status, status);
    await denied(force(op));
  }
  // Explicit opt-out then opt-in confirms a new generation (1 -> 2 -> 3).
  const account = await owner(paths.account(A));
  const off = nextConsent(account.consent, 'settings', false);
  await commit([{ path: paths.account(A), data: { ...account, consent: off } }], A);
  const on = nextConsent(off, 'settings', true);
  await commit([{ path: paths.account(A), data: { ...account, consent: on } }], A);
  await denied(force(operation('settings', ui, setting('ui_language', 'en'), 2)));
  const recreate = operation('settings', ui, setting('ui_language', 'en'), 2, { consent_generation: 3 });
  assert.deepEqual((({ status, revision }) => ({ status, revision }))(await submit(recreate)),
    { status: 'acknowledged', revision: 3 });
  const dictionaryBack = operation('dictionary', id(10), hint('back'), 2);
  assert.equal((await submit(dictionaryBack)).status, 'deleted');
  await denied(force(dictionaryBack));
});

test('transcript families: immutable text, live parents, fenced deletion and cascades', async () => {
  await seedActive();
  const [O, D1, D2, O2] = [id(20), id(21), id(22), id(23)];
  for (const op of [operation('transcripts', O, original(id(30))),
    operation('transcripts', D1, derived('cleaned', O, O, 'cleaned text')),
    operation('transcripts', D2, derived('edited', O, D1, 'edited text')),
    operation('transcripts', O2, original(id(31)))]) assert.equal((await submit(op)).status, 'acknowledged');
  assert.deepEqual(await owner(paths.family(A, O)), { member_ids: [D1, D2], deleted: false });
  const rejected = [
    [operation('transcripts', O, original(id(30), 'rewritten'), 1), 'immutable'],
    [operation('transcripts', D1, derived('cleaned', O, O, 'rewritten'), 1), 'immutable'],
    [operation('transcripts', id(24), original(id(30))), 'invalid_record'],
    [operation('transcripts', id(24), derived('edited', O, O2, 'cross-family parent')), 'invalid_record'],
    [operation('transcripts', id(24), derived('edited', O2, D1, 'cross-family parent')), 'invalid_record'],
    [operation('transcripts', id(24), derived('edited', id(99), id(99), 'no original')), 'invalid_record'],
  ];
  for (const [op, status] of rejected) {
    assert.equal((await submit(op)).status, status);
    await denied(force(op));
  }
  // The family index cannot be edited apart from a matching record write.
  await denied(commit([{ path: paths.family(A, O), data: { member_ids: [D1, D2, id(98)], deleted: false } }], A));
  await denied(commit([{ path: paths.family(A, O), data: { member_ids: [D1], deleted: false } }], A));
  await denied(commit([{ path: paths.attempt(A, id(32)), data: { original_record_id: O2 } }], A));
  // A live member of a live family cannot be cascade-tombstoned.
  const member = await owner(paths.record(A, D1));
  const cascade = { path: paths.record(A, D1),
    data: { ...member, revision: member.revision + 1, content: deleted } };
  await denied(commit([cascade], A));

  const removal = await submit(operation('transcripts', O, deleted, 1));
  assert.equal(removal.status, 'acknowledged');
  for (const memberId of [D1, D2]) {
    assert.deepEqual((({ revision, content }) => ({ revision, content }))(await owner(paths.record(A, memberId))),
      { revision: 2, content: deleted });
  }
  assert.deepEqual(await owner(paths.family(A, O)), { member_ids: [D1, D2], deleted: true });
  await denied(commit([{ path: paths.family(A, O), data: { member_ids: [D1, D2], deleted: false } }], A));
  const late = operation('transcripts', id(25), derived('edited', O, O, 'late offline edit'));
  assert.equal((await submit(late)).status, 'deleted');
  await denied(force(late));
  assert.equal((await submit(operation('transcripts', id(26), original(id(30))))).status, 'invalid_record');
});

test('a fenced family can be repaired later; rules cannot force every cascade into one commit', async () => {
  await seedActive();
  const [O, D] = [id(20), id(21)];
  await submit(operation('transcripts', O, original(id(30))));
  await submit(operation('transcripts', D, derived('edited', O, O, 'edited')));
  // A faulty client fences the family but omits the member tombstone.
  await force(operation('transcripts', O, deleted, 1));
  const member = await owner(paths.record(A, D));
  assert.equal(member.content.state, 'live');
  await denied(commit([{ path: paths.record(A, D), data: { ...member, revision: member.revision + 1,
    content: deleted, origin_device_id: id(2) } }], A));
  await commit([{ path: paths.record(A, D), data: { ...member, revision: member.revision + 1, content: deleted } }], A);
  assert.equal((await owner(paths.record(A, D))).content.state, 'deleted');
});

test('collection reads must be bounded', async () => {
  await seedActive();
  await submit(operation('dictionary', id(10), hint('word')));
  assert.equal((await list(paths.account(A), 'records', A, 500)).length, 1);
  await denied(list(paths.account(A), 'records', A));
  await denied(list(paths.account(A), 'records', A, 501));
  await denied(list(paths.account(A), 'receipts', A, 10));
});

test('cleanup fences: stale devices cannot write after category or account deletion', async () => {
  await seedActive();
  await submit(operation('dictionary', id(10), hint('word')));
  const account = await owner(paths.account(A));
  const now = Math.floor(Date.now() / 1000);
  const claims = { aud: projectId, iss: `https://securetoken.google.com/${projectId}`, sub: A, auth_time: now,
    firebase: { sign_in_provider: 'google.com' } };
  const request = { schema_version: 1, uid: A, account_epoch: 1, operation_id: id(50),
    expected_consent_revision: account.consent.revision, scope: { kind: 'categories', categories: ['dictionary'] },
    effect: 'everywhere' };
  const category = planCleanup({ projectId, claims, request, account, jobId: id(51), nowSeconds: now });
  await denied(commit(category.writes, A));
  await commit(category.writes, 'owner');
  assert.deepEqual((await getDocument(paths.account(A), A)).resets, { dictionary: { generation: 2, job_id: id(51) } });
  assert.equal((await getDocument(paths.cleanupJob(A, id(50)), A)).receipt.state, 'pending');
  await denied(getDocument(paths.cleanupJob(A, id(50)), B));
  await denied(commit([{ path: paths.cleanupJob(A, id(52)), data: { receipt: {} } }], A));
  const stale = operation('dictionary', id(10), deleted, 1);
  assert.equal((await submit(stale)).status, 'stale_generation');
  await denied(force(stale));
  assert.equal((await submit(operation('settings', settingIds.theme, setting('theme', 'dark')))).status,
    'acknowledged');

  const fenced = await owner(paths.account(A));
  const deletion = planCleanup({ projectId, claims, account: fenced, jobId: id(54), nowSeconds: now,
    request: { ...request, operation_id: id(53), expected_consent_revision: fenced.consent.revision,
      scope: { kind: 'account' }, effect: 'cloud_only' } });
  await commit(deletion.writes, 'owner');
  assert.equal((await getDocument(paths.account(A), A)).account_state, 'deleting');
  const late = operation('settings', settingIds.theme, setting('theme', 'light'), 1);
  assert.equal((await submit(late)).status, 'account_unavailable');
  await denied(force(late));
  await denied(force(operation('settings', settingIds.theme, setting('theme', 'light'), 1,
    { account_epoch: 2, consent_generation: 2 })));
  const current = await owner(paths.account(A));
  await denied(commit([{ path: paths.account(A), data: { ...current,
    consent: nextConsent(current.consent, 'settings', true) } }], A));
  await denied(commit([{ path: paths.device(A, DEVICE), data: { schema_version: 1, uid: A, account_epoch: 2,
    device_id: DEVICE, reconciled_generations: { settings: 1, dictionary: 1, transcripts: 1 } } }], A));
});

test('device acknowledgements are closed, bounded by consent and monotonic', async () => {
  await seedActive();
  const account = await owner(paths.account(A));
  const off = nextConsent(account.consent, 'dictionary', false);
  await commit([{ path: paths.account(A), data: { ...account, consent: off } }], A);
  await commit([{ path: paths.account(A), data: { ...account, consent: nextConsent(off, 'dictionary', true) } }], A);
  const device = (seen, overrides = {}) => ({ path: paths.device(A, DEVICE), data: { schema_version: 1, uid: A,
    account_epoch: 1, device_id: DEVICE, reconciled_generations: { settings: 1, dictionary: seen, transcripts: 1 },
    ...overrides } });
  await commit([device(2)], A);
  await denied(commit([device(1)], A));
  await denied(commit([device(4)], A));
  await denied(commit([device(3, { account_epoch: 2 })], A));
  await denied(commit([device(3, { device_id: id(2) })], A));
  await denied(commit([device(3, { hostname: 'synthetic-laptop' })], A));
  await denied(commit([device(3)], B));
  await commit([device(3)], A);
  await commit([{ path: paths.device(A, DEVICE), remove: true }], A);
});
