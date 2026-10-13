# Desktop shell: Tauri v2

The maintainer selected a parity migration from the C# WinForms/.NET Framework 4.8 shell
to Tauri v2 on 2026-09-07. Keep React and the Rust/Axum implementation. Visual
redesign is a separate task. The previous recommendation to retain WinForms is
superseded by this decision, not treated as evidence about the alternatives.

## Independently checked rationale

- **.NET Framework 4.8 remains supported.** Microsoft ties servicing to the host
  Windows lifecycle. Calling it unsupported, irrelevant, or unusable would be
  inaccurate. Its Windows scope and the extra C# integration layer are the
  project-specific reasons to replace it.
  [Microsoft support policy](https://dotnet.microsoft.com/en-us/platform/support/policy/dotnet-framework),
  [Windows Forms overview](https://learn.microsoft.com/en-us/dotnet/desktop/winforms/overview/).
- **Tauri matches the existing Rust and React code.** It uses WebView2 on Windows
  and WKWebView on macOS. Replacing the host does not inherently improve the
  appearance of the unchanged React interface. It also means web engine behavior
  must be tested on each OS.
  [Tauri webviews](https://v2.tauri.app/reference/webview-versions/),
  [process model](https://v2.tauri.app/concept/process-model/).
- **WKWebView supports microphone capture.** Apple's media-capture permission
  delegate and WebKit's MediaRecorder documentation contradict a categorical
  claim that it cannot record. They do not establish reliability for Writer's
  custom origin, permissions, device changes and background lifetime. We choose
  native CPAL capture to own those concerns and persistence outside the WebView;
  no local macOS reliability measurement has been made.
  [Apple media permission delegate](https://developer.apple.com/documentation/webkit/wkuidelegate/webview(_:requestmediacapturepermissionfor:initiatedbyframe:type:decisionhandler:)),
  [WebKit MediaRecorder](https://webkit.org/blog/11353/mediarecorder-api/),
  [CPAL](https://docs.rs/cpal/0.16.0/cpal/).
- **Mobile support is a foundation, not desktop feature parity.** Tauri's global
  shortcut plugin is compiled for desktop; overlays, background recording,
  permission handling, audio routing and Android IME delivery need their own
  implementations and acceptance. This change does not ship Android dictation.
  [Global shortcut plugin](https://v2.tauri.app/plugin/global-shortcut/),
  [mobile plugin development](https://v2.tauri.app/develop/plugins/develop-mobile/).
- **Electron supports Windows, macOS and Linux.** Its main process uses Node.js.
  Keeping a Rust service as a child process is a common integration choice, not
  an intrinsic quality downgrade; native modules are also possible. Electron is
  excluded by the maintainer's decision and is a weaker fit for keeping Rust as the
  desktop host while leaving a mobile path open.
  [Electron introduction](https://www.electronjs.org/docs/latest/),
  [native modules](https://www.electronjs.org/docs/latest/tutorial/using-native-node-modules).

Claims about competing applications' internal stacks or what is the most popular
2026 framework are not required to justify this architecture and have been
removed. No performance, binary-size or reliability advantage is claimed without
measurements of comparable Writer artifacts.

## Implementation boundaries

`apps/desktop/tauri` hosts the existing Axum router in-process, through the local
WebView resource handler. It exposes no desktop TCP listener and passes no API
capability into JavaScript. The standalone API binary and browser client retain
their deployment/capture paths. Rust owns microphone capture, saved PCM/WAV,
credential entry/storage, tray, window lifetime and shortcut registrations.

The Windows worker's Vulkan runtime remains a separate owned native process.
Moving the shell does not port GPU inference to Metal or bundle Windows DLLs for
macOS. A private Windows job now belongs to the Rust supervisor so a desktop
process crash still terminates its owned worker tree.

See the Tauri migration and validation notes for the actual
build commands, compatibility boundaries and measured evidence.

---

▶️ Follow TrickTic on [YouTube](https://www.youtube.com/@TrickTicAI).
