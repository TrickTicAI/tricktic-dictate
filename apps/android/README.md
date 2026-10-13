# Writer Android

Writer is a native Kotlin/Jetpack Compose app with Home, History, Dictionary,
Statistics and Settings. Its Paper, Ink, Sage and Ocean themes and adaptive launcher icon retain
Writer's desktop identity. A configurable rounded-square overlay expands into
**cancel / live microphone waveform / finish**. Drag it to the bottom Hide target to hide
it for 10, 30 or 60 minutes; Settings and Home can restore it immediately.

**No Writer server is required for new dictation.** Choose direct Gemini, OpenAI, Groq or Grok HTTPS
with a user-owned key encrypted by Android Keystore, or a pinned whisper.cpp
model that runs on this phone. Both engines are included in this branch. There
is no automatic cloud fallback from phone inference. Optional cloud cleanup is
explicit for phone transcripts and sends text, not audio.

Start with **Settings → Set up Writer**. Cloud, Models, Speech language and Permissions
have separate pages. Recording from Home or the floating control saves audio
before transcription. History retains audio, original text, dictionary output,
cleanup and delivery recovery. Settings → Dictation engine switches between cloud
and phone engines both ways. Text is inserted into the editor focused when it is ready
(Insert Anywhere, MW-72), verified by readback, and Writer never presses Send. See the redesign and validation notes,
cloud behavior,
phone engine and
signing/upgrade.

## Official Play and private dev builds

- **TrickTic Dictate** (`play`, `com.tricktic.dictate`) installs and updates
  through Google Play. It contains no self-updater or install-packages
  permission, and the first public release has no Google sign-in.
- **TrickTic Dictate Dev** (`dev`, `com.metrix.writer`) retains the maintainer's
  private in-app updates and existing signing identity.

They install side by side with separate storage, keys, permissions and version
numbers. Neither is an upgrade or data migration for the other. Keep both apps'
records when testing updates; do not clear or uninstall them.

Play 1.0.2 passed the security gate and S25 internal-track update checks, but
Google rejected its production submission for **Missing prominent disclosure**
(reported on 2026-10-06). The disclosure correction
adds separate first-launch consent and direct Home retry. Corrected
**1.0.3 /1000003** passed the exact-artifact security gate, updated reviewer
demonstration and Google review; production publication and anonymous public
listing access were verified on 2026-10-08. See the
production receipt,
updates and
Play release runbook.

## Privacy and permissions

