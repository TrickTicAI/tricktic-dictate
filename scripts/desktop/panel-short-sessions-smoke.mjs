// Run with native Windows Node. A private root and panel browser profile keep
// these deliberate failures away from the installed app, microphone and models.
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
if (process.platform !== 'win32' || !process.argv[2]) throw Error('Run on Windows: node panel-short-sessions-smoke.mjs <executable> [--expect-legacy]');
const legacy = process.argv.includes("--expect-legacy");
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
  const main = await connect('TauriWebView2', '');
  let panel = await connect('PanelWebView2', 'desktop-overlay.html');
  // Owner initialization emits an idle snapshot; wait for it before injecting
  // presentation-only states, or it can overwrite the first test appearance.
  await until(() => main.evaluate('typeof window.writerDesktopSnapshot === "function"'));
  await until(() => panel.evaluate('new Promise(resolve => requestAnimationFrame(() => resolve(document.body.dataset.visibility === "always")))'));
  await pause(500);
  const preferences = await main.evaluate('window.__TAURI__.core.invoke("preferences_get")');
  await main.evaluate(`window.__TAURI__.core.invoke("preferences_save", {preferences:${JSON.stringify({...preferences, visibility:'recording'})}})`);
  const epoch = randomUUID(); let session = randomUUID(), revision = 0;
  async function state(state) {
    // No capture_start, keyboard shortcut, model request or insertion.
    return main.evaluate(`window.__TAURI__.core.invoke("session_event", {message:${JSON.stringify({version:1,kind:'snapshot',epoch,session_id:session,revision:revision++,state,audio_saved:false})}})`);
  }
  async function burst() {
    session = randomUUID();
    await state('requesting'); await pause(2300);
    await state('idle'); await pause(2300);
  }
  await main.evaluate('window.shortSessionOwner = "alive"');
  await panel.evaluate('window.shortSessionHealthy = true');
  console.log('Checking six healthy short dictations');
  for (let cycle = 0; cycle < 6; cycle++) await burst();
  receipt.healthyShortSessionsPreservedDocument = await panel.evaluate('window.shortSessionHealthy === true');
  const diagnostics = () => readFile(join(root,'State','panel-diagnostics.json'),'utf8').then(JSON.parse, () => []);
  receipt.healthyShortSessionsDidNotReload = !(await diagnostics()).some(r => /reload|destroy/.test(r.reason));
  for (const fault of legacy ? ['stalled'] : ['stalled', 'blank']) {
    session = randomUUID();
    await state('requesting');
    await until(() => panel.evaluate('new Promise(resolve => requestAnimationFrame(() => resolve(document.body.dataset.state === "requesting")))'));
    console.log(`Breaking panel: ${fault}, then six short dictations`);
    await panel.evaluate(`window.shortSessionBroken = true; document.querySelector("section").style.opacity = "0"; ${fault === 'stalled' ? 'window.requestAnimationFrame = () => 0;' : ''}`);
    await state('idle'); await pause(2300);
    for (let cycle = 0; cycle < 6; cycle++) await burst();
    let broken;
    try { broken = await panel.evaluate('window.shortSessionBroken === true'); }
    catch { panel = await connect('PanelWebView2', 'desktop-overlay.html'); broken = await panel.evaluate('window.shortSessionBroken === true'); }
    if (legacy) {
      receipt.legacyStayedBrokenAcrossSixShortSessions = broken;
      receipt.legacyDidNotRepair = !(await diagnostics()).some(r => /reload|destroy/.test(r.reason));
    } else {
      receipt[`${fault}RecoveredAcrossShortSessions`] = !broken;
      await state('requesting');
      await until(() => panel.evaluate('new Promise(resolve => requestAnimationFrame(() => resolve(document.body.dataset.state === "requesting" && getComputedStyle(document.querySelector("section")).opacity === "1")))'));
      await state('idle'); await pause(2300);
    }
  }
  if (!legacy) {
    const events = await diagnostics();
    receipt.watchdogStartupRecorded = events.some(r => r.reason === 'watchdog_started');
    receipt.repairsRecorded = events.some(r => /reload|destroy/.test(r.reason));
  }
  receipt.ownerSurvived = await main.evaluate('window.shortSessionOwner === "alive"');
  receipt.captureStayedIdle = await main.evaluate('window.__TAURI__.core.invoke("capture_status").then(s => !s.active)');
  await writeFile(join(root,'short-session-probe.json'), JSON.stringify(receipt,null,2));
  if (Object.values(receipt).some(value => value !== true)) throw Error('Short-session recovery assertion failed');
  console.log(JSON.stringify({root,...receipt}));
} catch (error) {
  console.error(JSON.stringify({root,...receipt,error:String(error)})); process.exitCode = 1;
} finally {
  sockets.forEach(socket => socket.close());
  app.kill();
}
