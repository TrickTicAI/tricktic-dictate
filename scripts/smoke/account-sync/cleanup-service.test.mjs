// Trusted cleanup service tests (MW-OUATH-8). Run through cleanup-service.mjs:
// real Firestore and Auth emulators, real HTTP, synthetic Google users. Client
// writes go through the Security Rules; the service uses the Admin SDK.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { afterEach, beforeEach, test } from 'node:test';
import { initialAccount, paths, planOperation, recordDocument }
  from '../../../packages/contracts/account-sync.firestore.mjs';
import { settingIds } from '../../../packages/contracts/account-sync.schema.mjs';
import { createService } from '../../../infra/firebase/cleanup-service/src/service.mjs';
import { backoff } from '../../../infra/firebase/cleanup-service/src/jobs.mjs';
import { clear, commit, Denied, projectId, transact } from './firestore-rest.mjs';

const require = createRequire(new URL('../../../infra/firebase/cleanup-service/package.json', import.meta.url));
const { initializeApp } = require('firebase-admin/app');
const { getAuth } = require('firebase-admin/auth');
const { getFirestore } = require('firebase-admin/firestore');

const authHost = process.env.FIREBASE_AUTH_EMULATOR_HOST;
if (!authHost || !projectId.startsWith('demo-')) throw new Error('Run through cleanup-service.mjs');
const app = initializeApp({ projectId }, 'cleanup-service-test');
const admin = getAuth(app);
// Emulator limitation: its verifyIdToken always looks the user up, even without
// checkRevoked, while production then checks only signature and expiry. For
// that one call this stand-in does what production does with an unsigned
// emulator token: decode, then check expiry, audience and issuer.
const auth = Object.create(admin, { verifyIdToken: { value: async (token, checkRevoked) => {
  try {
    return await admin.verifyIdToken(token, checkRevoked);
  } catch (error) {
    if (checkRevoked || error.code !== 'auth/user-not-found') throw error;
    const claims = JSON.parse(Buffer.from(token.split('.')[1], 'base64url').toString('utf8'));
    if (!(claims.exp > Date.now() / 1000) || claims.aud !== projectId ||
        claims.iss !== `https://securetoken.google.com/${projectId}`) throw Object.assign(new Error(), { code: 'auth/argument-error' });
    return { ...claims, uid: claims.sub };
  }
} } });
const db = getFirestore(app);

const id = n => `11111111-1111-4111-8111-${String(n).padStart(12, '0')}`;
const DEVICE = id(1);
const ENROLLMENT = id(60);
const model = { provider: 'openai', model: 'gpt-4o-transcribe' };
let nextOperation = 1000;
let clock = 0;
let fault = null;
let service;
let server;
let base;

async function start(options = {}) {
  service = createService({ projectId, auth, db, now: () => Date.now() + clock, log: () => {},
    faults: { afterBatch: () => fault?.() }, ...options });
  server = createServer(service.handle);
  await new Promise(done => server.listen(0, '127.0.0.1', done));
  base = `http://127.0.0.1:${server.address().port}`;
}
beforeEach(async () => {
  clock = 0;
  fault = null;
  await clear();
  const reset = await fetch(`http://${authHost}/emulator/v1/projects/${projectId}/accounts`, { method: 'DELETE' });
  assert.equal(reset.ok, true);
  await start();
});
afterEach(() => new Promise(done => server.close(done)));

// A Google sign-in on the Auth emulator; its unsigned JSON ID token is accepted there only.
async function signIn(subject, provider = 'google.com') {
  const identity = `${authHost}/identitytoolkit.googleapis.com/v1`;
  const response = provider === 'anonymous'
    ? await fetch(`http://${identity}/accounts:signUp?key=demo-key`, { method: 'POST',
      headers: { 'content-type': 'application/json' }, body: JSON.stringify({ returnSecureToken: true }) })
    : await fetch(`http://${identity}/accounts:signInWithIdp?key=demo-key`, { method: 'POST',
      headers: { 'content-type': 'application/json' }, body: JSON.stringify({ requestUri: 'http://localhost',
        returnSecureToken: true, postBody: `id_token=${encodeURIComponent(JSON.stringify(
          { sub: subject, email: `${subject}@example.invalid`, email_verified: true }))}&providerId=google.com` }) });
  const body = await response.json();
  assert.equal(response.status, 200, JSON.stringify(body));
  return { uid: body.localId, token: body.idToken, refresh: body.refreshToken };
}

