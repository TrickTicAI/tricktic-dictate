// Reference authorization and fence step for trusted account cleanup (MW-OUATH-5
// interface, implemented as a service by MW-OUATH-8). Admin credentials bypass
// Security Rules, so this path validates ownership itself. See
// infra/firebase/trusted-cleanup.md for the full job lifecycle.
import { equal, initialAccount, paths } from './account-sync.firestore.mjs';

export const MAX_AUTH_AGE_SECONDS = 300;
const categories = ['settings', 'dictionary', 'transcripts'];

/**
 * The ownership check every trusted endpoint shares: a Google Firebase token of
 * this project for exactly `uid`, and, when `nowSeconds` is given, a Google
 * sign-in no older than MAX_AUTH_AGE_SECONDS. Returns an error kind or null.
 */
export function callerError(projectId, claims, uid, nowSeconds) {
  if (claims?.aud !== projectId || claims.iss !== `https://securetoken.google.com/${projectId}` ||
      claims.firebase?.sign_in_provider !== 'google.com' || typeof uid !== 'string' || claims.sub !== uid) {
    return 'not_owner';
  }
  if (nowSeconds !== undefined && !(nowSeconds - claims.auth_time <= MAX_AUTH_AGE_SECONDS)) return 'reauth_required';
  return null;
}

/**
 * `claims` must come from Admin SDK verifyIdToken(token, checkRevoked = true);
 * never from a decoded-but-unverified token or from request JSON.
 * `account` is the stored users/{uid} document, or null when the account never
 * turned sync on: that is the all-off initial account (epoch 1, revision 1).
 * Returns { receipt, writes } for a new job, { receipt, writes: [] } for an
 * identical retry, or { error } without side effects.
 */
export function planCleanup({ projectId, claims, request, account, existingJob, jobId, nowSeconds }) {
  const error = callerError(projectId, claims, request.uid, nowSeconds);
  if (error) return { error };
  if (existingJob) {
    return equal(existingJob.request, request) ? { receipt: existingJob.receipt, writes: [] }
      : { error: 'idempotency_conflict' };
  }
  account ??= initialAccount(request.uid);
  if (account.consent?.uid !== request.uid) return { error: 'not_owner' };
  if (account.account_state !== 'active') return { error: 'account_unavailable' };
  if (account.account_epoch !== request.account_epoch) return { error: 'stale_epoch' };
  if (account.consent.revision !== request.expected_consent_revision) return { error: 'stale_consent' };

  const scoped = request.scope.kind === 'account' ? categories : request.scope.categories;
  const next = structuredClone(account);
  // Disable and advance every scoped generation before acknowledging the job.
  for (const category of scoped) {
    const toggle = next.consent[category];
    toggle.enabled = false;
    toggle.generation += 1;
    if (category === 'transcripts') toggle.history = { mode: 'off' };
    // `everywhere` tells each device to drop its synced copies on reconnect.
    if (request.effect === 'everywhere') {
      next.resets = { ...next.resets, [category]: { generation: toggle.generation, job_id: jobId } };
    }
  }
  next.consent.revision += 1;
  if (request.scope.kind === 'account') {
    next.account_epoch += 1;
    next.account_state = 'deleting';
    next.consent.account_epoch = next.account_epoch;
  }
  const { expected_consent_revision, ...binding } = request;
  const receipt = { ...binding, job_id: jobId, state: 'pending',
    fenced_account_epoch: next.account_epoch, fenced_consent: next.consent };
  return { receipt, writes: [
    { path: paths.account(request.uid), data: next },
    { path: paths.cleanupJob(request.uid, request.operation_id), data: { request, receipt } },
  ] };
}
