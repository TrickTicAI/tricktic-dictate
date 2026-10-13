# Threat model

Reviewed 2026-09-20 against `cc6531d`. This model describes boundaries and residual
risks; it is not a penetration-test certificate. The
release audit and canonical remediation board carry
findings/status. A card in progress is not a shipped mitigation.

## Scope and assets

The intended public products are the current Tauri Windows/macOS app and native
Android app, subject to their platform release gates. The browser/API service is
private self-hosted infrastructure. The retired C#/WebView2 Windows shell is
outside public support and exists only for controlled data recovery/testing.

Protect microphone audio, raw and processed text, enabled application/selection/
clipboard context, vocabulary, provider credentials and billing authority,
installation/update integrity, and the user's ability to recover saved recordings.
Data retention supports retries: failed transcription/cleanup must not silently
delete saved audio or replace a valid raw transcript with fabricated output.

Trust the chosen OS account/device, installed Writer code and explicitly selected
providers to process the data the user gives them. An administrator/root user,
compromised same-user process, selected provider or recipient app can see data
within its authority. Those risks are not removed by app-private folders, DPAPI,
Keychain, Keystore or transport encryption.

## Boundaries and abuse cases

| Boundary / adversary | Existing controls at this baseline | Residual risk and release work |
| --- | --- | --- |
| Remote page or injected content → desktop native commands | Packaged UI, CSP, navigation/IPC command restrictions; in-process Axum relay rather than an always-on desktop API listener | WebView/command-boundary negative tests and exact packaged-artifact acceptance are required (MW-SEC-19). Do not assume arbitrary HTML is safe to execute. |
| Local/network client → speech workers or standalone API | Desktop relay is distinct from helper listeners; API binds loopback by default; legacy mobile routes authenticate separately | The main standalone router lacks auth/origin enforcement at this baseline; helper listener authentication and resource bounds need remediation (MW-SEC-1/2/4/5). Loopback is not authentication. Keep self-hosting private and do not expose helper ports. |
| Other Android app → exported components / editor access | OS service binding permissions, non-exported capture/inference services, read-only explicit export grants, focused-editor checks | Exported entry points and sensitive-field guards need native negative tests (MW-SEC-20); Accessibility permission itself grants broad OS access. |
| Other local user / lost unlocked device → retained content | Desktop user-folder protections and Android sandbox; platform-protected provider keys | Audio, transcripts and context are plaintext. Backups, administrator access and same-user malware remain risks. Minimize context and use OS disk encryption. |
| Malicious/oversized audio, model or request → parsers/resources | Pinned model/runtime identities and some request limits | Native libraries are an attack surface; patched attributable packages, target-specific scans, helper privileges and resource limits need gates (MW-SEC-3/4/6/7/8/9/16). |
| Selected cloud provider → speech/text/context | Explicit engine/model choice, HTTPS, separate raw results and optional text processing | Provider retention, billing and model output are outside local erasure. Early Super can transmit before Stop. Prompt instructions are not a confidentiality boundary. |
| Clipboard observer or recipient app → delivered text | Focus/selection guards and optional clipboard restoration | History/sync/managers or the recipient can retain text. Restoration is not erasure (MW-SEC-15). |
| Export recipient / stale cache → deleted Android history | App-private originals and time-limited export reads | Since MW-SEC-13, deletion erases export snapshots and revokes their grants, and startup prunes expired exports and crash text caches; recipient copies can never be recalled. |
| Update host, compromised publisher or stolen signing key → installed code | Desktop updater signatures; Android APK signer/package checks | Metadata binding, bounded downloads/cache revalidation, Windows publisher identity, macOS notarization and hosting/key recovery require separate evidence (MW-SEC-10/11/12/17, MW-54). A valid signature alone does not prove safe content. |

## Operating assumptions and non-goals

Use a supported OS, trusted installation source, private data directories and
providers appropriate to the data. Do not expose the self-hosted API as an
unauthenticated public service. Do not open local inference ports to a network.
Do not grant Accessibility or update installation authority to an unverified build.

Writer is not a secrets vault, secure-erasure tool, end-to-end encrypted sync
system, malware sandbox or a guarantee that model output preserves meaning.
Cloud transcription/text processing necessarily discloses input to the selected
recipient. A local speech model alone does not make optional cloud text processing
local. Logs, private crash receipts, audio fixtures and credentials must not be
attached to public issues without review and redaction.

## Incident handling and change review

Use the private route in [SECURITY.md](../../SECURITY.md). Triage separates observed
impact from theoretical attack paths and identifies affected platform, version,
artifact and data types. Preserve only necessary, access-limited evidence. Stop
an affected distribution or isolate a private service when warranted; credential
revocation, key rotation and release withdrawal are explicit owner actions.

Before releasing changes to capture, context, provider routing, retention,
permissions, native commands, downloads or signing, update this model and the
[user privacy guide](privacy.md), then repeat the relevant platform acceptance.
MW-SEC-21/22/23 gate exact Windows/macOS/Android artifacts; MW-SEC-24 records the
public-release decision and residual risks. None is implied complete by this card.

---

▶️ Follow TrickTic on [YouTube](https://www.youtube.com/@TrickTicAI).