async function call(method, path, { token, body, headers = {} } = {}) {
  const response = await fetch(`${base}${path}`, { method, headers: {
    ...(body !== undefined && { 'content-type': 'application/json' }),
    ...(token && { authorization: `Bearer ${token}` }), ...headers },
  body: body === undefined ? undefined : typeof body === 'string' ? body : JSON.stringify(body) });
  return { status: response.status, body: await response.json() };
}
const cleanup = (user, request) => call('POST', '/v1/cleanup', { token: user.token, body: request });
const status = (user, operationId) => call('GET', `/v1/cleanup/${operationId}`, { token: user.token });
const request = (user, overrides = {}) => ({ schema_version: 1, uid: user.uid, account_epoch: 1,
  operation_id: id(nextOperation++), expected_consent_revision: 1,
  scope: { kind: 'categories', categories: ['dictionary'] }, effect: 'cloud_only', ...overrides });
const accountScope = { scope: { kind: 'account' }, effect: 'cloud_only' };

const read = async path => (await db.doc(path).get()).data();
const collection = async (uid, name) => (await db.collection(`${paths.account(uid)}/${name}`).get()).docs;
const exists = uid => auth.getUser(uid).then(() => true, () => false);

// Sync on for every category, as an honest device would have left it.
async function enableSync(user) {
  const account = initialAccount(user.uid);
  for (const category of ['settings', 'dictionary']) account.consent[category] = { enabled: true, generation: 1 };
  account.consent.transcripts = { enabled: true, generation: 1,
    history: { mode: 'future_only', enrollment_id: ENROLLMENT } };
  await db.doc(paths.account(user.uid)).set(account);
  return account;
}
function operation(user, category, recordId, content, overrides = {}) {
  return { schema_version: 1, uid: user.uid, account_epoch: 1, consent_generation: 1, category,
    operation_id: id(nextOperation++), device_id: DEVICE, record_id: recordId, origin_device_id: DEVICE,
    base_revision: 0, ...(category === 'transcripts' && { history_enrollment_id: ENROLLMENT }),
    content, ...overrides };
}
const live = data => ({ state: 'live', data });
const submit = (user, op) =>
  transact(user.uid, read => planOperation({ authUid: user.uid, op, read, attemptEnrolled: true }));
// One of each kind of synced record, written through the rules by the client.
async function seedContent(user) {
  const ops = [
    operation(user, 'settings', settingIds.theme, live({ kind: 'setting', key: 'theme', value: 'dark' })),
    operation(user, 'dictionary', id(10), live({ kind: 'hint', enabled: true, text: 'שלום Writer' })),
    operation(user, 'dictionary', id(11), live({ kind: 'hint', enabled: true, text: 'Metrix' })),
    operation(user, 'transcripts', id(20), live({ kind: 'original', attempt_id: id(30),
      recorded_at_ms: 1_790_000_000_000, input_language: 'mixed', output_language: { mode: 'preserve' },
      selection: model, text: 'שלום world' })),
  ];
  for (const op of ops) assert.equal((await submit(user, op)).status, 'acknowledged');
  await db.doc(paths.device(user.uid, DEVICE)).set({ schema_version: 1, uid: user.uid, account_epoch: 1,
    device_id: DEVICE, reconciled_generations: { settings: 1, dictionary: 1, transcripts: 1 } });
  return ops;
}

