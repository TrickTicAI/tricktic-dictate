import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import Ajv from 'ajv';
import addFormats from 'ajv-formats';
import { evaluate, referenceState, validBackup, validPortableData, mayApplyResponse } from './account-sync.reference.mjs';

const read = path => JSON.parse(readFileSync(new URL(path, import.meta.url), 'utf8'));
const schema = read('./account-sync.schema.json');
const { catalog, records } = read('../../docs/testing/account-sync/wire.json');
const scenarios = read('../../docs/testing/account-sync/conflicts.json');
const ajv = new Ajv({ strict: true, allErrors: true });
addFormats(ajv);
ajv.addSchema(schema, 'account-sync');
const check = name => ajv.compile({ $ref: `account-sync#/definitions/${name}` });
const validateRecord = check('RemoteRecord');
const validateOperation = check('OutboxOperation');
const validateBackup = check('PortableBackup');
const accept = (validate, value) => assert.equal(validate(value), true, JSON.stringify(validate.errors));
const rejectChange = (value, validate, change) => {
  const copy = structuredClone(value); change(copy); assert.equal(validate(copy), false);
};

test('catalog and English/Hebrew/mixed records round-trip without text normalization', () => {
  accept(ajv.getSchema('account-sync'), catalog);
  for (const record of records) {
    accept(validateRecord, record);
    assert(validPortableData(record.id, record.category, record.content.data));
    assert.deepEqual(JSON.parse(JSON.stringify(record)), record);
  }
  assert(catalog.record.content.data.text.includes('e\u0301 / é'));
  assert(catalog.record.content.data.text.includes('\u200fעברית\u200e'));
});

test('unsupported versions, unsafe integers, wrong categories and malformed IDs fail closed', () => {
  for (const version of [0, 2, '1', null]) {
    rejectChange(catalog.record, validateRecord, value => value.schema_version = version);
    rejectChange(catalog.backup, validateBackup, value => value.format_version = version);
  }
  for (const revision of [-1, 0, 1.5, Number.MAX_SAFE_INTEGER + 1]) {
    rejectChange(catalog.record, validateRecord, value => value.revision = revision);
  }
  rejectChange(catalog.record, validateRecord, value => value.id = '../private');
  rejectChange(catalog.record, validateRecord, value => value.category = 'settings');
  rejectChange(catalog.outbox, validateOperation, value => value.consent_generation = 0);
  rejectChange(catalog.outbox, validateOperation, value => value.base_revision = -1);
  rejectChange(catalog.record, validateRecord, value => value.content.state = 'pending');
  rejectChange(catalog.record, validateRecord, value => value.content.data.text = '\u0000');
});

test('every object boundary rejects extra fields, including nested sensitive state', () => {
  const forbidden = ['api_key', 'access_token', 'refresh_token', 'device_path', 'microphone_id',
    'model_binary', 'clipboard', 'foreground_context', 'pending_provider_request', 'audio', 'extensions'];
  // Walk instances, not just top-level fields: no permissive nested escape hatch.
  function visit(value, path = []) {
    if (!value || typeof value !== 'object') return;
    if (!Array.isArray(value)) {
      for (const field of forbidden) {
        const copy = structuredClone(catalog);
        let target = copy;
        for (const key of path) target = target[key];
        target[field] = 'synthetic-excluded-value';
        assert.equal(ajv.getSchema('account-sync')(copy), false, `${path.join('.')}.${field}`);
      }
    }
    for (const [key, child] of Object.entries(value)) visit(child, [...path, key]);
  }
  visit(catalog);
});

test('portable backup preserves unsupported platform settings but excludes history and account binding', () => {
  accept(validateBackup, catalog.backup);
  assert(validBackup(catalog.backup));
  // A device supporting only these fields must retain, and re-export, the others unchanged.
  const supported = new Set(['ui_language', 'input_language']);
  const applied = catalog.backup.settings.filter(record => supported.has(record.data.key));
  const preserved = catalog.backup.settings.filter(record => !supported.has(record.data.key));
  assert.equal(applied.length, 2);
  assert.deepEqual([...applied, ...preserved], catalog.backup.settings);
  for (const field of ['transcripts', 'uid', 'consent', 'outbox']) {
    rejectChange(catalog.backup, validateBackup, value => value[field] = []);
  }
  const duplicate = structuredClone(catalog.backup);
  duplicate.dictionary.push(duplicate.dictionary[0]);
  assert.equal(validBackup(duplicate), false);
  const wrongId = structuredClone(catalog.backup);
  wrongId.settings[0].id = wrongId.dictionary[0].id;
  assert.equal(validBackup(wrongId), false);
  const tooLarge = structuredClone(catalog.backup);
  tooLarge.dictionary[0].data.text = 'א'.repeat(4097);
  assert.equal(validBackup(tooLarge), false); // UTF-8 bytes, not UTF-16/codepoint length.
});

