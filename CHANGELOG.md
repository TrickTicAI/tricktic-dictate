# TrickTic Dictate changelog

▶️ Release walkthroughs and demos: [TrickTic on YouTube](https://www.youtube.com/@TrickTicAI).

## 0.31.0 desktop / 0.15.0 Android dev — 2026-10-03

- Reject detected critical cleanup changes while retaining spoken text and audio.
- Align provider requests and stream saved cloud audio; inline small Gemini requests.
- Page large histories, preserve transcript version ancestry and record content-free stage timings.
- Match Android snippets to desktop, batch polishing requests and bound long-audio preparation memory.
- Add repeatable English, Hebrew and mixed-speech quality benchmarks.

## 0.29.0 — 2026-10-01

A redesign of the Windows and macOS app. All 15 themes keep working.

- One 48 px title bar: Home, Statistics and Dictionary, a search for
  everything (Ctrl/⌘ K), the speech engine's state and Settings.
- Home is a feed of what you said beside a reader. Copy is the main button,
  versions are named Polished and As spoken, the player draws the real
  waveform, and a dock is the one place to dictate. Receipts say where the
  words went ("Pasted into Slack"), and Discard gives five seconds to Undo.
- Statistics opens on the last 7 days with tiles, a real chart, when you
  dictate, speech-model times and polishing suggestions.
- Dictionary: words as chips with an "N of 128 active" meter, corrections and
  snippets written as sentences, one search and Undo.
- Settings is regrouped by what you are doing and shows each section's state,
  with new Microphone & sound and Floating panel pages, provider logos on API
  keys and one Animations setting for the window, panel and tray.
- The floating panel: every state looks different, no red light while
  listening, "✓ Pasted" for a moment once the text is in, Retry on a failure,
  and a corner counter for dictations still finishing.
- The tray menu opens on whether you can dictate, your last dictation with
  Copy it again, the current microphone, language, speech model and mode, and
  Preferences switches. The tray icon is drawn for your display and shows
  recording, transcribing and a failed dictation.
- Settings → Privacy & data shows what takes space on this computer
  (recordings, transcripts and settings, downloaded models) and frees it:
  audio only, or audio and transcripts, from before today or from every
  dictation, always reviewed first. Models stay installed.
- The window reopens where you left it, in your theme's colour. On a Mac it
  keeps its own traffic lights and gains Settings… (⌘,).
- Home stays responsive on a long history: opening 600 dictations settles in
  about 4 s instead of 24 s.

Requires Windows 10/11 x64 and an installed Microsoft Edge WebView2 Runtime
(Windows) or macOS 15+ on Apple Silicon with the development signing
identity (Mac). Parakeet is unavailable on Windows. Google sign-in and sync
are disabled in this release. The Windows installer has no Authenticode
publisher certificate, by release decision; Windows may show
**Unknown publisher**. In-app updates verify minisign signatures and signed
release metadata.

## 0.26.0 — 2026-09-29

- Faster dictation delivery on large histories: saving no longer rescans
  the whole data directory on every write, cutting seconds off the time
  from releasing the shortcut to pasted text on Windows.
- Settings manages provider keys in one place: Transcription, Text
  processing and General link to Settings → API keys, which gains a back
  button to the screen that sent you.
- Settings rows keep their titles readable beside long values such as
  model names.
- The floating panel reports when Listening actually paints, so slow
  microphone starts can be diagnosed from the delivery receipt. No
  dictation behavior changed.

Requires Windows 10/11 x64 and an installed Microsoft Edge WebView2 Runtime
(Windows) or macOS 15+ on Apple Silicon with the development signing
identity (Mac). Parakeet is unavailable on Windows. Google sign-in and sync
are disabled in this release. The Windows installer has no Authenticode
publisher certificate, by release decision; Windows may show
**Unknown publisher**. In-app updates verify minisign signatures and signed
release metadata.

## 0.25.0 — 2026-09-28

- New Settings → API keys section on Windows and macOS: every cloud
  provider with its key state and what uses it. Add, replace, check and
  remove keys through the existing native dialog; keys never enter the
  WebView, and the key status names the macOS Keychain on a Mac.
- Transcript cleanup gains a Provider key row that opens the API keys
  section when the chosen cleanup provider has no key, so
  key-needed models are one step away from working.
- Settings search routes key words (key, token, api, openrouter) to the
  API keys section instead of a sentence in Data & Privacy.

Requires Windows 10/11 x64 and an installed Microsoft Edge WebView2 Runtime
(Windows) or macOS 15+ on Apple Silicon with the development signing
identity (Mac). Parakeet is unavailable on Windows. Google sign-in and sync
are disabled in this release. The Windows installer has no Authenticode
publisher certificate, by release decision; Windows may show
**Unknown publisher**. In-app updates verify minisign signatures and signed
release metadata.

## 0.24.0 — 2026-09-27

First public Windows release, available at
[tricktic.com/dictate](https://tricktic.com/dictate). Publication status and exact
artifact evidence are in the release record.

- Dictate in Hebrew and English with local Whisper models or your own cloud
  provider key. Keep saved audio available for retry and raw transcripts beside
  optional cleaned text.
- Safer Windows updates: stage files before replacement, remove obsolete runtime
  files, and preserve saved recordings and settings when upgrading.
- Updated local speech and text runtimes, tighter worker request limits, and
  better recovery when a recording's storage path is too long.
- Bundled license notices and privacy, terms and security links.

Requires Windows 10/11 x64 and an installed Microsoft Edge WebView2 Runtime.
Parakeet is unavailable on Windows. Google sign-in and sync are disabled in this
release. The installer has no Authenticode publisher certificate, by
release decision; Windows may show **Unknown publisher**. In-app updates verify
minisign signatures and signed release metadata.
