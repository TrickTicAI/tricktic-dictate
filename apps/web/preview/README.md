# Desktop preview over made-up data

Runs the real desktop UI (`apps/web`) in an ordinary browser, as if it were inside
the Tauri shell, but over a fake host with made-up data. Use it to screenshot
the app, to review a UI branch from a phone or laptop, or to click through a
redesign. It is not part of any build and never reaches a real backend,
microphone, keychain, account or recording.

| File | What it does |
| --- | --- |
| `fake-host.js` | Installs `window.__TAURI__` (`core.invoke`, `event.listen`) and replaces `fetch` for `/api/v1/*` and `/native/audio/*`. Unknown commands and routes are listed in `window.__fakeUnknown` and logged. |
| `fake-data.mjs` | About 30 English, Hebrew and mixed dictations over two weeks (one failed, one interrupted, one never transcribed), Super results, a GPU local model, cloud key states, dictionary rules, modes and desktop preferences. IDs are UUIDs because the session protocol rejects anything else. |
| `vite.config.mjs` | Serves `apps/web` with the fake host injected before the app loads. It also relaxes `isTauri()`'s `tauri.localhost` origin check, which a browser can never pass. |
| `shoot.mjs` | Screenshots every page, every Settings section, Paper and Dracula, a 1024 px and a narrow window, a right-click menu and the first-run guide. |
| `tray-demo.mjs` | The rows the shell sends the tray menu, in `tray.rs`'s shape, for each state. The preview serves `/desktop-tray.html` with them; the tray page's browser tests import them too. |
| `shoot-panel-tray.mjs` | Screenshots the floating panel in every state and the tray menu (ready, recording, failed, three submenus), in Paper and Dracula. |

## Run it

Node 24 and the workspace dependencies (`pnpm install`) are needed.

```bash
cd apps/web
node node_modules/vite/bin/vite.js --config preview/vite.config.mjs --host 127.0.0.1 --port 15301 --strictPort
# then, in another shell
node preview/shoot.mjs http://127.0.0.1:15301/ /tmp/shots
node preview/shoot-panel-tray.mjs http://127.0.0.1:15301/ /tmp/shots
```

To hand a branch to a reviewer on another device, serve it through the dev
gateway; the base path comes from `DEVGW_BASE`:

```bash
devgw run dictate-ui-live --cwd "$PWD" -- \
  node node_modules/vite/bin/vite.js \
  --config preview/vite.config.mjs --host 127.0.0.1 --port {port} --strictPort
```

Useful URLs: `?tab=statistics`, `?tab=dictionary&sub=snippets`,
`?settings=engine`, and the tray menu at
`desktop-tray.html?state=ready|recording|working|failed|starting&theme=paper|dracula&platform=windows|macos`
(a strip under it switches between them). `?history=600` makes the history that
long (up to 2000), for measuring long lists. Set `window.__FAKE_ONBOARDING = true` before load
(Playwright `addInitScript`) to see the first-run guide.

## Limits

- Recording is simulated: `capture_start` returns synthetic levels and `capture_stop`
  adds a recording, but nothing is transcribed, so the guide stops at **Try it**.
- The panel and tray pages are plain HTML. `/desktop-overlay.html` needs its
  events emitted from a script; `/desktop-tray.html` is served with demo rows,
  and clicking a row only logs the id the shell would receive.
- When the UI starts calling a new command or route, add it to `fake-host.js`;
  `window.__fakeUnknown` tells you which ones are missing.
