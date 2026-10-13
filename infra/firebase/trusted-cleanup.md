# Trusted account cleanup interface v1

Status: interface specified by MW-OUATH-5. MW-OUATH-8 implements the service
([`cleanup-service/`](cleanup-service/)), its HTTP API below and the native
account controls. Nothing here is provisioned or deployed. The
cleanup contract
(MW-OUATH-1) owns the request/receipt shapes. This document says how a trusted
path applies them to the Firestore layout.

## Why a trusted path

Clients can only write through [Security Rules](firestore.rules). The rules let a
client tombstone its own records, but not advance an account epoch, set the
`deleting` fence, publish `resets`, erase receipts or delete the Firebase Auth
user. Those need Admin SDK or IAM access, which **bypasses Security Rules**.
So the trusted path authorizes every request itself. Never ship an admin
credential in an app, and never accept a UID from request JSON as proof of identity.

Spark (no billing) has no Cloud Functions or Cloud Run. On 2026-09-25 the maintainer
chose to run the service on an existing self-managed VPS instead of upgrading
the project to Blaze. See [Hosting](#hosting) for the billing effect.

## Request authorization

The reference is `planCleanup` in
[account-sync.cleanup.mjs](../../packages/contracts/account-sync.cleanup.mjs);
its tests cover each refusal. For each `AccountCleanupRequest`:

1. Verify the caller's Firebase ID token with the Admin SDK and
   `checkRevoked = true`. The token travels in the `Authorization` header, never
   in the JSON body.
2. Require `aud` = the Writer project ID,
   `iss` = `https://securetoken.google.com/<project>`,
   `firebase.sign_in_provider` = `google.com` and `sub` = `request.uid`.
3. Require a recent Google reauthentication: `auth_time` no older than 300
   seconds. Anything older returns `reauth_required`.
4. Read `users/{uid}` inside the same transaction as the fence. Require
   `consent.uid` = `request.uid`, `account_state` = `active`,
   `account_epoch` = `request.account_epoch`, and
   `consent.revision` = `expected_consent_revision`. A stale revision fails, and
   the client previews again.
5. Look up `users/{uid}/cleanup_jobs/{operation_id}`. If the exact same request
   is there, return its receipt unchanged. A different request with that
   operation ID is `idempotency_conflict`. Retries never move a fence forward twice.

## Fence before acknowledgement

In one transaction, before returning `pending`:

- Set `enabled: false` for every category in scope and advance its generation
  by one. Transcript history becomes `off`. `consent.revision` advances by one.
- `effect: everywhere` also writes `resets.<category> = { generation, job_id }`
  on the account document. A device whose reconciled generation is below it
  removes its downloaded copies of that category when it reconnects, and keeps
  unsynced originals.
- `scope: account` also advances `account_epoch`, sets `account_state:
  deleting` and copies the new epoch into `consent.account_epoch`.
- Write the job document `{ request, receipt }` at
  `users/{uid}/cleanup_jobs/{operation_id}`. Clients can read it but never write it.

Once this commits, the rules refuse every client write that carries the old
generation or epoch, and every write while the account is `deleting`. That
includes stale offline devices and receipts cached before the fence. The
emulator suite proves this after both a category and an account fence.

## Erasure and completion

The job moves from `pending` to `running`, then to `failed_retryable` or
`completed`. It runs in bounded batches of at most 500 writes, retries
idempotently, and pauses on `RESOURCE_EXHAUSTED`.

| Scope | Erase | Keep |
| --- | --- | --- |
| Category | Replace scoped records' `content` with `{state: deleted}` (revision +1); delete scoped receipts, which contain operation payloads. | Tombstones and the ID-only `families`/`attempts` indexes, consent, devices, jobs. |
| Account | Everything under `users/{uid}` except the account document and `cleanup_jobs`; then delete the Firebase Auth user **last**. | The account document as a content-free `deleting` fence (epoch, consent generations, resets) and the job documents. |

A job is `completed` only after a verification read finds no scoped live content
and no receipts left, and, for account scope, after the Auth user is gone. A
partial run is `failed_retryable`, never `completed`. Deleting the Auth user
last means the owner can still reauthenticate to check or retry the job while
content remains. After that, a new Google sign-in gets a new Firebase UID, and
data can't be matched to the old account by email. Cloud Storage has no Writer
data yet. When audio exists, its ownership check and erasure need their own
entry in this table.

Tombstone compaction is a separate trusted job. It may remove a tombstone
fenced at generation `G` only when every device document of the current epoch
reports `reconciled_generations.<category> >= G`. The only exception is a device
the user removed through account controls. A category generation reset makes
older devices reconcile before they write, so compaction can never bring back
deleted data.

## Service HTTP API v1

One small Node service holds the only admin credential. Desktop and Android
call it over HTTPS at the build-configured base URL (`WRITER_ACCOUNT_SERVICE_URL`).
Without that URL a build reports account deletion and session revocation as
`not_configured` and never guesses an endpoint.

Every request carries `Authorization: Bearer <Firebase ID token>`. The service
refuses a request that has an `Origin` header: browsers are not a client, so
no CORS is offered. JSON bodies are at most 8 KiB. The caller's UID always comes
from the verified token's `sub`, never from the path. Responses are JSON:
success bodies are below, and a failure is `{ "error": <kind> }` with no other text.

| Endpoint | Auth | Body → success |
| --- | --- | --- |
| `POST /v1/cleanup` | Verified, not revoked, `auth_time` ≤ 300 s old | `AccountCleanupRequest` → `200 { receipt }` |
| `GET /v1/cleanup/{operation_id}` | Verified and not revoked; see below for a deleted user | — → `200 { receipt }` |
| `POST /v1/sessions/revoke` | Verified, not revoked, `auth_time` ≤ 300 s old | `{ "schema_version": 1, "uid": <uid> }` → `200 { "revoked": true }` |

`POST /v1/cleanup` validates the request against the MW-OUATH-1 schema, applies
the fence (above) and starts the job. It waits up to 10 seconds for a terminal
state, so a small account usually answers `completed` directly. Otherwise it
returns the current receipt. An identical request is the idempotent retry: it
returns the same job and resumes it if it isn't running.

`GET /v1/cleanup/{operation_id}` reads the caller's own job. After an account
job deletes the Firebase Auth user, a token can no longer pass the revocation
check. Such a token may still read that account's **completed account-scope job
only**, and only if its signature verifies and it hasn't expired (Firebase ID
tokens last one hour). Clients keep the token from the deletion request in memory
for that final read. They never store it.

