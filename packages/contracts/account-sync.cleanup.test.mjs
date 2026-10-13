import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import Ajv from 'ajv';
import addFormats from 'ajv-formats';
import { planCleanup } from './account-sync.cleanup.mjs';
import { initialAccount, nextConsent } from './account-sync.firestore.mjs';

const read = path => JSON.parse(readFileSync(new URL(path, import.meta.url), 'utf8'));
const ajv = new Ajv({ strict: true, allErrors: true });
addFormats(ajv);
ajv.addSchema(read('./account-sync.schema.json'), 'account-sync');
const validReceipt = ajv.compile({ $ref: 'account-sync#/definitions/AccountCleanupReceipt' });
const { catalog } = read('../../docs/testing/account-sync/wire.json');

const projectId = 'demo-metrix-writer';
const nowSeconds = 1_800_000_000;
const uid = catalog.cleanup_request.uid;
const claims = { aud: projectId, iss: `https://securetoken.google.com/${projectId}`, sub: uid,
  auth_time: nowSeconds - 60, firebase: { sign_in_provider: 'google.com' } };
const jobId = catalog.cleanup_receipt.job_id;
const account = initialAccount(uid);
account.consent = nextConsent(account.consent, 'dictionary', true);
const request = { ...catalog.cleanup_request, expected_consent_revision: 2 };
const plan = overrides => planCleanup({ projectId, claims, request, account, jobId, nowSeconds, ...overrides });

test('category cleanup disables and advances only its scope before acknowledging', () => {
  const { receipt, writes } = plan();
  assert.equal(validReceipt(receipt), true, JSON.stringify(validReceipt.errors));
  assert.deepEqual(receipt.fenced_consent.dictionary, { enabled: false, generation: 3 });
  assert.deepEqual(receipt.fenced_consent.settings, account.consent.settings);
  assert.equal(receipt.fenced_consent.revision, 3);
  assert.equal(writes[0].data.account_state, 'active');
  assert.equal(writes[0].data.resets, undefined);
  assert.equal(writes[1].path, `users/${uid}/cleanup_jobs/${request.operation_id}`);
});

test('delete everywhere records a per-category reset for reconnecting devices', () => {
  const { writes } = plan({ request: { ...request, effect: 'everywhere' } });
  assert.deepEqual(writes[0].data.resets, { dictionary: { generation: 3, job_id: jobId } });
});

test('account deletion advances the epoch and leaves a durable deleting fence', () => {
  const { receipt, writes } = plan({ request: { ...request, scope: { kind: 'account' } } });
  assert.equal(validReceipt(receipt), true, JSON.stringify(validReceipt.errors));
  assert.deepEqual([receipt.account_epoch, receipt.fenced_account_epoch, receipt.fenced_consent.account_epoch],
    [1, 2, 2]);
  assert.equal(writes[0].data.account_state, 'deleting');
  for (const category of ['settings', 'dictionary', 'transcripts']) {
    assert.equal(receipt.fenced_consent[category].enabled, false);
    assert.equal(receipt.fenced_consent[category].generation, account.consent[category].generation + 1);
  }
});

test('identical retries return the same job; changed retries and stale previews fail', () => {
  const first = plan();
  const existingJob = first.writes[1].data;
  assert.deepEqual(plan({ existingJob, account: first.writes[0].data }), { receipt: first.receipt, writes: [] });
  assert.deepEqual(plan({ existingJob, request: { ...request, effect: 'everywhere' } }),
    { error: 'idempotency_conflict' });
  assert.deepEqual(plan({ request: { ...request, expected_consent_revision: 1 } }), { error: 'stale_consent' });
  assert.deepEqual(plan({ account: first.writes[0].data, request: { ...request,
    operation_id: '11111111-1111-4111-8111-000000000052' } }), { error: 'stale_consent' });
});

test('ownership is checked from verified claims, independently of Security Rules', () => {
  for (const change of [{ sub: 'synthetic-user-b' }, { aud: 'metrix-writer' },
    { iss: 'https://accounts.google.com' }, { firebase: { sign_in_provider: 'anonymous' } }]) {
    assert.deepEqual(plan({ claims: { ...claims, ...change } }), { error: 'not_owner' });
  }
  assert.deepEqual(plan({ claims: undefined }), { error: 'not_owner' });
  assert.deepEqual(plan({ claims: { ...claims, auth_time: nowSeconds - 301 } }), { error: 'reauth_required' });
  assert.deepEqual(plan({ claims: { ...claims, auth_time: undefined } }), { error: 'reauth_required' });
  assert.deepEqual(plan({ account: { ...account, consent: { ...account.consent, uid: 'synthetic-user-b' } } }),
    { error: 'not_owner' });
  assert.deepEqual(plan({ account: { ...account, consent: undefined } }), { error: 'not_owner' });
  assert.deepEqual(plan({ account: { ...account, account_state: 'deleting' } }), { error: 'account_unavailable' });
  assert.deepEqual(plan({ account: { ...account, account_epoch: 2 } }), { error: 'stale_epoch' });
});

test('an account that never turned sync on is the all-off initial account', () => {
  const absent = { ...request, scope: { kind: 'account' }, effect: 'cloud_only', expected_consent_revision: 1 };
  const { receipt, writes } = plan({ account: null, request: absent });
  assert.equal(validReceipt(receipt), true, JSON.stringify(validReceipt.errors));
  assert.deepEqual([writes[0].data.account_epoch, writes[0].data.account_state, receipt.fenced_consent.revision],
    [2, 'deleting', 2]);
  assert.deepEqual(plan({ account: null }), { error: 'stale_consent' });
  assert.deepEqual(plan({ account: undefined, request: { ...absent, account_epoch: 2 } }), { error: 'stale_epoch' });
});
