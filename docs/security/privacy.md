# Privacy and data lifecycle

This describes the implementation reviewed on 2026-09-20 at `cc6531d` and
rechecked on 2026-09-24 at `c08fe22` (MW-RELEASE-6), not a promise that a future
release has passed its security gates. Read the
[security policy](../../SECURITY.md) for supported releases and reporting, and
[permissions guide](permissions.md) before granting access. This is technical
product documentation, not a legal compliance certification. The public privacy
policy, terms and security policy for the Windows and Play releases are at
`https://legal.tricktic.com/tricktic-dictate/`; their clause-to-code
trace is release/legal.

## Optional desktop accounts and sync

Desktop releases require Google/Firebase account configuration from 2026-10-04.
Account-free use remains available. Sign-in alone uploads no settings/history;
settings/dictionary and text-only transcript history require separate opt-ins.
Enabled data and device/revision metadata live in the `metrix-writer` Firestore
project (`me-west1`). Firebase Authentication handles identity. Sync excludes
audio, provider keys, models and captured application/clipboard context and is
not end-to-end encrypted. See live acceptance
for the implemented boundaries and remaining limits.

Sign-out and disabling sync do not erase cloud or local copies. Account controls
support cloud-data deletion, account deletion and revocation through
`https://writer-account.tricktic.com`. Local recordings, original transcripts and provider keys survive account deletion.
Users can separately keep/remove downloaded account copies or request deletion
everywhere; offline devices can retain copies until reconnecting.
Anonymous checks on 2026-10-09 confirmed the desktop account/privacy text is
live at legal.tricktic.com. The current Play build remains account-free.
MW-RELEASE-12's receipt records
live deletion/disclosure publication, accepted Play declarations and published
OAuth links separately. The owner confirmed Google Play approval on 2026-10-12;
the public Data safety page independently shows the submitted categories.

Account deletion removes synced content and the Firebase user, but the account
fence and cleanup-job receipts remain for stale-write prevention and retry
safety. They include identifiers and consent/deletion state, with no automatic
expiry; they contain no dictation content. Firebase Authentication processes
IP addresses and SDK/device/app metadata and has its own retention rules.
The account-enabled worksheet
traces identity, every synced category and this metadata. Local audio and raw
transcripts remain independent of cloud deletion and model execution.

## Where processing happens

| Choice | Where audio goes | Where text goes |
| --- | --- | --- |
| Desktop local speech model | Saved on this computer; processed by its local runtime | Stays here unless cloud text processing, copy, export or insertion is used |
| Desktop cloud speech model | Saved here and sent to the selected speech provider | Returned here; optional text processing can send it to a different provider |
| Browser with server-local speech | Saved in browser storage and uploaded to the configured Writer server; inference runs on that server | Saved on the server and cached by the browser; optional cloud text processing sends text onward |
| Browser with cloud speech | Saved in browser storage, uploaded to the Writer server, then sent to the speech provider | Returned through that server; optional text processing can use another provider |
| Android phone model | Saved and processed on this phone; no automatic cloud fallback | Stays here unless cloud polishing, copy, sharing or editor insertion is used |
| Android cloud speech | Saved on this phone and sent directly to the selected provider over HTTPS | Returned to the phone; optional polishing can send it to another provider |

Browser-local storage does **not** mean browser-local speech recognition.
New Android dictation does not require a Writer server. Its legacy server
connection exists to resume older saved requests, using their saved endpoint.
Writer does not provide automatic recording synchronization between devices.

Desktop/server speech adapters use OpenAI, Gemini or Grok where the selected
language is supported; Android also offers Groq. Cloud text choices include
OpenAI, Gemini, Grok and OpenRouter; Android also supports Groq text processing.
OpenRouter introduces its selected downstream model host as another recipient.
See the provider/model shown in Settings before recording or retrying. Changing
settings does not rewrite an already captured request's provider or key revision.

## Optional text and context can leave before Stop

Speech recognition and text processing are separate choices. A local speech
model with cloud Super, Command or cleanup is **not an entirely offline flow**.
Desktop Super can send provisional transcript text to the chosen text model
while you are still speaking. The factory Super profile enables **Process while
speaking**; a usable text model/key is still required. Turn that option off to
wait until the final transcript, or use Regular with cleanup off to avoid cloud
text processing. Canceling cannot retract a request already sent.

