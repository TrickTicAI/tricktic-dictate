// Firestore layout and commit planner for sync v1 (MW-OUATH-5). This is the
// reference the desktop (REST) and Android (SDK) adapters mirror; every write it
// plans is independently enforced by infra/firebase/firestore.rules.
// Precondition, as for the oracle: validate the envelope against the schema first.
import { settingIds } from './account-sync.schema.mjs';
import { validPortableData } from './account-sync.reference.mjs';

export const MAX_FAMILY_MEMBERS = 200;
export const MAX_LIST_LIMIT = 500;
const settingIdSet = new Set(Object.values(settingIds));
const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

// Firebase UIDs are opaque; refuse anything that could change the path shape.
export function checkedUid(uid) {
  if (typeof uid !== 'string' || !/^[^/]{1,128}$/.test(uid) || uid === '.' || uid === '..' ||
      /^__.*__$/.test(uid)) throw new Error('invalid account UID');
  return uid;
}
function checkedId(id) {
  if (!uuidPattern.test(id)) throw new Error('invalid record or operation ID');
  return id;
}

export const paths = {
  account: uid => `users/${checkedUid(uid)}`,
  record: (uid, id) => `users/${checkedUid(uid)}/records/${checkedId(id)}`,
  receipt: (uid, id) => `users/${checkedUid(uid)}/receipts/${checkedId(id)}`,
  family: (uid, id) => `users/${checkedUid(uid)}/families/${checkedId(id)}`,
  attempt: (uid, id) => `users/${checkedUid(uid)}/attempts/${checkedId(id)}`,
  device: (uid, id) => `users/${checkedUid(uid)}/devices/${checkedId(id)}`,
  cleanupJob: (uid, id) => `users/${checkedUid(uid)}/cleanup_jobs/${checkedId(id)}`,
};

// The only account document a client may create: everything disabled.
export function initialAccount(uid) {
  const off = { enabled: false, generation: 1 };
  return { schema_version: 1, account_epoch: 1, account_state: 'active', consent: {
    schema_version: 1, uid, account_epoch: 1, revision: 1, settings: { ...off },
    dictionary: { ...off }, transcripts: { ...off, history: { mode: 'off' } },
  } };
}

// Next ConsentState for an explicit user choice. Any toggle or history change
// advances that category's generation by one; the client never picks it.
export function nextConsent(consent, category, enabled, history = null) {
  const next = structuredClone(consent);
  const before = consent[category];
  const after = { enabled, generation: before.generation };
  if (category === 'transcripts') after.history = enabled ? structuredClone(history) : { mode: 'off' };
  const changed = before.enabled !== enabled ||
    (category === 'transcripts' && !equal(before.history, after.history));
  if (changed) after.generation += 1;
  next[category] = after;
  next.revision += 1;
  return next;
}

// Storage document <-> RemoteRecord. The extra fields bind the record to the
// receipt of the operation that wrote it and the generation it was written in.
export function recordDocument(op, revision) {
  return { schema_version: 1, id: op.record_id, category: op.category,
    origin_device_id: op.origin_device_id, revision, content: structuredClone(op.content),
    operation_id: op.operation_id, consent_generation: op.consent_generation };
}
export function remoteRecord(document) {
  const { schema_version, id, category, origin_device_id, revision, content } = document;
  return { schema_version, id, category, origin_device_id, revision, content };
}

export function equal(a, b) {
  if (a === b) return true;
  if (!a || !b || typeof a !== 'object' || typeof b !== 'object' || Array.isArray(a) !== Array.isArray(b)) {
    return false;
  }
  const keys = Object.keys(a);
  return keys.length === Object.keys(b).length && keys.every(key => equal(a[key], b[key]));
}

/**
 * Decide one outbox operation inside a Firestore transaction.
 * `read(path)` returns the transaction's snapshot of a document, or null.
 * `attemptEnrolled` is the device's durable history-enrollment decision for this
 * attempt; Firestore cannot observe it, so the device must supply it.
 * Returns { status, revision?, record?, current?, writes }. Only an
 * `acknowledged` result with a non-empty `writes` list is committed.
 */