`POST /v1/sessions/revoke` calls Admin `revokeRefreshTokens`. Every device's
refresh fails from then on, and the next sensitive request from any of them needs
a new Google sign-in. Firestore rules can't see revocation: an ID token already
issued stays usable until it expires (at most one hour). Clients stop their own
listeners and outbox when they revoke. The sessions of other devices end at
their next refresh.

| Error | HTTP | Meaning / client action |
| --- | --- | --- |
| `unauthenticated` | 401 | Missing, malformed, expired or revoked token. Refresh; if that fails, sign in again. |
| `reauth_required` | 401 | Last Google sign-in is older than 300 s. Reauthenticate the same account, then retry the identical request. |
| `not_owner` | 403 | Token is not a Google Firebase token of this project for this UID. |
| `not_found` | 404 | No such job for this UID, or an unknown route. |
| `invalid_request` | 400 | Body fails the schema, malformed operation ID, unsupported content type, body too large, or an `Origin` header. |
| `account_unavailable` | 409 | Account is already `deleting`. |
| `stale_epoch`, `stale_consent` | 409 | Re-read the account document, preview again, and send a new operation ID. |
| `idempotency_conflict` | 409 | Operation ID reused for a different request. |
| `rate_limited` | 429 | More than 20 requests per UID per minute. |
| `unavailable` | 503 | Firestore/Auth unreachable or quota exhausted. The job, if created, resumes later. |

An account that never turned on sync has no `users/{uid}` document. The service
treats it as the all-off initial account (`initialAccount`: epoch 1, consent
revision 1), so a client that finds no account document sends `account_epoch: 1`
and `expected_consent_revision: 1`. Account deletion then still writes the
`deleting` fence and deletes the Auth user.

## Job runner

The service runs one process, so jobs are serialized per UID in memory. At
startup and every 5 minutes it sweeps `cleanup_jobs` (collection group) for
`pending`, `running` or `failed_retryable` receipts and resumes them. A retry
backs off exponentially, capped at one hour. Each step is idempotent:
tombstoning an already deleted record, deleting a missing document and deleting
an already deleted Auth user are all no-ops. So a crash at any point resumes
safely. A failure records `failed_retryable` with `runner: { failures, reason,
next_attempt_at_ms }` beside the receipt, starting at 30 seconds. An identical
client request resumes a failed job at once. The exception is
`RESOURCE_EXHAUSTED` (`reason: quota`), which waits for the sweeper's backoff.
The sweep query uses the collection-group index on `receipt.state` in
[firestore.indexes.json](firestore.indexes.json).

Category erasure touches only records and receipts written in a generation
below the fence. Anything a later opt-in writes in the new generation stays.
A record is tombstoned with an update precondition on its last update time, so
a concurrent change fails that batch and is re-read on the retry. Logs carry job IDs, states and counts
only: never a token, UID, email or record content.

## Hosting

| Item | Choice |
| --- | --- |
| Host | A self-managed VPS, selected 2026-09-25. Not provisioned. |
| Runtime | Node 22 LTS under a dedicated system user and systemd unit, listening on `127.0.0.1` only. |
| Public entry | An HTTPS reverse proxy on the VPS with a hostname the operator chooses. That hostname becomes `WRITER_ACCOUNT_SERVICE_URL`. |
| Credential | A service account in the `metrix-writer` project with only **Cloud Datastore User** and **Firebase Authentication Admin**. Its key file is readable only by the service user and never enters the repo, logs or an app. |
| Billing | The project stays on Spark. Admin reads and writes count against the same daily Firestore quota as clients (see the protocol page). Auth user deletion and token revocation carry no Firebase charge. The VPS is already paid for. |
| Region | Firestore stays `me-west1`. The VPS location doesn't change where data is stored. It only adds that network path for cleanup requests. |

Deployment steps are in [`cleanup-service/README.md`](cleanup-service/README.md).
Provisioning the service account, the proxy hostname or the unit needs the
operator's explicit go-ahead.