Desktop Modes can include application context (window title on Windows, app name
on Mac), selected text and clipboard text. Regular starts without these context options; Command enables selected text.
Enabled context is captured before microphone startup and can accompany both
early and final text-model calls. It can contain material unrelated to your
speech. A mode with a fallback model can send the same inputs to the fallback's
provider, and both requests can be in flight at once. Since MW-SEC-14, a
successful run erases the context from its attempt and capture snapshot, and
startup cleans older successful attempts and orphaned snapshots
(desktop context retention). A failed or
interrupted attempt keeps its context in plaintext for retry until bulk desktop
history deletion. Turning context off affects future captures only. Deleting
just one recording's audio does not remove job/text history.

Custom instructions and vocabulary hints may also reach a selected provider.
Deterministic corrections/snippets run locally on the processing host; protected
spans are withheld from optional formatting where implemented. This is not a
redaction service for arbitrary secrets in speech or enabled context. Review
text before inserting it into another app; model output is not guaranteed exact.

Android optional polishing sends transcript text, not audio, to its selected
text provider. It is off by default. Accessibility editor access is a separate
permission for targeting and verifying insertion; it is not desktop Modes context.

## Storage, keys and retention

| Data | Protection and lifetime |
| --- | --- |
| Desktop audio, raw/processed text, attempts and enabled context | Ordinary plaintext files under the user-owned data folder (`%LOCALAPPDATA%\MetrixWriter`, `~/Library/Application Support/MetrixWriter`), retained for recovery/retry until deleted. Context is kept only for failed or interrupted attempts. Open the folder from Settings → Data & Privacy. It is outside the installation directory. |
| Browser recordings and attempt cache | Per-origin/profile IndexedDB. Upload leaves the browser copy in place; browser eviction or clearing site data can remove it. Server copies have a separate lifetime. |
| Self-hosted recordings, jobs, vocabulary and mode data | Plaintext files on the operator's server. Server filesystem access, backups and configuration are under that operator's control. |
| Android audio, text, delivery recovery and attempts | Plaintext WAV/JSON in app-private storage. App sandboxing is not application-level encryption of dictation history. Saved or interrupted audio survives failed transcription. |
| Desktop provider keys | Windows DPAPI for the Windows account; macOS login Keychain. Native credential entry does not pass the key through the WebView. A provider key in the process environment (`OPENAI_API_KEY`, `GEMINI_API_KEY`, `XAI_API_KEY`, `OPENROUTER_API_KEY`) takes precedence over the stored one. |
| Browser/server provider keys | Held by the server process/environment, not returned to the browser. Environment configuration is not an encrypted credential vault. |
| Android provider keys | AES-GCM encrypted using an Android Keystore key, in storage excluded from backup. Earlier encrypted revisions can remain for pending requests until **Forget** removes them. |

Encrypted credentials do not encrypt audio, text, dictionary entries or context.
The app must decrypt keys in memory to call providers. A compromised account,
root/admin access or malicious same-user code is outside those storage protections.
Use device locking, OS disk encryption and private backup destinations.

There is no general automatic history-retention period. A failed model call does
not delete successfully saved audio or the independently retained raw transcript.
A canceled/discarded recording is an explicit deletion action, not a failed call.
Provider requests can continue or incur charges after a local cancellation or
uncertain retry. Writer cannot guarantee provider-side erasure, training exclusion,
a particular retention duration or exactly-once billing. Check the chosen
provider/account's current terms; local deletion does not contact every recipient.

## Delete, export, back up and uninstall

- **Desktop:** stop recording and allow pending processing to finish, then use
  Settings → Data & Privacy → **Delete all audio and transcripts**. It removes
  recordings, jobs, mode histories/captured context and the current origin's
  attempt cache. Models, dictionary, settings and provider keys remain. This is
  file deletion, not secure overwriting or deletion of backups and exports.
- **Individual desktop/server audio deletion:** preserves job/raw/processed text
  history. Use bulk desktop history deletion for that installation's full history.
  Do not treat the recording DELETE API as a transcript-erasure endpoint.
- **Browser:** clearing site data does not delete server recordings. Server
  deletion does not erase browser downloads, old origins or other profiles.
  Coordinate server retention with the operator; browser mode has no bulk
  desktop-history button.
- **Android:** History deletes individual saved dictations; Settings → Storage
  & about offers **Delete all audio and transcripts**. Active/pending dictations
  are kept and reported. Models, dictionary, settings and keys remain. Since
  MW-SEC-13, deletion also erases the dictation's export snapshots, revokes their
  share grants and clears leftover phone-inference text; expired export copies
  and orphaned caches are pruned on every start. A recipient app that already
  opened a share may keep what it read.
