// Cleanup jobs: the fence transaction, the erasure runner and the sweeper.
// See infra/firebase/trusted-cleanup.md. Every step is idempotent, so a crash
// or a failed batch resumes from wherever it stopped.
import { randomUUID } from 'node:crypto';
import { FieldPath, FieldValue } from 'firebase-admin/firestore';
import { paths, planCleanup } from './contract.mjs';

const PAGE = 500;
const RETRY_BASE_MS = 30_000;
const RETRY_CAP_MS = 60 * 60_000;
const OPEN = ['pending', 'running', 'failed_retryable'];
const RESOURCE_EXHAUSTED = 8;

class VerificationFailed extends Error {}
const reason = error => error?.code === RESOURCE_EXHAUSTED ? 'quota'
  : error instanceof VerificationFailed ? 'verification' : 'error';
export const backoff = failures => Math.min(RETRY_BASE_MS * 2 ** (failures - 1), RETRY_CAP_MS);

export function createJobs({ db, auth, now, log, faults = {} }) {
  const active = new Map();
  const queues = new Map();
  const jobRef = (uid, operationId) => db.doc(paths.cleanupJob(uid, operationId));

  // Fence before acknowledgement: planCleanup's writes commit in one transaction.
  async function create({ projectId, claims, request }) {
    const accountRef = db.doc(paths.account(request.uid));
    const ref = jobRef(request.uid, request.operation_id);
    const plan = await db.runTransaction(async tx => {
      const [account, job] = await tx.getAll(accountRef, ref);
      const outcome = planCleanup({ projectId, claims, request, jobId: randomUUID(),
        account: account.exists ? account.data() : null, existingJob: job.exists ? job.data() : undefined,
        nowSeconds: Math.floor(now() / 1000) });
      for (const write of outcome.writes ?? []) tx.set(db.doc(write.path), write.data);
      return outcome;
    });
    if (plan.writes?.length) log('fenced', { job_id: plan.receipt.job_id, scope: plan.receipt.scope.kind });
    return plan;
  }

  const read = async (uid, operationId) => {
    const snapshot = await jobRef(uid, operationId).get();
    return snapshot.exists ? snapshot.data() : null;
  };

  // One runner per job, and one job at a time per account.
  function run(uid, operationId) {
    const key = `${uid}/${operationId}`;
    if (active.has(key)) return active.get(key);
    const promise = (queues.get(uid) ?? Promise.resolve()).then(() => execute(uid, operationId));
    const settled = promise.finally(() => {
      active.delete(key);
      if (queues.get(uid) === settled) queues.delete(uid);
    });
    active.set(key, settled);
    queues.set(uid, settled);
    return settled;
  }

  // A client retry resumes the job at once, except while Firestore reports quota
  // exhaustion: then the sweeper waits out the backoff.
  function resume(uid, operationId, job) {
    const runner = job?.runner;
    if (job?.receipt.state === 'completed') return Promise.resolve();
    if (runner?.reason === 'quota' && runner.next_attempt_at_ms > now()) return Promise.resolve();
    return run(uid, operationId);
  }

  async function execute(uid, operationId) {
    const ref = jobRef(uid, operationId);
    const snapshot = await ref.get().catch(() => null);
    if (!snapshot?.exists || snapshot.get('receipt.state') === 'completed') return;
    const { receipt, runner } = snapshot.data();
    try {
      await ref.update({ 'receipt.state': 'running' });
      const counts = receipt.scope.kind === 'account' ? await eraseAccount(uid) : await eraseCategories(uid, receipt);
      if (receipt.scope.kind === 'account') await deleteUser(uid);
      await ref.update({ 'receipt.state': 'completed', runner: FieldValue.delete() });
      log('completed', { job_id: receipt.job_id, ...counts });
    } catch (error) {
      const failures = (runner?.failures ?? 0) + 1;
      const next = { failures, reason: reason(error), next_attempt_at_ms: now() + backoff(failures) };
      await ref.update({ 'receipt.state': 'failed_retryable', runner: next }).catch(() => {});
      log('failed_retryable', { job_id: receipt.job_id, failures, reason: next.reason });
    }
  }

  // Visits a collection page by page in document ID order.
  async function scan(collection, visit) {
    let last = null;
    for (;;) {
      let query = collection.orderBy(FieldPath.documentId()).limit(PAGE);
      if (last) query = query.startAfter(last);
      const { docs } = await query.get();
      if (!docs.length) return;
      await visit(docs);
      last = docs.at(-1);
    }
  }
  // `plan(docs, batch)` adds at most one write per document, so each page
  // commits as one batch of at most 500 writes.
  async function pages(collection, plan) {
    let written = 0;
    await scan(collection, async docs => {
      const batch = db.batch();
      const count = plan(docs, batch);
      if (!count) return;
      await batch.commit();
      written += count;
      await faults.afterBatch?.();
    });
    return written;
  }
  async function count(collection, matches) {
    let found = 0;
    await scan(collection, docs => { found += docs.filter(matches).length; });
    return found;
  }

  // Tombstone scoped records written before the fence and drop their receipts.
  // A record or receipt from a newer generation belongs to a later opt-in and stays.
  async function eraseCategories(uid, receipt) {
    const account = db.doc(paths.account(uid));
    const scoped = receipt.scope.categories;
    const fenced = (category, generation) => scoped.includes(category) &&
      generation < receipt.fenced_consent[category].generation;
    const liveRecord = doc => fenced(doc.get('category'), doc.get('consent_generation')) &&
      doc.get('content.state') !== 'deleted';
    const scopedReceipt = doc => fenced(doc.get('operation.category'), doc.get('operation.consent_generation'));
    const tombstone = (docs, batch) => docs.filter(liveRecord).map(doc => batch.update(doc.ref,
      { content: { state: 'deleted' }, revision: doc.get('revision') + 1 }, { lastUpdateTime: doc.updateTime })).length;
    const remove = (docs, batch) => docs.filter(scopedReceipt).map(doc => batch.delete(doc.ref)).length;
    const records = await pages(account.collection('records'), tombstone);
    const receipts = await pages(account.collection('receipts'), remove);
    const left = await count(account.collection('records'), liveRecord) +
      await count(account.collection('receipts'), scopedReceipt);
    if (left) throw new VerificationFailed();
    return { records, receipts };
  }

  // Everything under users/{uid} except the fence document and cleanup_jobs.
  async function eraseAccount(uid) {
    const account = db.doc(paths.account(uid));
    const erasable = async () => (await account.listCollections()).filter(c => c.id !== 'cleanup_jobs');
    let documents = 0;
    for (const collection of await erasable()) {
      documents += await pages(collection, (docs, batch) => docs.map(doc => batch.delete(doc.ref)).length);
    }
    for (const collection of await erasable()) {
      if (!(await collection.limit(1).get()).empty) throw new VerificationFailed();
    }
    return { documents };
  }

  // Last: the owner can reauthenticate to check or retry until content is gone.
  async function deleteUser(uid) {
    await auth.deleteUser(uid).catch(error => { if (error?.code !== 'auth/user-not-found') throw error; });
    const still = await auth.getUser(uid).then(() => true,
      error => { if (error?.code === 'auth/user-not-found') return false; throw error; });
    if (still) throw new VerificationFailed();
  }

  // Resume every open job whose backoff has passed. Returns when those runs settle.
  async function sweep() {
    const { docs } = await db.collectionGroup('cleanup_jobs').where('receipt.state', 'in', OPEN).get();
    const runs = docs.filter(doc => doc.ref.parent.parent?.parent.id === 'users').flatMap(doc => {
      const uid = doc.ref.parent.parent.id;
      const due = doc.get('receipt.state') !== 'failed_retryable' || !(doc.get('runner.next_attempt_at_ms') > now());
      return due && !active.has(`${uid}/${doc.id}`) ? [run(uid, doc.id)] : [];
    });
    await Promise.all(runs);
    return runs.length;
  }

  return { create, read, run, resume, sweep };
}