export async function planOperation({ authUid, op, read, attemptEnrolled = true }) {
  const reject = status => ({ status, writes: [] });
  if (!authUid || authUid !== op.uid) return reject('wrong_account');
  const uid = checkedUid(op.uid);
  const account = await read(paths.account(uid));
  if (!account) return reject('consent_required');
  if (account.account_state !== 'active') return reject('account_unavailable');
  if (account.account_epoch !== op.account_epoch) return reject('stale_epoch');
  const consent = account.consent[op.category];
  if (consent.generation !== op.consent_generation) return reject('stale_generation');
  if (!consent.enabled) return reject('consent_required');
  if (op.category === 'transcripts' && (!attemptEnrolled ||
      consent.history.enrollment_id !== op.history_enrollment_id)) return reject('history_not_enrolled');

  const receipt = await read(paths.receipt(uid, op.operation_id));
  if (receipt) {
    if (!equal(receipt.operation, op)) return reject('idempotency_conflict');
    return { status: 'acknowledged', revision: receipt.revision,
      record: remoteRecord(recordDocument(op, receipt.revision)), writes: [] };
  }

  if ((op.category === 'settings') !== settingIdSet.has(op.record_id)) return reject('invalid_record');
  const current = await read(paths.record(uid, op.record_id));
  if (current && (current.category !== op.category || current.origin_device_id !== op.origin_device_id)) {
    return reject('invalid_record');
  }
  // A fixed setting ID may be recreated only in a newer confirmed generation.
  if (current?.content.state === 'deleted' && !(op.category === 'settings' &&
      current.consent_generation < op.consent_generation)) return reject('deleted');

  const writes = [];
  const data = op.content.state === 'live' ? op.content.data : null;
  if (data) {
    if (!validPortableData(op.record_id, op.category, data)) return reject('invalid_record');
    if (op.category === 'dictionary' && current?.content.state === 'live' &&
        current.content.data.kind !== data.kind) return reject('invalid_record');
  }
  if (op.category === 'transcripts' && data?.kind === 'original' && !current) {
    const attempt = await read(paths.attempt(uid, data.attempt_id));
    if (attempt) return reject('invalid_record');
    writes.push({ path: paths.attempt(uid, data.attempt_id), data: { original_record_id: op.record_id } },
      { path: paths.family(uid, op.record_id), data: { member_ids: [], deleted: false } });
  } else if (op.category === 'transcripts' && data && data.kind !== 'original' && !current) {
    const family = await read(paths.family(uid, data.original_record_id));
    if (family?.deleted) return reject('deleted');
    const parentId = data.parent_record_id;
    const parent = await read(paths.record(uid, parentId));
    if (!family || !parent || parent.category !== 'transcripts' || parent.content.state !== 'live' ||
        (parentId !== data.original_record_id && !family.member_ids.includes(parentId))) {
      return reject('invalid_record');
    }
    if (family.member_ids.length >= MAX_FAMILY_MEMBERS) return reject('quota_exceeded');
    writes.push({ path: paths.family(uid, data.original_record_id),
      data: { member_ids: [...family.member_ids, op.record_id], deleted: false } });
  }

  if (!current && op.base_revision !== 0) return reject('invalid_base');
  if (current && current.revision !== op.base_revision) {
    return { status: 'conflict', revision: current.revision, current: remoteRecord(current), writes: [] };
  }
  if (current && op.category === 'transcripts' && data) return reject('immutable');

  const revision = (current?.revision ?? 0) + 1;
  writes.unshift({ path: paths.record(uid, op.record_id), data: recordDocument(op, revision) },
    { path: paths.receipt(uid, op.operation_id), data: { operation: structuredClone(op), revision } });
  // Deleting an original fences its family and tombstones every live version in
  // the same commit. Readers also treat members of a fenced family as deleted.
  if (op.category === 'transcripts' && !data && current?.content.data?.kind === 'original') {
    const family = await read(paths.family(uid, op.record_id));
    if (family && !family.deleted) {
      writes.push({ path: paths.family(uid, op.record_id), data: { ...family, deleted: true } });
      for (const memberId of family.member_ids) {
        const member = await read(paths.record(uid, memberId));
        if (member?.content.state !== 'live') continue;
        writes.push({ path: paths.record(uid, memberId),
          data: { ...member, revision: member.revision + 1, content: { state: 'deleted' } } });
      }
    }
  }
  return { status: 'acknowledged', revision, record: remoteRecord(recordDocument(op, revision)), writes };
}