- **Export/share/copy:** creates another copy under the destination's control.
  Delete it there separately, including trash, clipboard managers and backups.
  Revoking a grant cannot recall bytes a recipient already copied.
- **Backups:** desktop/server and browser-profile backups may include plaintext
  content. Android disables OS backup and excludes app data from cloud backup
  and device transfer; do not depend on reinstall restoring recordings or keys.
  Export important recordings to a destination you control before removal.
- **Uninstall:** desktop repair/uninstall leaves its data folder and may leave
  Keychain credentials and old WebView profiles. On Windows, the uninstaller's
  "Delete app data" option currently removes only `com.metrix.writer` folders,
  not `%LOCALAPPDATA%\MetrixWriter` (finding F1 in
  release/legal). Remove keys through Writer and
  delete history before uninstalling when possible; review the saved data folder
  and your backups separately. Android uninstall/clear app storage removes its
  local app data and keys, not recipient exports or provider-held copies.

## Clipboard, downloads and update traffic

Desktop paste publishes text to the OS clipboard. **Preserve clipboard after
dictation** restores prior contents where possible; it does not undo reads,
Windows clipboard history, cloud synchronization or third-party clipboard-manager
storage. By default Windows dictation is marked eligible for clipboard history.
**Private clipboard delivery** (MW-SEC-15, off by default) asks Windows to
exclude it from monitoring, history and cloud clipboard, and uses concealed and
transient pasteboard types on macOS. It cannot recall previously copied text.
Android editor insertion and explicit Copy/Share have different destinations;
check the receiving app before delivering sensitive material.

Local inference still needs model/runtime downloads during setup (Hugging Face
and the HTTPS hosts it redirects to) and can coexist with update checks. The
desktop checks `updates.tricktic.com` about 10 seconds after the window opens
and every 6 hours, with no identifier or query; the Android `dev` build's
updater contacts it too. The Android `play` build has no updater and is updated
by Google Play. Artifact requests can go to the URLs supplied by the feed.
Opening cloud setup with an unchecked key, and desktop warm-up while speaking,
send an authenticated request that carries only the key.
These hosts/providers receive normal connection metadata such as IP addresses.
Selecting a local engine is not a switch that blocks every network request.

Implementation evidence and release checks tie these
statements to source and identify the remaining release acceptance work.
# Desktop storage controls

Settings → Privacy & data shows, under Storage, current history usage and its
configured device limit, recording audio included in that usage,
text/processing/settings bytes, and downloaded model bytes separately, as one
bar and three figures. It uses the quota walk's file identities so
native/imported WAV and prepared PCM hard links count once. These are logical
file sizes, not filesystem allocation; WebView caches, app binaries and exported
copies are outside this readout. Models are outside the history quota.

Free up space chooses what to delete (audio only, or audio and transcripts) and
from when (before today, or every dictation), which gives four local deletions.
Each says what it deletes and keeps before anything is reviewed, and each is
reviewed before it runs:

- Audio only, before today: transcripts and processing history stay.
- Audio and transcripts, before today: the selected recordings' attempts and
  mode context go too. Today stays.
- Audio only, every dictation: transcripts and processing history stay.
- Audio and transcripts, every dictation: the existing complete history wipe.

“Before today” means strictly before local midnight today on this computer.
The date and recording IDs are frozen for confirmation; recordings created later
are outside that selection. Retries are classified by the original recording
date, including recordings whose audio was already deleted. Undated/corrupt
records are not silently classified as old. Active capture, queued transcription,
text execution or upload refuses deletion. Confirmations are local and do not
delete model weights, settings, dictionary or provider keys.

Audio-only deletion leaves transcript sync and caches alone. Date-limited history
deletion removes older original transcript families and their derived local
copies across account catalogs and kept copies. It stops history sync on this
device, clears pending uploads and fences stale source snapshots. Newer local
families remain. No cloud tombstones are sent; cloud copies remain and can be
restored if the user reenables sync. Account settings provide separate cloud
deletion. Current sync stores optional text/settings/dictionary data in Writer's
Firebase Cloud Firestore project (`me-west1`); it never uploads audio.

---

▶️ Follow TrickTic on [YouTube](https://www.youtube.com/@TrickTicAI).
