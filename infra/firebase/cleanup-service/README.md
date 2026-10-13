# Writer account cleanup service

The trusted path of [trusted-cleanup.md](../trusted-cleanup.md) (MW-OUATH-8):
it deletes cloud data and Writer accounts and revokes sessions for Desktop and
Android. It is the only holder of an admin credential. Deployed on the maintainers' VPS on
2026-09-26 (MW-OUATH-11); see the deployment record below.

| File | Role |
| --- | --- |
| `src/server.mjs` | Entry point: loopback HTTP server, sweeper start, clean shutdown. |
| `src/config.mjs` | Environment configuration; refuses a half-emulated or credential-less start. |
| `src/service.mjs` | HTTP API v1: routes, 8 KiB bodies, `Origin` refusal, fixed error kinds, per-UID rate limit. |
| `src/auth.mjs` | Firebase ID token verification (`checkRevoked`), including the deleted-user status read. |
| `src/jobs.mjs` | Fence transaction, erasure runner, verification, Auth user deletion and sweeper. |
| `src/contract.mjs` | Loads `planCleanup`, `callerError`, paths and the request schema from `packages/contracts`. |
| `writer-account-cleanup.service` | Example systemd unit. |

`planCleanup` in [`packages/contracts/account-sync.cleanup.mjs`](../../../packages/contracts/account-sync.cleanup.mjs)
stays the single reference for authorization and the fence. The service imports
it by relative path, so a deployment keeps `packages/contracts/` next to
`infra/firebase/cleanup-service/` in the same layout as the repo. The package
has its own lockfile outside the pnpm workspace, and nothing in it ships in an app.

## Configuration

| Variable | Meaning |
| --- | --- |
| `WRITER_FIREBASE_PROJECT_ID` | `metrix-writer`; a `demo-` project only with both emulators. |
| `GOOGLE_APPLICATION_CREDENTIALS` | Path of the service-account key file. Never the key itself. |
| `HOST`, `PORT` | Listening address; default `127.0.0.1:8787`. |
| `FIRESTORE_EMULATOR_HOST`, `FIREBASE_AUTH_EMULATOR_HOST` | Tests only; both or neither. |

Logs are JSON lines with job IDs, states, counts and fixed reason kinds. They
never contain a token, UID, email, request body or record content.

## Deployment

The maintainer chose a dedicated VPS on 2026-09-25 and authorized the deployment for
MW-OUATH-11 on 2026-09-26. It was done as below, with these specifics:

- Node 22.23.3 from the official tarball (checksum verified) in
  `/opt/writer-node`; the unit's `ExecStart` is overridden by the drop-in
  `/etc/systemd/system/writer-account-cleanup.service.d/node.conf`.
- Code from `df6d46a` in `/opt/writer-account-cleanup` (`SOURCE_COMMIT` beside it),
  owned by root; `/etc/writer-account-cleanup` is `root:writer-cleanup 0750`, the
  key `writer-cleanup 0400`.
- `https://writer-account.tricktic.com`: Cloudflare proxied A record and origin
  certificate in `/etc/writer-account-cleanup/tls/`; its own nginx server in
  `/etc/nginx/conf.d/writer-account.conf` (access log off, 16 KiB bodies, only
  `/v1/` forwarded). Other sites on the host and their configuration are unchanged.
- To update: `git archive` the new reviewed commit over the directory, `npm ci
  --omit=dev`, update `SOURCE_COMMIT`, `systemctl restart writer-account-cleanup`.

1. **Service account.** In the `metrix-writer` Google Cloud project, create a
   dedicated service account with only **Cloud Datastore User** and **Firebase
   Authentication Admin**. Create one JSON key. Install it on the VPS as
   `/etc/writer-account-cleanup/service-account.json`, owned by
   `writer-cleanup` with mode `0600`. The key never enters the repository, a
   log, a shell history or an app. Rotate it by creating a new key, restarting,
   then deleting the old key.
2. **Code.** Create a `writer-cleanup` system user. Install Node 22 LTS. Unpack
   the two directories from a reviewed commit, then install locked
   dependencies:
   ```bash
   git archive <commit> infra/firebase/cleanup-service packages/contracts \
     | sudo tar -x -C /opt/writer-account-cleanup
   sudo npm ci --omit=dev --prefix /opt/writer-account-cleanup/infra/firebase/cleanup-service
   ```
