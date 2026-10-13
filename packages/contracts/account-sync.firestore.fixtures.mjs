// Maps a conflicts.json `initial` authority onto Firestore documents, so the
// same vectors drive the in-memory planner test and the emulator rules test.
import { initialAccount, paths } from './account-sync.firestore.mjs';

export const SEED_OPERATION_ID = 'ffffffff-ffff-4fff-bfff-000000000000';

// The test authority for one category, as the account document stores it.
export function accountDocument(authority, uid, category) {
  const account = initialAccount(uid);
  account.account_epoch = authority.account_epoch;
  account.account_state = authority.account_state;
  account.consent.account_epoch = authority.account_epoch;
  const toggle = { enabled: authority.enabled, generation: authority.generation };
  account.consent[category] = category !== 'transcripts' ? toggle : { ...toggle, history: authority.enabled
    ? { mode: 'future_only', enrollment_id: authority.history_enrollment_id } : { mode: 'off' } };
  return account;
}

// Records plus the family and attempt indexes the rules maintain for transcripts.
export function seedDocuments(initial, uid, category) {
  const documents = new Map([[paths.account(uid), accountDocument(initial, uid, category)]]);
  for (const record of initial.records) {
    documents.set(paths.record(uid, record.id), { ...structuredClone(record),
      operation_id: SEED_OPERATION_ID, consent_generation: initial.generation });
    const data = record.content.data;
    if (data?.kind === 'original') {
      documents.set(paths.attempt(uid, data.attempt_id), { original_record_id: record.id });
      documents.set(paths.family(uid, record.id), { deleted: record.content.state === 'deleted',
        member_ids: initial.records.filter(member => member.content.data?.original_record_id === record.id)
          .map(member => member.id) });
    }
  }
  return documents;
}

export function remoteRecords(documents, uid) {
  const prefix = `${paths.account(uid)}/records/`;
  return [...documents].filter(([path]) => path.startsWith(prefix))
    .map(([, { operation_id, consent_generation, ...record }]) => record)
    .sort((a, b) => a.id.localeCompare(b.id));
}
