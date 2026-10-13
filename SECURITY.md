# Security policy

Report suspected Writer vulnerabilities privately to
[report@tricktic.com](mailto:report@tricktic.com) or through
[GitHub private vulnerability reporting](https://github.com/TrickTicAI/tricktic-dictate/security/advisories/new).
The TrickTic maintainers own triage and coordinate fixes, release decisions and
reporter follow-up. Do not post exploit details, keys, private audio or
transcripts in public issues.

## Reporting

Include the affected platform, Writer version/build, installation source, a short
description of the impact and minimal reproduction steps. Prefer synthetic audio
and invented text. Remove credentials, authorization headers, personal paths and
other people's data from attachments. Start with a description if reproducing
would require sharing sensitive material; agree on a private transfer method first.
Email is not a promise of end-to-end encryption.

The maintainer reviews the report, acknowledges receipt, confirms reproduction
and severity, and agrees on follow-up/disclosure with the reporter. There is no
24/7 response service or guaranteed acknowledgement/fix deadline. If no reply
arrives, follow up on the same email thread; do not publish sensitive evidence to
obtain attention. There is no promised bounty or legal safe-harbor program.

Investigate only installations/accounts you own or are explicitly authorized to
test. Do not probe live release infrastructure, access other users' data, incur
provider charges on another account or disrupt a running service.

## Supported versions and launch scope

| Product / version | Security support |
| --- | --- |
| Current Tauri Windows/macOS and native Android release lines | Public-launch targets. After platform approval, fixes target the latest stable artifact for each platform; older versions must upgrade. No LTS or older-version backport commitment. |
| Existing private previews, debug or development-signed builds | Reports accepted; corrections target current source/next candidate on a best-effort basis. These are not security-approved public releases. |
| Self-hosted browser/Rust API | Private operator-managed use of current source only. Reports accepted; no public multi-user hosting or managed-service support promise. |
| Retired Windows C#/WebView2 shell | Unsupported; retained for controlled migration/recovery tests. Excluded from the public launch. Export old data rather than continuing daily use. |

As of **2026-09-20**, the release audit has open remediation/acceptance gates;
this policy does not approve any public artifact. A channel named “stable”, a
version number or an updater signature alone is not security approval. Approved
release notes must identify the exact version/build, artifact and applicable
platform acceptance. Windows, macOS and Android can have different versions.

Public distribution must include this policy and the linked privacy/permissions
documents in a location readable by users. Public GitHub issues are not a
reporting route for vulnerabilities. Keep the mailbox and private vulnerability
reporting monitored throughout the support period.
For the public Windows and Play releases this is
`https://legal.tricktic.com/tricktic-dictate/security`, with the
privacy policy (including permissions) at `.../privacy` and terms at `.../terms`
(MW-RELEASE-6).

## Patches, incidents and disclosure

Triage prioritizes demonstrated credential/data exposure, unauthorized execution,
update compromise and loss of recoverable recordings. The maintainer records the
reproduction, affected versions, severity, owner, mitigation and next update in a
private issue or restricted incident record. Reporters are told when the assessment
changes; incomplete or unconfirmed reports are not described as proven exploits.

Fixes go through a reviewed task branch, scoped regression checks and the exact
platform release gate. Release notes identify affected/fixed versions, practical
mitigations, data effects and any required user action. Preserve saved audio and
independent raw transcripts through fixes and upgrades. If no safe fix is ready,
disclose an actionable workaround or withdraw the affected release rather than
asserting an unsupported guarantee. Older stable versions are serviced by upgrading
to the corrected stable artifact, not by a separate backport line.

The TrickTic maintainers own incident coordination and decisions to pause
distribution, isolate an operator-managed service, revoke exposed provider
credentials, recover signing keys or publish a replacement artifact. These actions need the appropriate owner
access; documenting them does not perform them. Publish a sanitized advisory once
users have a mitigation, coordinating timing and credit with the reporter. Keep
private evidence access-limited and retain only what is needed for resolution.

## Further information

- [Privacy, processing destinations and deletion limits](docs/security/privacy.md)
- [Permissions and revocation](docs/security/permissions.md)
- [Threat model and trust boundaries](docs/security/threat-model.md)