test('polish instructions carry Super text plus every writing preference, closed', () => {
  const polish = records.find(record => record.content.data?.key === 'polish_instructions');
  accept(validateRecord, polish);
  assert(validPortableData(polish.id, polish.category, polish.content.data));
  assert.equal(polish.id, '00000000-0000-4000-8000-000000000009');
  const value = polish.content.data.value;
  assert.equal(typeof value.instructions, 'string');
  assert.deepEqual(Object.keys(value.formatting).sort(),
    ['layout', 'lists', 'remove_fillers', 'rewrite', 'self_corrections', 'spoken_punctuation']);
  rejectChange(polish, validateRecord, record => record.content.data.value.instructions = '');
  rejectChange(polish, validateRecord, record => record.content.data.value.instructions = 'x'.repeat(16385));
  rejectChange(polish, validateRecord, record => record.content.data.value.instructions = 'ok\u0000');
  rejectChange(polish, validateRecord, record => record.content.data.value.formatting.rewrite = 'flowery');
  rejectChange(polish, validateRecord, record => record.content.data.value.formatting.layout = 'two_columns');
  rejectChange(polish, validateRecord, record => record.content.data.value.formatting.lists = 'yes');
  rejectChange(polish, validateRecord, record => delete record.content.data.value.formatting.spoken_punctuation);
  rejectChange(polish, validateRecord, record => record.content.data.value.prompt_versions = []);
  assert.equal(validPortableData('00000000-0000-4000-8000-000000000008', 'settings', polish.content.data), false);
});

for (const scenario of scenarios) {
  test(`conformance: ${scenario.name}`, () => {
    const state = referenceState(scenario.initial);
    for (const record of state.records) accept(validateRecord, record);
    for (const { operation, expected, authority_patch } of scenario.steps) {
      if (authority_patch) Object.assign(state, authority_patch);
      accept(validateOperation, operation);
      const before = structuredClone(state.records);
      assert.deepEqual(evaluate(state, operation), expected);
      if (expected.status !== 'acknowledged') assert.deepEqual(state.records, before);
      for (const record of state.records) accept(validateRecord, record);
    }
    if (scenario.name === 'concurrent-edited-versions-retained') {
      assert.equal(state.records.filter(record => record.content.data?.kind === 'edited').length, 2);
    }
    if (scenario.name === 'delete-original-fences-descendants') {
      assert(state.records.every(record => record.content.state === 'deleted'));
    }
  });
}

test('late response cannot cross sign-out, UID, epoch or consent boundaries', () => {
  const binding = { uid: catalog.response.uid, account_epoch: 1, consent_generation: 1, enabled: true };
  assert(mayApplyResponse(binding, catalog.response));
  for (const change of [{ uid: null }, { uid: 'synthetic-user-b' }, { account_epoch: 2 },
    { consent_generation: 2 }, { enabled: false }]) {
    assert.equal(mayApplyResponse({ ...binding, ...change }, catalog.response), false);
  }
});

test('tombstones carry no text; cleanup/import result shapes cannot imply an unfinished success', () => {
  const deleted = { ...catalog.record, content: { state: 'deleted' } };
  accept(validateRecord, deleted);
  rejectChange(deleted, validateRecord, value => value.content.data = catalog.record.content.data);
  const receipt = check('AccountCleanupReceipt');
  rejectChange(catalog.cleanup_receipt, receipt, value => delete value.fenced_consent);
  rejectChange(catalog.cleanup_receipt, receipt, value => value.state = 'succeeded');
  rejectChange(catalog.import_receipt, check('ImportReceipt'), value => delete value.rollback_snapshot_id);
  rejectChange(catalog.consent, check('ConsentState'), value => value.transcripts.enabled = true);
  rejectChange(catalog.consent, check('ConsentState'), value => {
    value.transcripts.history = { mode: 'future_only', enrollment_id: catalog.outbox.operation_id };
  });
  rejectChange(catalog.cleanup_request, check('AccountCleanupRequest'), value => {
    value.scope = { kind: 'account' }; value.effect = 'everywhere';
  });
});
