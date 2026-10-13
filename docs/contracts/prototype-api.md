# Prototype API v1 — MW-02

This freezes payloads and extension points for the feature cards. Only health and
provider configuration discovery work today. All actions registered by MW-02
return HTTP 501 with `not_implemented`, including storage, transcription, job
lookup, language presets, vocabulary, settings and cleanup. There are no model
calls, durable jobs or durable audio in these stubs.

## Source of truth and validation

`apps/api/src/transcription.rs` owns Rust wire types. `packages/contracts/schema.json`
is generated JSON Schema draft 7; `packages/contracts/index.d.ts` is generated
TypeScript, imported with `import type { ... } from '@metrix-writer/contracts'`.
The catalog is a schema container, **not** an HTTP body. Each property describes
one endpoint's request or response. Domain objects are snake_case. New v1 fields
require coordinated regeneration; unknown fields are rejected rather than silently
ignored. A breaking change needs a new URL and envelope version.

Run `pnpm contracts:generate` after changing Rust types, then `pnpm validate` and
`pnpm typecheck`. Rust tests compare the generated schema and round-trip examples;
Node tests validate examples and invalid payloads; generation checks catch stale
TypeScript. Schemars generates schemas from Rust
([generator documentation](https://docs.rs/schemars/latest/schemars/generate/struct.SchemaSettings.html));
[json-schema-to-typescript](https://github.com/bcherny/json-schema-to-typescript)
generates declarations. Schema-only numeric range restrictions must also be checked
by implementing ingress handlers; Serde does not enforce Schemars range attributes.

## HTTP contract

Requests and successful responses use `{"api_version":"1","data":...}`. Errors
use `{"api_version":"1","error":{"code":"...","message":"...","retryable":false}}`.
The existing health and providers discovery responses retain their bootstrap
shapes for UI compatibility. Credentials never appear in any request or response.

| Route | Future behavior / success response |
| --- | --- |
| `POST /api/v1/recordings` | Multipart `metadata` JSON envelope (`RecordingMetadata`) and `audio` binary file; 201 `Envelope<Recording>` only after atomic durable save |
| `GET /api/v1/recordings/{id}` | 200 `Envelope<Recording>` |
| `GET /api/v1/recordings/{id}/audio` | Actual binary audio with recorded MIME, no JSON envelope |
| `DELETE /api/v1/recordings/{id}` | Explicit deletion; 204, no body |
| `POST /api/v1/transcriptions` | `Envelope<TranscriptionRequest>`; 202 `Envelope<Job>` (including idempotent duplicate) |
| `GET /api/v1/jobs/{id}` | 200 `Envelope<Job>`; poll for status |
| `GET/PUT /api/v1/settings` | `Envelope<Settings>`; settings UI wiring belongs to MW-10 |
| `GET /api/v1/language-presets` | Reserved for MW-07, currently 501; list payload to be added by its owner |
| `GET/PUT /api/v1/vocabulary` | Reserved for MW-08; use the shared `Vocabulary` shape |
| `POST /api/v1/jobs/{id}/cleanup` | Reserved for MW-09; explicit rerun policy to be added by its owner |

All listed actions currently return 501, regardless of payload. The route table
is an implementation target, not proof of ingestion or validation. Implementors
must validate before side effects and return 400 for malformed metadata/IDs,
404 for unknown IDs, 409 for unsaved audio or idempotency conflicts, 413 for an
actual configured byte limit, 422 for unsupported options, 503 for an unavailable
provider, and 501 for an unimplemented feature. A job failure is represented in a
200 job lookup, independently of request-level errors. Upstream authentication,
rate limits, timeouts and malformed results map to the structured error codes;
never forward upstream response bodies or secrets as messages. `retryable` means
the same options may succeed later; changing configuration/model is a separate
user choice. A 501 is not automatically retryable.

## IDs, audio and retention

Recording IDs, job IDs and idempotency keys are UUID strings. Browser recording
IDs are assigned before capture; a server upload retains that ID. IDs are opaque
and never filesystem paths. A second upload under the same ID must either match
the saved content and return it or fail with conflict; never overwrite retained
audio. Only server-resolved `SavedAudio` contains a filesystem path, and it is not
serializable/deserializable from the wire.

`created_at_ms`, `updated_at_ms`, `duration_ms` use milliseconds (timestamps are
Unix epoch); byte counts and times must be nonnegative safe JavaScript integers.
The MIME type is the actual container and codec reported by capture, not a fixed
filename extension. `capture_state` distinguishes complete and interrupted
capture from capture in progress. `audio.state` separately distinguishes upload
in progress, durably saved bytes and explicit deletion. Browser and server
storage locations are explicit; a saved browser recording is not a server upload.
Interrupted capture can be uploaded if nonempty/playable; transcription requires
server audio state `saved`. Providers validate their actual codec/size limits.

The upload is streamed to temporary storage and committed before job creation.
IndexedDB chunk storage is internal to MW-03; atomic server storage is internal to
MW-06. Chunk sequence and ASR overlap/segment metadata remain internal, so neither
can force clients to treat MediaRecorder fragments as independently decodable
files. There is no product duration cutoff. MW-06 must document actual whole-file
limits until segmentation exists. No storage implementation is selected here;
Tokio filesystem primitives support the initial file-based store without a new
DB service. Storage must retain unresolved job audio by default. No timer-driven
deletion is implied; deletion is an explicit action with active-job checks.

## Jobs, retries and original text

Submission is separate from audio ingestion. Persist the request/options snapshot
with the job. State goes `queued → transcribing → succeeded | failed`; restart
recovery must find queued/interrupted work. Persist raw text before cleanup.
`status.state = succeeded` always includes a result; failed jobs contain an error
and no invented transcript. A successful result has a separate cleanup state:
`off`, `pending`, `succeeded` with cleaned text, or `failed` with an error. Cleanup
failure does not turn successful transcription into failure or remove raw text.
An empty real transcript is allowed for silence; it is never fabricated on error.

A repeated identical submission with the same idempotency key returns the same
job. Reusing a key for a different payload is a conflict. An intentional retry
uses a **new key**, the **same recording_id**, and `retry_of_job_id` identifying the
previous attempt. The backend verifies the referenced job uses that recording.
The retry may select another provider/model, keeps the previous attempt and its
results, and never requests another microphone recording. Lost-response retries
reuse the key; intentional new attempts do not.

## Language, models and credentials

`input_language` is `en`, `he` or `mixed`. Mixed means preserving both languages,
not translating all speech. `output_language` is explicitly `{mode:"preserve"}`
or `{mode:"translate",language:"en"|"he"}`; unsupported translation must fail with
`unsupported_capability`, not silently fall back. `ui_language` is separate.
Input hints/instructions are not an accuracy guarantee.

`selection` contains only `provider` (`openai`, `gemini`, `local`) and the adapter's
model identifier. Fixture model names in examples are intentionally not production
recommendations. Each adapter validates nonempty/supported model names and its
language/output capabilities. Cloud adapters execute at `cloud`; local models
execute at `self_hosted_server`, never on the browser/device. These choices have
no device-offline guarantee. Local adapters must not download models implicitly
from a request; model files and executable paths are server configuration.

`OPENAI_API_KEY` and `GEMINI_API_KEY` are read on the server from this prototype's
explicit environment configuration. They must not enter contracts, browser
storage, logs or `VITE_*` variables. No keys are copied from the reference apps.
Configured means only a nonblank server setting; available requires an implemented
callable adapter and its prerequisites. It is not proof of a live successful
transcription. Stub adapters report `available: false` even with credentials.

Vocabulary hints bias ASR; corrections are deterministic postprocessing rules;
snippets insert exact text for spoken phrases. They are separate arrays, and the
original ASR text remains immutable. Cleanup is explicitly `off` or `enabled`
with its own model selection; it does not run by default.

## Extension points and ownership

`lib.rs` registers each feature router once. MW-06 can implement recording/job
handlers inside `recordings/` and `jobs/` without editing the root router. Cloud
(MW-04) and local (MW-05) adapters are registered through each module's `adapters()`
factory; `providers::adapters()` combines them for job orchestration. Factories
read server configuration only. The legacy provider-discovery handler remains a
bootstrap indicator; MW-10 wires runtime provider info into the existing UI.

`TranscriptionProvider` is an async object-safe interface taking a server-resolved
`SavedAudio` and `TranscriptionOptions`, returning raw `Transcript` or `ApiError`.
`RecordingStore` accepts an async byte reader and resolves UUIDs to saved audio.
`JobStore` defines atomic submission, lookup, status persistence and recovery.
Implementations and test doubles can be injected with `Arc<dyn ...>`; none of
these interfaces requires a key, SDK client, HTTP request or global process.
The stubs fail without consuming audio or making network calls.

Manifests predeclare multipart ingestion, Reqwest JSON/multipart/streaming with
Rustls, base64 for inline audio, UUIDs, async traits, Tokio streaming readers,
and Tokio filesystem/process/time/sync support for the
feature owners. The root ignore rule `recordings/` also matches the source module directory;
MW-02 explicitly tracks its owned `mod.rs`. Until that ignore rule is scoped,
stage new recording source files by exact path with `git add -f` (never audio).
The web workspace already depends on the types-only contracts
package. Settings and feature endpoints remain unavailable until their owners
provide real behavior; the initial recorder UI still uses its in-memory model.

## Wire examples

`packages/contracts/examples.json` contains complete request, saved recording,
successful job, settings and error examples. `retry-examples.json` contains a
provider timeout and the subsequent saved-recording retry with another model.
These are synthetic contract fixtures, not live provider output.

Normal result (inside `Envelope<Job>.data.status`):

```json
{"state":"succeeded","result":{"transcript":{"raw_text":"שלום Metrix","selection":{"provider":"openai","model":"fixture-stt"},"execution_location":"cloud","input_language":"mixed","output_language":{"mode":"preserve"}},"cleanup":{"state":"off"}}}
```

Retryable provider failure (same retained recording referenced by the job):

```json
{"state":"failed","error":{"code":"provider_timeout","message":"Provider timed out; saved audio is available for retry.","retryable":true}}
```

Saved-recording retry reuses `aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa`, references the
failed job `bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb`, and submits fresh idempotency key
`dddddddd-dddd-4ddd-8ddd-dddddddddddd`; its full options envelope is in the fixture.
