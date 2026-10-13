// HTTP API v1 of the trusted cleanup service (infra/firebase/trusted-cleanup.md).
// Admin credentials bypass Security Rules, so every route authorizes the caller
// itself. Failures carry a fixed error kind and never any upstream text.
import { createVerifier } from './auth.mjs';
import { createJobs } from './jobs.mjs';
import { callerError, validCleanupRequest, validOperationId } from './contract.mjs';

const BODY_LIMIT = 8192;
const RATE_WINDOW_MS = 60_000;
const SWEEP_MS = 5 * 60_000;
const statuses = { unauthenticated: 401, reauth_required: 401, not_owner: 403, not_found: 404,
  invalid_request: 400, account_unavailable: 409, stale_epoch: 409, stale_consent: 409,
  idempotency_conflict: 409, rate_limited: 429, unavailable: 503 };

class Refusal extends Error {
  constructor(kind) { super(kind); this.kind = kind; }
}
const refuse = kind => { throw new Refusal(kind); };

// One JSON line per event: job IDs, states, counts and reason kinds only.
export const jsonLog = (event, fields = {}) =>
  process.stdout.write(`${JSON.stringify({ time: new Date().toISOString(), event, ...fields })}\n`);

export function createService({ projectId, auth, db, now = Date.now, log = jsonLog, faults,
  waitMs = 10_000, rateLimit = 20 }) {
  const verify = createVerifier(auth);
  const jobs = createJobs({ db, auth, now, log, faults });
  const hits = new Map();
  let timer = null;

  function limit(uid) {
    const since = now() - RATE_WINDOW_MS;
    const recent = (hits.get(uid) ?? []).filter(time => time > since);
    if (recent.length >= rateLimit) refuse('rate_limited');
    recent.push(now());
    hits.set(uid, recent);
  }

  async function caller(request, options) {
    const verified = await verify(request.headers.authorization, options);
    if (verified.error) refuse(verified.error);
    limit(verified.claims.sub);
    return verified;
  }

  function body(request) {
    if (!/^application\/json(;|$)/i.test(request.headers['content-type'] ?? '')) refuse('invalid_request');
    if (Number(request.headers['content-length']) > BODY_LIMIT) refuse('invalid_request');
    return new Promise((done, fail) => {
      const chunks = [];
      let size = 0;
      request.on('data', chunk => {
        size += chunk.length;
        if (size <= BODY_LIMIT) chunks.push(chunk);
      });
      request.on('end', () => {
        if (size > BODY_LIMIT) return fail(new Refusal('invalid_request'));
        try { done(JSON.parse(Buffer.concat(chunks).toString('utf8'))); }
        catch { fail(new Refusal('invalid_request')); }
      });
      request.on('error', () => fail(new Refusal('invalid_request')));
    });
  }

  async function cleanup(request) {
    const payload = await body(request);
    const { claims } = await caller(request);
    if (!validCleanupRequest(payload)) refuse('invalid_request');
    const plan = await jobs.create({ projectId, claims, request: payload });
    if (plan.error) refuse(plan.error);
    const { uid, operation_id } = payload;
    if (plan.receipt.state !== 'completed') {
      const run = jobs.resume(uid, operation_id, await jobs.read(uid, operation_id));
      let timeout;
      await Promise.race([run, new Promise(done => { timeout = setTimeout(done, waitMs); })]);
      clearTimeout(timeout);
    }
    return { receipt: (await jobs.read(uid, operation_id)).receipt };
  }

  // A user deleted by an account job may still read that completed job, and
  // nothing else, while its last ID token is unexpired.
  async function status(request, operationId) {
    if (!validOperationId(operationId)) refuse('invalid_request');
    const { claims, deleted } = await caller(request, { deletedUser: true });
    const error = callerError(projectId, claims, claims.sub);
    if (error) refuse(error);
    const job = await jobs.read(claims.sub, operationId);
    if (!job || (deleted && !(job.receipt.scope.kind === 'account' && job.receipt.state === 'completed'))) {
      refuse('not_found');
    }
    return { receipt: job.receipt };
  }

  async function revoke(request) {
    const payload = await body(request);
    const { claims } = await caller(request);
    const keys = payload && typeof payload === 'object' && !Array.isArray(payload) ? Object.keys(payload) : [];
    if (keys.length !== 2 || payload.schema_version !== 1 || typeof payload.uid !== 'string') {
      refuse('invalid_request');
    }
    const error = callerError(projectId, claims, payload.uid, Math.floor(now() / 1000));
    if (error) refuse(error);
    await auth.revokeRefreshTokens(payload.uid);
    log('sessions_revoked');
    return { revoked: true };
  }

  function route(request) {
    // Browsers are not a client of this API: no CORS, and any Origin is refused.
    if (request.headers.origin !== undefined) refuse('invalid_request');
    const { pathname } = new URL(request.url, 'http://service.invalid');
    if (request.method === 'POST' && pathname === '/v1/cleanup') return cleanup(request);
    if (request.method === 'POST' && pathname === '/v1/sessions/revoke') return revoke(request);
    const job = /^\/v1\/cleanup\/([^/]+)$/.exec(pathname);
    if (request.method === 'GET' && job) return status(request, job[1]);
    refuse('not_found');
  }

  async function handle(request, response) {
    let status = 200;
    let payload;
    try {
      payload = await route(request);
    } catch (error) {
      const kind = error instanceof Refusal ? error.kind : 'unavailable';
      if (!(error instanceof Refusal)) log('request_failed');
      status = statuses[kind];
      payload = { error: kind };
      request.resume();
    }
    const text = JSON.stringify(payload);
    response.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store',
      'content-length': Buffer.byteLength(text) });
    response.end(text);
  }

  const sweep = () => jobs.sweep().catch(() => { log('sweep_failed'); return 0; });
  return {
    handle,
    jobs,
    sweep,
    start() {
      void sweep();
      timer = setInterval(sweep, SWEEP_MS);
      timer.unref();
    },
    stop() { clearInterval(timer); },
  };
}
