# Platform authentication adapter v1

Status: implementation boundary specified by MW-OUATH-2. **Live login/refresh
passed on Windows, macOS and Android**, with Android cancellation also verified;
see the setup record. The recorded desktop client and loopback
callback are qualified. Project registrations, audience and planned region/cost
assumptions are recorded. This v1 boundary is frozen for the downstream platform
cards. This document owns auth operations and events; MW-OUATH-1 owns data,
outbox and consent schemas. These are separate version namespaces, not competing
definitions of sync records.

## Identity and operations

Use `{ projectId, uid }` as the account key. `uid` is Firebase's UID, never the
Google subject, email, display name or a Remote Sessions identity. Optional
display metadata is presentation only. Initial provider is exactly `google.com`.
There is no Writer password registration/reset flow or anonymous auto-sign-in.

| Operation | Required behavior |
| --- | --- |
| `snapshot()` | Return current state and generation without initiating login or networking for a guest |
| `signInGoogle(attemptId)` | Explicit user action; one attempt at a time; Android native Credential Manager or qualified desktop external browser; return a sanitized result |
| `cancelSignIn(attemptId)` | Invalidate only that attempt; late callback/result cannot establish a session |
| `getFirebaseIdToken(accountKey, generation, forceRefresh)` | Native/internal only; return a valid Firebase token for exactly that account/generation or typed failure; never return Google tokens to a sync consumer |
| `signOut()` | Invalidate generation and stop account work first; clear protected local session and Credential Manager state; retain original local recordings |
| `reauthenticateGoogle(attemptId, accountKey)` | Require the same UID; selecting another account does not authorize the original account's deletion or sensitive operation |
| `subscribe(listener)` | Ordered state events plus immediate current snapshot; unsubscribe detaches only the listener |

No token, PKCE verifier, refresh credential or raw provider exception crosses
the Tauri/WebView/UI event boundary. Google tokens are short-lived exchange
inputs. Firebase refresh credentials remain in protected native storage (desktop
DPAPI/Keychain; Android SDK storage protection/backup policy must be verified).
Do not claim that Firebase SDK persistence alone proves Android Keystore
protection or backup exclusion. Persist a rotated refresh token atomically before
reporting success. A token refresh never enables sync or changes data categories.

## States, events and races

The UI sees `guest`, `signing_in`, `signed_in`, or `reauth_required`. Network
unavailability is a retryable operation error, not account deletion. Expose
only fixed error kinds: `not_configured`, `cancelled`, `timed_out`, `offline`,
`provider_rejected`, `invalid_response`, `reauth_required`, `unavailable`.
Never surface an upstream body, response URL or exception message as UI text.

Every event has `version: 1`, a monotonic process-local `sequence`, and a
monotonic `generation`. Generation advances whenever a session is invalidated,
including sign-out, switching and cancellation of a pending login. Pending work
captures both generation and `{projectId, uid}`; reject a completion if either
changed. On restart, discard all prior process work and publish restored state
only after protected-storage validation. Durable sync consent/outbox generations
belong to MW-OUATH-1 and must not be replaced with this process-local counter.

| Event | Payload / consumer action |
| --- | --- |
| `auth_snapshot` | Current state, optional account key and display metadata; never enables uploads |
| `sign_in_started` | Attempt ID; show optional login progress while dictation continues |
| `sign_in_finished` | Attempt ID, success/cancel/error; success followed by `auth_snapshot`; sync stays off |
| `session_refreshed` | Same account key and generation, no tokens; no consent effect |
| `session_invalidated` | Previous account key and reason; suspend listeners/outbox before publishing next account |

Sign out A before switching to B. Invalidate A before launching B's sign-in and
never emit A's delayed successful callback after invalidation. Reauthentication
does not replace A with B. Bind every cloud operation to its captured identity
and consent state again immediately before sending and applying a response.
Do not delete local captures when any auth operation fails. No Firestore request
is permitted merely because a `signed_in` event arrived; explicit sync consent
is checked independently by the sync adapter.

## Proof versus production adapter

The Node and Kotlin helpers qualify isolated exchanges only. They do not yet
implement this complete state machine, protected persistence, reauthentication,
account switching, native cancellation during an in-flight exchange, revocation,
or account deletion. Those belong to MW-OUATH-3/4/8; they consume this boundary
only after MW-OUATH-2's live gate is satisfied. The Android adapter (MW-OUATH-4) is
`AccountSession` in the app; see Android sign-in. The synthetic tests assert the
proof's implemented guards, not the unimplemented product lifecycle.
