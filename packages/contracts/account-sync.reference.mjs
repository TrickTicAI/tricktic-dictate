// Test oracle for the language-neutral vectors, NOT a persistence or sync adapter.
import { isDeepStrictEqual } from 'node:util';
import { settingIds } from './account-sync.schema.mjs';

export function validPortableData(id, category, data) {
  if (category === 'settings' && settingIds[data.key] !== id) return false;
  if (category !== 'settings' && Object.values(settingIds).includes(id)) return false;
  if (category === 'dictionary') {
    for (const [key, value] of Object.entries(data)) {
      if (typeof value === 'string' && key !== 'kind' &&
          (!value.trim() || Buffer.byteLength(value) > 8192)) return false;
    }
  }
  const models = [data.selection,
    data.key === 'transcription_model' ? data.value : null,
    data.key === 'cleanup' ? data.value.selection : null].filter(Boolean);
  for (const { model } of models) {
    if (model.includes('..') || model.split('/').some(part => !part)) return false;
  }
  return true;
}

export function validBackup(backup) {
  const records = [...backup.settings, ...backup.dictionary];
  if (new Set(records.map(record => record.id)).size !== records.length) return false;
  if (Buffer.byteLength(JSON.stringify(backup.dictionary)) > 262144) return false;
  for (const kind of ['hint', 'correction', 'snippet']) {
    if (backup.dictionary.filter(record => record.data.kind === kind).length > 200) return false;
  }
  return records.every(record => validPortableData(record.id, record.category, record.data));
}

export function referenceState(initial) {
  return { ...structuredClone(initial), receipts: new Map() };
}

// Precondition: validate the envelope against the schema before calling this oracle.
export function evaluate(state, operation) {
  const op = operation;
  const reject = status => ({ status });
  if (!state.uid || state.uid !== op.uid) return reject('wrong_account');
  if (state.account_state !== 'active') return reject('account_unavailable');
  if (state.account_epoch !== op.account_epoch) return reject('stale_epoch');
  if (state.generation !== op.consent_generation) return reject('stale_generation');
  if (!state.enabled) return reject('consent_required');
  if (op.category === 'transcripts' && (!state.history_enrolled ||
      op.history_enrollment_id !== state.history_enrollment_id)) return reject('history_not_enrolled');
  const receipt = state.receipts.get(op.operation_id);
  if (receipt) return isDeepStrictEqual(receipt.operation, op)
    ? receipt.result : reject('idempotency_conflict');
  if ((op.category === 'settings') !== Object.values(settingIds).includes(op.record_id)) {
    return reject('invalid_record');
  }
  const current = state.records.find(record => record.id === op.record_id);
  if (current && (current.category !== op.category || current.origin_device_id !== op.origin_device_id)) {
    return reject('invalid_record');
  }
  if (current?.content.state === 'deleted') return reject('deleted');
  if (op.content.state === 'live') {
    const data = op.content.data;
    if (!validPortableData(op.record_id, op.category, data)) return reject('invalid_record');
    if (op.category === 'transcripts' && data.kind === 'original' && state.records.some(record =>
      record.id !== op.record_id && record.content.data?.kind === 'original' &&
      record.content.data.attempt_id === data.attempt_id)) return reject('invalid_record');
    if (op.category === 'transcripts' && data.kind !== 'original') {
      const original = state.records.find(record => record.id === data.original_record_id);
      const parent = state.records.find(record => record.id === data.parent_record_id);
      if (original?.content.state === 'deleted') return reject('deleted');
      if (!original || original.category !== 'transcripts' || original.content.data.kind !== 'original' ||
          !parent || parent.category !== 'transcripts' || parent.content.state !== 'live' ||
          (parent.id !== original.id && parent.content.data.original_record_id !== original.id)) {
        return reject('invalid_record');
      }
    }
    if (op.category === 'dictionary' && current && current.content.data.kind !== data.kind) {
      return reject('invalid_record');
    }
  }
  if (!current && op.base_revision !== 0) return reject('invalid_base');
  if (current && current.revision !== op.base_revision) return { status: 'conflict', revision: current.revision };
  if (current && op.category === 'transcripts' && op.content.state === 'live') return reject('immutable');
  const revision = (current?.revision ?? 0) + 1;
  const saved = { schema_version: 1, id: op.record_id, category: op.category,
    origin_device_id: op.origin_device_id, revision, content: structuredClone(op.content) };
  state.records = state.records.filter(record => record.id !== op.record_id);
  state.records.push(saved);
  // Deleting an original atomically hides/tombstones all its text descendants.
  if (op.category === 'transcripts' && current?.content.data?.kind === 'original' &&
      op.content.state === 'deleted') {
    state.records = state.records.map(record => record.content.data?.original_record_id === op.record_id
      ? { ...record, revision: record.revision + 1, content: { state: 'deleted' } } : record);
  }
  const result = { status: 'acknowledged', revision };
  state.receipts.set(op.operation_id, { operation: structuredClone(op), result });
  return result;
}

export function mayApplyResponse(binding, response) {
  return binding.uid === response.uid && binding.account_epoch === response.account_epoch &&
    binding.consent_generation === response.consent_generation && binding.enabled;
}