test('requests without a valid token, or from a browser, are refused without detail', async () => {
  const user = await signIn('owner-a');
  for (const token of [undefined, 'not-a-token', 'a.b.c', `${user.token}x`]) {
    assert.deepEqual(await cleanup({ token }, request(user)), { status: 401, body: { error: 'unauthenticated' } });
  }
  assert.deepEqual(await call('POST', '/v1/cleanup', { token: user.token, body: request(user),
    headers: { origin: 'https://metrix-writer.web.app' } }), { status: 400, body: { error: 'invalid_request' } });
  assert.deepEqual(await call('POST', '/v1/cleanup', { token: user.token, body: 'x'.repeat(9000) }),
    { status: 400, body: { error: 'invalid_request' } });
  assert.deepEqual(await call('POST', '/v1/cleanup', { token: user.token, body: JSON.stringify(request(user)),
    headers: { 'content-type': 'text/plain' } }), { status: 400, body: { error: 'invalid_request' } });
  assert.deepEqual(await cleanup(user, { ...request(user), api_key: 'secret' }),
    { status: 400, body: { error: 'invalid_request' } });
  assert.deepEqual(await status(user, 'not-a-uuid'), { status: 400, body: { error: 'invalid_request' } });
  assert.deepEqual(await call('GET', '/v1/other', { token: user.token }), { status: 404, body: { error: 'not_found' } });
  assert.equal(await read(paths.account(user.uid)), undefined);
});

test('only a recent Google sign-in of the same UID may clean up or revoke', async () => {
  const a = await signIn('owner-a');
  const b = await signIn('owner-b');
  const anonymous = await signIn(null, 'anonymous');
  assert.deepEqual(await cleanup(b, request(a)), { status: 403, body: { error: 'not_owner' } });
  assert.deepEqual(await cleanup(anonymous, request(anonymous)), { status: 403, body: { error: 'not_owner' } });
  assert.deepEqual(await call('POST', '/v1/sessions/revoke', { token: b.token,
    body: { schema_version: 1, uid: a.uid } }), { status: 403, body: { error: 'not_owner' } });
  clock = 301_000;
  assert.deepEqual(await cleanup(a, request(a)), { status: 401, body: { error: 'reauth_required' } });
  assert.deepEqual(await call('POST', '/v1/sessions/revoke', { token: a.token,
    body: { schema_version: 1, uid: a.uid } }), { status: 401, body: { error: 'reauth_required' } });
  assert.equal(await read(paths.account(a.uid)), undefined);
  assert.equal(await exists(a.uid), true);
});

test('a stale preview fails and must be previewed again', async () => {
  const user = await signIn('owner-a');
  await enableSync(user);
  assert.deepEqual(await cleanup(user, request(user, { account_epoch: 2 })),
    { status: 409, body: { error: 'stale_epoch' } });
  assert.deepEqual(await cleanup(user, request(user, { expected_consent_revision: 2 })),
    { status: 409, body: { error: 'stale_consent' } });
  assert.equal((await collection(user.uid, 'cleanup_jobs')).length, 0);
});