Read [the privacy guide](../../docs/security/privacy.md) and
[permission/revocation instructions](../../docs/security/permissions.md#android)
before setup. Audio and transcripts are plaintext in app-private storage;
Keystore encryption protects provider keys, not all dictation data. Phone speech
inference with cloud polishing enabled still sends text to a provider. Grok speech
is offered only for English; OpenRouter is text-only.

Accessibility reads the focused editor and selection to target and verify
insertion. Its OS grant allows window-content access; Writer's code restricts how
it uses that access. The overlay permission separately enables the floating
control. Microphone permission applies to both builds. Install-unknown-apps permission
applies only to the dev updater. Each can be revoked in Android Settings.

Exported/shared copies, provider-held data and clipboard contents are outside
History deletion. At the reviewed baseline, history deletion also leaves some
Writer-owned export snapshots/crash caches; see the privacy guide for the precise
limits and MW-SEC-13 follow-up. OS backup/device transfer are disabled; export
important recordings before clearing app data or uninstalling.

Use [SECURITY.md](../../SECURITY.md) for private vulnerability reporting and the
supported release policy. Public APK approval requires its platform acceptance;
a private “stable” channel does not by itself establish that approval.

## Optional Google sign-in (dev build only)

The `dev` build has **Settings → Account**: optional Google sign-in through
Credential Manager and Firebase Authentication (MW-OUATH-4). Signing in uploads
nothing, and dictation never depends on it. The `play` build has no sign-in code
until MW-RELEASE-12. Set `WRITER_ANDROID_FIREBASE_CONFIG` to the private
`google-services.json` when building; without it the page reports that sign-in
is not set up. See Android sign-in.

**Settings → Export & import** (both builds) saves and restores settings and the
dictionary as a versioned file, with a preview, atomic apply and undo; no account
needed. In the `dev` build, **Account → Backup & sync** syncs the same data with
the signed-in account after a preview and explicit consent (MW-OUATH-7). Keys,
recordings and device setup never leave the phone through settings/dictionary sync. See
Android sync, including
`scripts/account-sync-emulator.py` for the Firestore emulator suite.

The dev app also has separate transcript consent and text-only account History
(MW-OUATH-10). Transcript uploads require explicit opt-in with the Firebase privacy disclosure; see
Android transcript sync
for enrollment, offline recovery, deletion and validation evidence.

## Pinned build

Verified on Linux x86_64 with Java 21, Python 3 and Node 24. Requires Android SDK
platform `platforms;android-37.0`, NDK 28.2.13676358, CMake 3.22.1, build-tools 36.0.0 and platform-tools. Set `ANDROID_HOME` to the SDK
and put the chosen JDK on PATH / set `JAVA_HOME`. The emulator loop also requires
`system-images;android-36;google_apis;x86_64`, the emulator and host acceleration.
Install missing SDK packages explicitly using your installed `sdkmanager`; no
script accepts licenses, wipes AVDs or installs system packages silently.

Set `WRITER_WHISPER_SOURCE` to a read-only whisper.cpp checkout at
`927cfce34f31707e17f2bff35c349632fb9e2c3a` (see `benchmark/assets.json`).
The Gradle build verifies this revision; model weights stay outside source.

From a fresh repository worktree:

```bash
cd apps/android
./gradlew --version
./gradlew :app:assembleDevDebug :app:assembleDevDebugAndroidTest :editor:assembleDebug :editor:assembleDebugAndroidTest
./gradlew :app:assembleDevRelease :app:bundlePlayRelease :app:lintDevDebug :app:lintPlayRelease
```

The app has two flavors that install side by side:

| Flavor | Package | Launcher name | Updates |
| --- | --- | --- | --- |
| `dev` | `com.metrix.writer` (debug `.debug`) | TrickTic Dictate Dev, amber corner band | Private self-update feed, existing `writer-private` signer |
| `play` | `com.tricktic.dictate` (debug `.debug`) | TrickTic Dictate | Google Play only |

Updater code, `REQUEST_INSTALL_PACKAGES`, the install receiver and sideload
guidance live in `app/src/dev/`; `app/src/play/` supplies no-op hooks, so the
Play build compiles without them. Updater instrumentation tests are in
`app/src/androidTestDev/`. `dev.py` and `presentation-smoke.py` select a flavor
with `WRITER_ANDROID_FLAVOR=dev|play` (default `dev`); the other smoke scripts
use the dev build. See the Play bundle runbook.
`store-screenshots.py` captures the Google Play listing screenshots from the `play`
build; see the listing assets.

The standard wrapper pins Gradle 9.6.0 with its distribution checksum. AGP 9.4.1 with built-in Kotlin upgraded to 2.4.20 and a matching Compose compiler plugin,
Compose BOM 2026.08.00 and Navigation 3 version 1.1.7, dependency lockfiles and strict artifact checksum verification
pin dependency resolution. First build downloads dependencies; subsequent cached
builds can run `--offline`. Do not regenerate verification metadata to bypass an
unexpected checksum failure. Intentional dependency updates use `--write-locks
--write-verification-metadata sha256` followed by review of the changed artifacts.
Android tooling security records the
scoped host-library constraints, advisory dispositions and graph/APK checks.
Other host toolchains have not been validated; native Gradle Windows entrypoint
is `gradlew.bat` (platform-specific tool artifacts may need reviewed checksums).

Debug output: `app/build/outputs/apk/dev/debug/app-dev-debug.apk`. Without a signing
configuration, release output is
`app/build/outputs/apk/dev/release/app-dev-release-unsigned.apk`. For a privately signed
APK, use `python3 scripts/release.py build --output /private/new-candidate` after
one-time `python3 scripts/release.py init-key`. See
signing, delivery, upgrade and recovery.
Build caches/APKs are ignored. Gradle's debug signer is under the
user's `.android`, outside source. Minimum API is 33; target API is 36 and compile SDK is 37.0.
The old `node scripts/build.mjs` entrypoint now delegates to Gradle.

## Explicit-device loop

```bash
python3 scripts/dev.py devices
python3 scripts/dev.py start --port 5582
python3 scripts/dev.py install --serial emulator-5582
python3 scripts/dev.py test --serial emulator-5582
python3 scripts/presentation-smoke.py --serial emulator-5582
python3 scripts/floating-smoke.py --serial emulator-5582
python3 scripts/capture-smoke.py --serial emulator-5582
python3 scripts/dev.py collect --serial emulator-5582
python3 scripts/dev.py stop --serial emulator-5582
```

`start` creates/reuses only the marked, idle `Writer_API36` AVD (or
`Writer_API33`/`Writer_API34` with `WRITER_ANDROID_API=33`/`34`) under
`~/.local/share/metrix-writer/android-dev/avd`. `WRITER_ANDROID_DEV` can select a
separate private loop directory. The reservation, startup receipt (PID/serial/
ports/image), and emulator log live there too. It launches headlessly without
host audio, and refuses occupied ports, running serials and active reservations.
`stop` checks its receipt and AVD identity and stops only that emulator. A stale
reservation is an inspection task: verify the recorded process is gone before
removing the private `active.json`; never restart shared adb to repair a loop.
Install/test/collect always require an explicit ready serial.

`test` runs nine framework instrumentation tests against real AtomicFile/storage
and credential handling — seven `SessionStoreTest` and two `MobileConnectionTest`;
there are no emulator mocks of filesystem behavior. It names both classes
explicitly so the accessibility-dependent floating suite stays out of it.
`floating-smoke.py` separately runs twenty-one instrumentation scenarios covering the paused (not yet accepted) service, real
accessibility insertion, edge gestures, drag-to-snooze/restore, rotation, missing accessibility cursor metadata, unmarked empty-field placeholders with native field-identity checks, insertion at the field focused when the result arrives (other field, recreated field, away-and-back, auxiliary/focused windows), rich-text paragraph representation, results arriving during screen lock and visible
Settings grant/revocation flows, then checks delivery bytes across process
recreation. It requires the ownership receipt from `dev.py start` (including the
same `WRITER_ANDROID_DEV` if overridden), refuses non-emulator serials, leaves
accessibility revoked, and writes sanitized receipts to a private directory.
These are fixture-delivery checks, not speech recognition. See
the MW-57 receipts and editor support limits.

`presentation-smoke.py` runs nine Compose scenarios at normal text size and again
at 150% text with Hebrew app-locale RTL, captures private screenshots, restores Android
settings and the standard test runner, then runs 55 storage/audio/engine/UI-model
tests. It builds the required APKs and uses no live provider credentials.

The independent editor APK contains native English, Hebrew, mixed, password,
numeric and private fields plus an isolated WebView textarea and a Send counter.
Launch with:

```bash
adb -s emulator-5582 shell am start -W -n com.metrix.writer.editor/.EditorActivity
```

Grant microphone access through Writer onboarding first (the capture suite also
exercises that visible grant). The historical server fixture smoke (MW-59–61, before the Compose shell) starts an **owned fake API** on unused loopback port 18795,
generates a synthetic 16 kHz tone in private storage, and exercises the inherited
recording/force-stop/rotation/background/retry checks plus uncertain-request
identity and result persistence while the editor app is foreground. It checks
WAV/PCM hashes and previous originals after failed attempts. It closes its own API
in `finally`; receipts remain private. It requires a Writer-owned emulator and
never uses a connected phone automatically. No provider or microphone-quality
success is claimed. The retained `smoke.mjs` is fixture-only and is invoked by `fixture-smoke.mjs`,
which provisions a disposable endpoint-bound credential and builds its dedicated
test runner. These old server-shell UI runners are historical evidence, not current Compose
acceptance commands. The current direct-cloud and phone checks are described below.

The historical Windows-only `emulator.mjs`, `adb.mjs` and `start-gpu.mjs` retain
MW-14 host-specific evidence; use `dev.py` for this loop. Their old fixed paths,
serial and GPU runtime are not requirements on this host.

## Android security boundaries

Physical checks use the isolated S25 test package; never run the emulator Keystore failure suite on a phone.

MW-SEC-20 adds a separate-UID,
separate-signer hostile fixture for component/export access, forged update
callbacks, sensitive-input guards, Keystore failures and protected UI windows.
`python3 scripts/security-smoke.py --serial <owned-emulator> --directory <new-private-path>`
builds/installs the test APKs; add `--release` for a non-debuggable build with a
disposable fixture signer and release test-CA exclusion checks. It never uses
production signing keys or physical phone credentials. Follow the runbook to
restore the framework runner before the floating suite; its independently signed
editor cannot be overwritten by the ordinary debug-signed fixture.

## Export/migrate old spike recordings

Launch the old app once to recover any interrupted write. Keep it installed.
Install the new debug app alongside it, then use a private destination:

```bash
python3 scripts/transfer.py export --serial emulator-5582 --archive /private/writer-spike.tar
python3 scripts/transfer.py import --serial emulator-5582 --archive /private/writer-spike.tar
```

Export defaults to `com.metrix.writer.spike`; add
`--source-package com.metrix.writer.debug` to archive the new debug app. Source
and destination serials may differ. The tools stop only the selected Writer
package, never delete recordings, and refuse to overwrite an archive/session.
Import preserves all WAV/JSON bytes and uncertain request identities. Reopen
Writer after import. Archives contain private audio/text; do not add them to Git.
In-app export/share is available in History (MW-61); same-signer release upgrade
testing is documented in the private release guide.
The debug transfer tool cannot import into a non-debuggable release; retain the
old app/archive and explicitly add the key in Settings → API keys in the release.

For a repeatable synthetic migration proof, provide an old spike APK and a fresh
Writer emulator with no spike package installed:

```bash
python3 scripts/migration-smoke.py --serial emulator-5582 --legacy-apk /private/old-spike.apk
```

This installs only the old fixture package, seeds synthetic data, invokes the real
export/import commands, checks every byte in both app copies and verifies that a
second import refuses to overwrite. It never replaces an existing spike install.

## Standalone cloud validation

Run the normal framework runner after any specialized proof build:

```bash
./gradlew --offline :app:assembleDevDebug :app:assembleDevDebugAndroidTest :app:lintDevDebug
python3 scripts/dev.py install --serial emulator-5590
adb -s emulator-5590 shell am instrument -w \
  -e class com.metrix.writer.DirectCloudTest,com.metrix.writer.CompanionTest,com.metrix.writer.SessionStoreTest,com.metrix.writer.MobileConnectionTest,com.metrix.writer.AudioCaptureTest \
  com.metrix.writer.debug.test/android.test.InstrumentationTestRunner
```

Use only an owned, unconfigured emulator for `DirectCloudTest`; its key-revision
case explicitly creates and forgets fixture credentials. Keep speech, transcripts
and keys outside source. For a live check with an explicitly consented/public
PCM16 mono 16 kHz WAV and an owner-only key file:

```bash
WRITER_ANDROID_DEV=/private/writer-emulator python3 scripts/direct-smoke.py \
  --serial emulator-5590 --directory /private/new-direct-proof \
  --key-file /private/gemini.key --wav /private/consented.wav --consented
```

The helper checks emulator ownership, refuses a preconfigured cloud account,
canonicalizes the WAV header while preserving PCM, and uses the real Android
job manager, direct HTTPS transcription and optional cleanup. It checks retained
WAV hashes and raw/cleaned results, saves private evidence, and removes its
temporary phone key. It makes no physical microphone or editor-insertion claim.
The test instrumentation is separate from the production APK. Restore the normal
runner afterwards. `floating-smoke.py` and `capture-smoke.py` remain independent
editor/capture checks; their emulator results do not establish One UI behavior.

## History, dictionary and cleanup

History offers playback/export/share, saved-audio retries and explicit text
processing. Cloud provider/model/key revisions, dictionary and wording choices are frozen at
capture start; an interrupted request retains its credential revision and upload
reference. Resuming never grants automatic insertion. **Start a new cloud attempt
with current settings** explicitly replaces an unresolved attempt while retaining
its history and audio; a request already accepted by Google might be charged
again. There is no exactly-once promise from Google's generation API.

Dictionary corrections and exact snippets are applied on the phone. Cleanup is
off by default; when enabled, only editable text spans are sent to the chosen polishing provider and
exact replacements are retained locally. Original text and earlier cleaned text
survive cleanup failures. UI language remains independent of speech language.

History deletion also removes Writer-owned share snapshots and revokes their access.
Startup and History sweep expired shares and orphan phone results without touching
active inference. See cache-deletion behavior, limits and tests.

## Legacy server regression tools

MW-59–61 originally used an authenticated Writer mobile server. Their pairing
classes and pending-request engine remain only to recover old saved requests.
A new explicit direct-cloud attempt can use the same retained audio. Do not use
legacy `fixture-smoke.mjs`, `mobile-proof.py` or `cloud-smoke.py` to claim the new
standalone path works: those runners configure the former server topology.
Their historical commands/evidence remain in MW-59,
MW-60 and
MW-61. The historical backend runbook
is not Android onboarding.

## Phone-local mode

MW-64 owns the app's offline engine and
model controls. Its device/model receipts and the combined MW-65 acceptance must
establish actual network-disabled inference. The separate
[benchmark harness](benchmark/README.md) alone does not add an engine to this APK.

## Combined private-release acceptance

The MW-65 receipt records the exact
signed candidate and distinguishes the historical upgrade, same-APK installed
model retention, emulator inference and physical S25 checks. Follow the
combined signed-release command
after the exact historical upgrade. Debug automatic-delivery fixtures are absent
from signed release code; the release inspector rejects their DEX markers.

Ivrit-turbo was permanently retired from phone model choices
on 2026-09-14 after unacceptable S25 latency. Existing audio and originals remain;
a pending request for the retired model fails without loading it. No acceptable
phone-local Hebrew replacement is qualified yet. See the MW-65 receipt.

## MW-68 review candidate

The Android polish specification and acceptance record are in
MW-68. The candidate uses version
0.5.0/code 19 with the existing private signer. Readiness starts with real saved
microphone frames; routine successful insertion is quiet. History offers spoken
wording, saved corrections/snippets, polished text and insertion recovery.
Accessibility guidance appears before leaving Writer, and theme previews apply
to the app and overlay. The About page truthfully identifies private APK updates;
there is no Google Play listing or live update-availability check.

The review called for emulator validation and a subsequent shared phone review.
Emulator evidence does not complete physical first-word/Samsung acceptance or
MW-65's unresolved local Hebrew/mixed language qualification.