3. **Unit.** Install `writer-account-cleanup.service`, then `systemctl enable
   --now writer-account-cleanup`. It listens on loopback only.
4. **HTTPS.** Point the VPS's existing reverse proxy at `127.0.0.1:8787` on a
   hostname you choose, with a valid certificate. Forward only `/v1/`. Do
   not add CORS headers. That `https://` origin becomes
   `WRITER_ACCOUNT_SERVICE_URL` in the desktop and Android build configuration.
5. **Indexes.** The sweeper's collection-group query needs the `cleanup_jobs`
   `receipt.state` override in [firestore.indexes.json](../firestore.indexes.json).
   Deploy it with the rules when Firestore is provisioned.

The project stays on Spark. The service's reads and writes count against the
same daily Firestore quota as clients. Run exactly one instance: jobs are
serialized in memory, and a second instance could run the same job concurrently.
Every step is idempotent, but that is still wasted quota.

## Tests

```bash
npm ci --prefix infra/firebase/emulator; npm ci --prefix infra/firebase/cleanup-service
export JAVA_HOME=<JDK 21+>; export PATH="$JAVA_HOME/bin:$PATH"   # Node 22.9+ also required
node scripts/smoke/account-sync/cleanup-service.mjs
```

The runner starts private Firestore and Auth emulators on free ports with a
`demo-` project. It loads the real rules and signs synthetic Google users in on
the Auth emulator. The service runs in-process on real HTTP, with an injected
clock and fault hook. It never contacts a real project.

Two emulator limits apply. The Auth emulator's `verifyIdToken` always looks the
user up, even without `checkRevoked`, so the deleted-user status read goes
through a stand-in. For that one call, the stand-in does what production does
with an unsigned emulator token: it decodes the token and checks expiry,
audience and issuer. The emulator also still refreshes a revoked session, so
the suite checks the production-relevant property instead: a token refreshed
after revocation keeps its old `auth_time` and is refused.

### Result, 2026-09-25 (development host, `agent/mw-ouath-8`)

firebase-tools 15.31.0 (Firestore emulator 1.22.0 and Auth emulator),
firebase-admin 14.5.0 (@google-cloud/firestore 9.2.0), Node 22.23.2, OpenJDK 21.0.12.1.

| Suite | Result |
| --- | --- |
| `node scripts/smoke/account-sync/cleanup-service.mjs` | **11/11 passed** |
| `node scripts/smoke/account-sync/firestore-rules.mjs` | 35/35 passed (unchanged by the `planCleanup` update) |
| `pnpm test:contracts` | 63/63 passed, including the new absent-account `planCleanup` case; typecheck clean |

The service suite covers these refusals:
- missing, malformed and tampered tokens;
- an `Origin` header, an oversized body, the wrong content type, an extra field and a bad operation ID;
- B's token for A's UID, an anonymous-provider token, and a sign-in older than 300 s (on both cleanup and revoke);
- a stale epoch or consent revision.

It also covers these behaviors:
- **Category cleanup (dictionary + transcripts):** records are tombstoned at revision + 1 and their receipts deleted, while settings, families, attempts, devices and account B stay. Generations advance and categories are disabled. An identical retry returns the same receipt without moving the fence. A changed retry gets `idempotency_conflict`. A stale device's planner write and forced write are both denied by the rules, while a current-generation settings write succeeds.
- **`everywhere` resets:** written with the job ID.
- **Account deletion:** only `cleanup_jobs` remains. The fence is `deleting` at epoch 2, the Auth user is gone, and B is untouched. The last token reads the completed account job but not the earlier category job.
- **An account that never turned on sync:** it can be deleted.
- **Failure after the first batch:** the job is `failed_retryable`. The sweeper skips it until the backoff passes, then completes the same job with no second fence.
- **Quota failure:** an identical retry waits for the backoff, then completes.
- **Revoke:** the old token and a token refreshed from it are both refused.
- **Rate limit:** 20 requests per UID per minute, isolated per UID and reset by the window.

`npm audit --omit=dev` reports 2 moderate advisories: `uuid` < 11.1.1 through
`gaxios`, a dependency of firebase-admin 14.5.0. The advisory covers the
v3/v5/v6 `buf` argument. No non-breaking fix is available, and this service
does not call `uuid` itself.