test('category cleanup fences, erases scoped content only and is idempotent', async () => {
  const a = await signIn('owner-a');
  const b = await signIn('owner-b');
  await enableSync(a);
  await enableSync(b);
  const [setting, , , original] = await seedContent(a);
  await seedContent(b);
  const scoped = request(a, { scope: { kind: 'categories', categories: ['dictionary', 'transcripts'] } });
  const first = await cleanup(a, scoped);
  assert.equal(first.status, 200);
  assert.equal(first.body.receipt.state, 'completed');
  assert.equal(first.body.receipt.fenced_account_epoch, 1);

  const account = await read(paths.account(a.uid));
  assert.equal(account.account_state, 'active');
  assert.equal(account.resets, undefined);
  assert.equal(account.consent.revision, 2);
  assert.deepEqual(account.consent.dictionary, { enabled: false, generation: 2 });
  assert.deepEqual(account.consent.transcripts, { enabled: false, generation: 2, history: { mode: 'off' } });
  assert.deepEqual(account.consent.settings, { enabled: true, generation: 1 });
  const records = Object.fromEntries((await collection(a.uid, 'records')).map(doc => [doc.id, doc.data()]));
  for (const recordId of [id(10), id(11), id(20)]) {
    assert.deepEqual([records[recordId].content, records[recordId].revision], [{ state: 'deleted' }, 2]);
  }
  assert.equal(records[settingIds.theme].content.state, 'live');
  const receipts = (await collection(a.uid, 'receipts')).map(doc => doc.id);
  assert.deepEqual(receipts, [setting.operation_id]);
  assert.equal((await collection(a.uid, 'families')).length, 1);
  assert.equal((await read(paths.attempt(a.uid, id(30)))).original_record_id, original.record_id);
  assert.equal((await collection(a.uid, 'devices')).length, 1);
  // The other account is untouched.
  assert.equal((await collection(b.uid, 'receipts')).length, 4);
  assert.ok((await collection(b.uid, 'records')).every(doc => doc.get('content.state') === 'live'));

  // Identical retries return the same job and never advance the fence again.
  assert.deepEqual(await cleanup(a, scoped), first);
  assert.equal((await read(paths.account(a.uid))).consent.revision, 2);
  assert.deepEqual(await status(a, scoped.operation_id), first);
  assert.deepEqual(await cleanup(a, { ...scoped, effect: 'everywhere' }),
    { status: 409, body: { error: 'idempotency_conflict' } });
  assert.deepEqual(await status(b, scoped.operation_id), { status: 404, body: { error: 'not_found' } });

  // A stale offline device can't write the erased category back, through the planner or around it.
  const stale = operation(a, 'dictionary', id(12), live({ kind: 'hint', enabled: true, text: 'late' }));
  assert.equal((await submit(a, stale)).status, 'stale_generation');
  await assert.rejects(commit([{ path: paths.record(a.uid, id(12)), data: recordDocument(stale, 1) },
    { path: paths.receipt(a.uid, stale.operation_id), data: { operation: stale, revision: 1 } }], a.uid),
  error => error instanceof Denied && error.status === 403);
  const current = operation(a, 'settings', settingIds.theme,
    live({ kind: 'setting', key: 'theme', value: 'light' }), { base_revision: 1 });
  assert.equal((await submit(a, current)).status, 'acknowledged');
});

test('delete everywhere publishes a reset for reconnecting devices', async () => {
  const user = await signIn('owner-a');
  await enableSync(user);
  await seedContent(user);
  const { body } = await cleanup(user, request(user, { effect: 'everywhere' }));
  assert.equal(body.receipt.state, 'completed');
  assert.deepEqual((await read(paths.account(user.uid))).resets,
    { dictionary: { generation: 2, job_id: body.receipt.job_id } });
});

test('account deletion erases every subcollection, keeps the fence and deletes the user last', async () => {
  const a = await signIn('owner-a');
  const b = await signIn('owner-b');
  await enableSync(a);
  await enableSync(b);
  await seedContent(a);
  await seedContent(b);
  const category = request(a);
  assert.equal((await cleanup(a, category)).body.receipt.state, 'completed');
  const deletion = request(a, { ...accountScope, expected_consent_revision: 2 });
  const { status: code, body } = await cleanup(a, deletion);
  assert.equal(code, 200);
  assert.equal(body.receipt.state, 'completed');
  assert.equal(body.receipt.fenced_account_epoch, 2);

  const fence = await read(paths.account(a.uid));
  assert.deepEqual([fence.account_state, fence.account_epoch, fence.consent.account_epoch], ['deleting', 2, 2]);
  const remaining = (await db.doc(paths.account(a.uid)).listCollections()).map(c => c.id);
  assert.deepEqual(remaining, ['cleanup_jobs']);
  assert.equal((await collection(a.uid, 'cleanup_jobs')).length, 2);
  assert.equal(await exists(a.uid), false);
  assert.equal(await exists(b.uid), true);
  assert.equal((await collection(b.uid, 'records')).length, 4);

  // The deleted user's last token still reads that completed account job, and nothing else.
  assert.deepEqual(await status(a, deletion.operation_id), { status: 200, body });
  assert.deepEqual(await status(a, category.operation_id), { status: 404, body: { error: 'not_found' } });
  assert.deepEqual(await cleanup(a, deletion), { status: 401, body: { error: 'unauthenticated' } });
});

test('an account that never turned on sync can still be deleted', async () => {
  const user = await signIn('owner-a');
  const { body } = await cleanup(user, request(user, accountScope));
  assert.equal(body.receipt.state, 'completed');
  const fence = await read(paths.account(user.uid));
  assert.deepEqual([fence.account_state, fence.account_epoch, fence.consent.revision], ['deleting', 2, 2]);
  assert.equal(await exists(user.uid), false);
});

