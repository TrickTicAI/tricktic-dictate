// Run with native Windows Node. A private root and panel browser profile keep
// these deliberate failures away from the installed app, microphone and models.
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
if (process.platform !== 'win32' || !process.argv[2]) throw Error('Run on Windows: node panel-recovery-smoke.mjs <executable>');
const root = await mkdtemp(join(tmpdir(), 'writer-panel-recovery-'));
await mkdir(join(root, 'State'));
await writeFile(join(root, 'State', 'onboarding.json'), '{"completed_version":1}');
await writeFile(join(root, 'State', 'tauri-preferences.json'), JSON.stringify({
  recording: 'Ctrl+Alt+Shift+F11', cancel: 'Ctrl+Alt+Shift+F10', visibility: 'always',
  panel_colors: { background: '#a62973', wave: '#ffffff' },
}));
const app = spawn(resolve(process.argv[2]), ['--hidden', '--user-root', root], {
  env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: '--remote-debugging-port=0' }, stdio: 'ignore',
});
const sockets = [];
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, timeout = 30000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try { const value = await check(); if (value) return value; } catch {}
    await pause(200);
  }
  throw Error('Panel recovery condition timed out');
}
async function connect(profile, suffix) {
  const target = await until(async () => {
    const port = Number((await readFile(join(root, profile, 'EBWebView', 'DevToolsActivePort'), 'utf8')).split('\n')[0]);
    const tabs = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
    return tabs.find(tab => tab.type === 'page' && tab.url.endsWith(suffix));
  });
  const socket = new WebSocket(target.webSocketDebuggerUrl); sockets.push(socket);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let serial = 0;
  const waiting = new Map();
  socket.onmessage = event => {
    const data = JSON.parse(event.data); const pending = waiting.get(data.id);
    if (pending) { waiting.delete(data.id); clearTimeout(pending.timer); data.error ? pending.reject(Error(data.error.message)) : pending.resolve(data.result); }
  };
  function call(method, params = {}) {
    return new Promise((resolve, reject) => {
      const id = ++serial;
      const timer = setTimeout(() => { waiting.delete(id); reject(Error(`${method} timed out`)); }, 3000);
      waiting.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }));
    });
  }
  async function evaluate(expression) {
    const result = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw Error(result.exceptionDetails.text);
    return result.result.value;
  }
  return { call, evaluate };
}
const receipt = {};
try {
  console.log('Connecting to the isolated owner and panel');
  const main = await connect('TauriWebView2', '');
  let panel = await connect('PanelWebView2', 'desktop-overlay.html');
  await main.evaluate('window.panelProbeOwner = "unchanged"');
  const healthy = () => panel.evaluate('new Promise(resolve => requestAnimationFrame(() => resolve(document.body.dataset.visibility === "always")))');
  // Reload can escalate to a new native panel/browser. The old debugging
  // socket then belongs to the destroyed page, even when recovery succeeded.
  async function restored(marker) {
    try {
      const ready = await panel.evaluate(`window[${JSON.stringify(marker)}] !== true && document.body.dataset.visibility === "always"`);
      return ready && await healthy();
    } catch {
      panel = await connect('PanelWebView2', 'desktop-overlay.html');
      return false;
    }
  }
  await until(healthy);
  console.log('Checking healthy frame replies');
  await pause(10000);
  // A healthy idle panel must not enter a reload loop.
  receipt.healthy = await panel.evaluate('document.body.dataset.visibility === "always"');
  receipt.noSpuriousReload = await readFile(join(root, 'State', 'panel-recovery.json'), 'utf8').then(text => !/reload|destroy/.test(text), () => true);
  // The renderer remains responsive and produces frames, but its visible fill
  // is gone. A frame-only heartbeat incorrectly accepts this indefinitely.
  console.log('Blanking the panel while keeping its frames alive');
  await panel.evaluate('window.panelProbeBlank = true; document.querySelector("section").style.opacity = "0"');
  receipt.blankPanelStillProducedFrames = await panel.evaluate('new Promise(resolve => requestAnimationFrame(() => resolve(window.panelProbeBlank === true)))');
  await until(() => restored('panelProbeBlank'), 45000);
  receipt.blankSurfaceRecovered = true;
  receipt.ownerSurvivedBlankSurface = await main.evaluate('window.panelProbeOwner === "unchanged"');
  console.log('Stopping panel animation frames');
  await panel.evaluate('window.panelProbeStalled = true; window.requestAnimationFrame = () => 0');
  await until(() => restored('panelProbeStalled'), 45000);
  receipt.stalledFramesRecovered = true;
  receipt.ownerSurvivedReload = await main.evaluate('window.panelProbeOwner === "unchanged"');
  // An actual renderer crash, restricted to the panel's private browser profile.
  console.log('Crashing the isolated panel renderer');
  void panel.call('Page.crash').catch(() => {});
  await pause(1000);
  await until(async () => {
    panel = await connect('PanelWebView2', 'desktop-overlay.html');
    return healthy();
  }, 45000);
  receipt.rendererCrashRecovered = true;
  receipt.ownerSurvivedCrash = await main.evaluate('window.panelProbeOwner === "unchanged"');
  // A dead browser cannot recover by reloading its page; it needs a new window.
  console.log('Closing the isolated panel browser');
  void panel.call('Browser.close').catch(() => {});
  await pause(2000);
  panel = await connect('PanelWebView2', 'desktop-overlay.html');
  await until(healthy);
  receipt.browserRecreated = true;
  receipt.ownerSurvivedRecreation = await main.evaluate('window.panelProbeOwner === "unchanged"');
  receipt.captureStayedIdle = await main.evaluate('window.__TAURI__.core.invoke("capture_status").then(s => !s.active)');
  if (Object.values(receipt).some(value => value !== true)) throw Error('Panel recovery assertion failed');
  await writeFile(join(root, 'panel-probe.json'), JSON.stringify(receipt, null, 2));
  console.log(JSON.stringify({ root, ...receipt }));
} catch (error) {
  console.error(JSON.stringify({ root, ...receipt, error: String(error) })); process.exitCode = 1;
} finally {
  for (const socket of sockets) socket.close();
  app.kill();
}
