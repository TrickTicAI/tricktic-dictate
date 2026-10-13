# Permissions and how to revoke them

Grant only the features you intend to use. Revocation prevents future access; it
does not delete saved recordings or recall text already delivered. See
[privacy and deletion](privacy.md) and the [security policy](../../SECURITY.md).
OS labels vary by release and device manufacturer.

## Android

Start at **Settings → Set up Writer** or **Settings → Permissions** inside Writer.

| Access | Why Writer uses it | How to revoke / effect |
| --- | --- | --- |
| Microphone | Records your dictation, including through the floating control's foreground recording service | Android Settings → Apps → Writer → Permissions → Microphone → Don't allow. New capture cannot start. |
| Display over other apps | Shows the floating microphone beside your existing keyboard | Android Settings → Apps → Special app access → Display over other apps → Writer → off. The floating control disappears; saved history remains. |
| Writer · voice insertion (Accessibility) | Finds the focused editable control, inspects text/selection and verifies insertion by reading it back; observes focus/window changes | Android Settings → Accessibility → Installed/Downloaded apps → Writer · voice insertion → off. Automatic editor insertion and Accessibility-driven floating behavior are unavailable. Use saved text in History. |
| Notifications | Shows recording status with Stop/Discard actions outside Writer | Android Settings → Apps → Writer → Notifications → off. This removes notification controls, not microphone permission or saved data. |
| Install unknown apps | Dev build only (the Play build neither requests nor needs it). Lets Writer submit a downloaded update to Android PackageInstaller | Android Settings → Apps → Special app access → Install unknown apps → Writer → Allow from this source → off. Self-installation is blocked; recording/history do not require this permission. |
| Internet/network state | Direct cloud processing, optional polishing, model downloads and, in the dev build only, update checks | No ordinary runtime permission toggle. Use phone inference with polishing off for local processing, and device network controls when you need to block all network access. |

Accessibility is powerful: Android grants the service the ability to retrieve
window content, not a permission restricted by the OS to one text field. Writer's
code narrows use to editor targeting/insertion, rejects recognized sensitive fields
and does not press Send. Do not assume every app marks sensitive fields correctly.
Install only a Writer build you trust before granting this access. Enabling it
is separate from choosing a cloud provider and from granting microphone access.

Before Writer opens Android's Accessibility settings, it shows a prominent
disclosure (`AccessibilityDisclosure` in `ui/Legal.kt`) with separate **Accept**
and **Decline** buttons. The disclosure says what the service reads (the focused
field's text, cursor position and ID; focus, text and window-change events; the
foreground window and keyboard position), why it reads them, and that field text
stays on the phone and is not saved. This follows Play's AccessibilityService
policy for apps that are not accessibility tools. Only Accept continues to the setup steps and
then to Settings. Decline is saved (`writer-consent` preferences), no settings
screen opens, and recording and History keep working. The Permissions row then
reads "Declined" and offers the disclosure again when tapped. Closing the dialog
with Back saves nothing.

The service itself also waits for Accept (`AccessibilityConsent` in
`PermissionStatus.kt`). If it is switched on directly in Android Settings,
without Accept in Writer, it stays connected but ignores accessibility events,
shows no floating control and inserts nothing. A result that arrives in that
state is kept for History. Home shows "Text insertion is paused", and
Permissions shows "paused until you accept". Accepting resumes the service at
once, with no second trip to Android Settings. Existing installs that already
have the service on need this one Accept after updating.

Each claim in the disclosure is checked against `WriterInputService`,
`DeliveryStore` and `editor_accessibility.xml`, so change the disclosure
whenever the service's data access changes. The emulator tests in
`ComplianceUiTest` cover Accept, Decline, dismissal and the paused state;
`FloatingInputTest` proves that a paused service inserts nothing.

The privacy policy and terms (`LegalLinks` in `ui/Legal.kt`) are linked from the
welcome dialog, **Set up Writer** and **Storage & about → Legal**. That page also
opens **Open-source licenses**, which works offline. Its library list is generated
at build time from the release runtime entries in `gradle.lockfile`, and a
library whose license is not yet classified fails the build. Full license texts
are packaged under `assets/licenses/`, together with whisper.cpp's own license.

If a sideloaded build is blocked by **Restricted settings**, confirm the app's
source before using Android's Allow restricted settings option. This is not a
required bypass for an unknown APK. The optional Accessibility shortcut can stay
off. Switching off Accessibility or overlay access does not uninstall Writer.
Google documents [changing app permissions](https://support.google.com/android/answer/9431959).

## macOS desktop

Microphone permission is for capture. **Accessibility** enables the fn event hook,
automatic paste and optional selected-text context. The event hook observes key
and modifier events to implement the configured shortcuts. Context-enabled modes
can read selected text/application information; enabled clipboard context reads
the general pasteboard. See [when context is sent and saved](privacy.md#optional-text-and-context-can-leave-before-stop).

Use **System Settings → Privacy & Security → Microphone / Accessibility** and
turn off Writer. Without Accessibility, use the recording button and manually
review/copy saved text; shortcut/paste availability depends on the configured
shortcut and app. Microphone access remains separately controlled. Writer's
current permission flow does not require a separate blanket Input Monitoring
permission. Apple explains [Accessibility access and revocation](https://support.apple.com/guide/mac-help/allow-accessibility-apps-to-access-your-mac-mh43185/mac).

## Windows desktop

Writer uses native microphone capture, global shortcut/input hooks, UI Automation
for eligible selected-text context, the foreground window title and clipboard
contents for modes that enable application or clipboard context, and
clipboard/paste delivery. Windows does
not offer the same per-app Accessibility toggle as macOS. Disable context in
**Settings → Modes**, change/disable the relevant mode shortcuts, or **Quit**
Writer from its tray menu to stop its hooks. Closing the window hides it to the
tray and is not Quit. Remove launch-at-login in **Settings → System** if desired.

Windows microphone privacy settings control desktop microphone access; the
available desktop-app switch can affect other desktop apps too. Recordings
already saved remain accessible after revocation. Writer does not need to run as
administrator for routine dictation. Do not elevate it to work around insertion
into another elevated application.

## Installation and updates

A self-update installs executable code with Writer's existing account access and
permissions. It is distinct from a model choice. Windows uses the current-user
installer; macOS installs a replacement app bundle; Android uses PackageInstaller
and may show a confirmation. An updater signature is not the same as Windows
publisher signing or macOS notarization. Use only the release channel covered by
the [security policy](../../SECURITY.md); debug/dev-signed builds are not public
security-approved releases.

On desktop there is no permanent “may self-update” OS grant equivalent to
Android's install-source toggle. Decline download/install when offered and quit
Writer to stop its checks; ordinary app permissions can still be revoked in the
OS. No instruction here authorizes disabling OS signature or malware protections.

## Browser and private self-hosted use

Grant microphone access only to the Writer origin you intend to use. Revoke it
from the browser's site-permission controls. This stops future capture and leaves
IndexedDB/server copies unchanged. Browser microphone access requires localhost
or HTTPS; a secure connection does not establish that the server operator is
trustworthy. The operator can access uploaded audio, text and configured cloud
credentials. The standalone service is private-use infrastructure, not a public
multi-user hosting product.

---

▶️ Follow TrickTic on [YouTube](https://www.youtube.com/@TrickTicAI).