test('an interrupted cleanup reports failed_retryable and resumes without fencing twice', async () => {
  const user = await signIn('owner-a');
  await enableSync(user);
  await seedContent(user);
  fault = () => { fault = null; throw Object.assign(new Error('synthetic'), { code: 14 }); };
  const deletion = request(user, accountScope);
  const failed = await cleanup(user, deletion);
  assert.equal(failed.body.receipt.state, 'failed_retryable');
  assert.equal(await exists(user.uid), true);
  const job = await read(paths.cleanupJob(user.uid, deletion.operation_id));
  assert.deepEqual([job.runner.failures, job.runner.reason], [1, 'error']);

  // The sweeper waits out the backoff, then finishes the same job.
  assert.equal(await service.sweep(), 0);
  clock = backoff(1) + 1000;
  assert.equal(await service.sweep(), 1);
  const done = await read(paths.cleanupJob(user.uid, deletion.operation_id));
  assert.equal(done.receipt.state, 'completed');
  assert.equal(done.receipt.job_id, failed.body.receipt.job_id);
  assert.equal(done.runner, undefined);
  const fence = await read(paths.account(user.uid));
  assert.deepEqual([fence.account_epoch, fence.consent.revision], [2, 2]);
  assert.equal(await exists(user.uid), false);
});

test('an identical retry resumes a failed category job at once, but quota failures wait', async () => {
  const user = await signIn('owner-a');
  await enableSync(user);
  await seedContent(user);
  fault = () => { throw Object.assign(new Error('synthetic'), { code: 8 }); };
  const scoped = request(user);
  assert.equal((await cleanup(user, scoped)).body.receipt.state, 'failed_retryable');
  assert.equal((await read(paths.cleanupJob(user.uid, scoped.operation_id))).runner.reason, 'quota');
  fault = null;
  assert.equal((await cleanup(user, scoped)).body.receipt.state, 'failed_retryable');
  clock = backoff(1) + 1000;
  assert.equal((await cleanup(user, scoped)).body.receipt.state, 'completed');
  assert.equal((await read(paths.account(user.uid))).consent.revision, 2);
  assert.equal((await collection(user.uid, 'receipts')).length, 2);
});

test('revoking sessions refuses the old token and anything refreshed from it', async () => {
  const user = await signIn('owner-a');
  // Revocation is recorded in whole seconds; stay clear of the sign-in second.
  await new Promise(done => setTimeout(done, 1100));
  assert.deepEqual(await call('POST', '/v1/sessions/revoke', { token: user.token,
    body: { schema_version: 1, uid: user.uid, extra: true } }), { status: 400, body: { error: 'invalid_request' } });
  assert.deepEqual(await call('POST', '/v1/sessions/revoke', { token: user.token,
    body: { schema_version: 1, uid: user.uid } }), { status: 200, body: { revoked: true } });
  await assert.rejects(auth.verifyIdToken(user.token, true), { code: 'auth/id-token-revoked' });
  const refreshed = await fetch(`http://${authHost}/securetoken.googleapis.com/v1/token?key=demo-key`, {
    method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({ grant_type: 'refresh_token', refresh_token: user.refresh }) });
  // Production refuses this refresh outright; the emulator still issues a token,
  // but it carries the old auth_time, so the revocation check refuses it too.
  const renewed = refreshed.ok ? (await refreshed.json()).id_token : null;
  if (renewed) assert.deepEqual(await status({ token: renewed }, id(1)), { status: 401, body: { error: 'unauthenticated' } });
  assert.deepEqual(await status(user, id(1)), { status: 401, body: { error: 'unauthenticated' } });
  assert.equal(await exists(user.uid), true);
});

test('each UID is limited to 20 requests a minute', async () => {
  const a = await signIn('owner-a');
  const b = await signIn('owner-b');
  for (let n = 0; n < 20; n++) assert.equal((await status(a, id(1))).status, 404);
  assert.deepEqual(await status(a, id(1)), { status: 429, body: { error: 'rate_limited' } });
  assert.equal((await status(b, id(1))).status, 404);
  clock = 61_000;
  assert.equal((await status(a, id(1))).status, 404);
});
