// Run with native Windows Node. A private root and panel browser profile keep
// these deliberate failures away from the installed app, microphone and models.
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
if (process.platform !== 'win32' || !process.argv[2]) throw Error('Run on Windows: node panel-recovery-smoke.mjs <executable>');
const legacy = process.argv.includes("--expect-legacy");
const nativeProbe = join(dirname(fileURLToPath(import.meta.url)), "panel-window-probe.ps1");
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
let backdrop;
function native(action = 'Inspect') {
  return new Promise((resolve, reject) => {
    const child = spawn('powershell.exe', ['-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',nativeProbe,'-ProcessId',String(app.pid),'-UserRoot',root,'-Action',action]);
    let output = '', errors = '';
    child.stdout.on('data', chunk => { output += chunk; }); child.stderr.on('data', chunk => { errors += chunk; });
    child.on('error', reject); child.on('exit', code => code === 0 ? resolve(JSON.parse(output.trim())) : reject(Error(errors)));
  });
}

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
  console.log('Checking near-black content over a black native backdrop');
  console.log('Reading private panel preferences');
  const originalPreferences = await main.evaluate('window.__TAURI__.core.invoke("preferences_get")');
  console.log('Applying the near-black panel size and colour');
  await main.evaluate(`window.__TAURI__.core.invoke("preferences_save", {preferences: ${JSON.stringify({...originalPreferences, scale: 0.8, panel_colors:{background:'#0a0a0b',wave:'#bd93f9'}})}})`);
  backdrop = spawn('powershell.exe', ['-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',nativeProbe,'-ProcessId',String(app.pid),'-UserRoot',root,'-Action','Backdrop'], {stdio:['ignore','pipe','pipe']});
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(Error('Backdrop did not appear')), 10000);
    backdrop.stdout.on('data', chunk => { if (String(chunk).includes('READY')) { clearTimeout(timer); resolve(); } });
    backdrop.stderr.on('data', chunk => { clearTimeout(timer); reject(Error(String(chunk))); });
    backdrop.on('error', reject);
  });
  await panel.evaluate('window.panelProbeBlackBlank = true; document.querySelector("section").style.opacity = "0"');
  await pause(1500);
  const black = await native();
  receipt.blackBackdropMatchesOldFillCheck = black.fillLooksBlack;
  if (!receipt.blackBackdropMatchesOldFillCheck) throw Error('Black regression did not reproduce the ambiguous fill');
  if (legacy) {
    await pause(22000);
    receipt.legacyAcceptedInvisibleBlackPanel = await panel.evaluate('window.panelProbeBlackBlank === true');
    receipt.legacyOwnerSurvived = await main.evaluate('window.panelProbeOwner === "unchanged"');
    if (!receipt.legacyAcceptedInvisibleBlackPanel) throw Error('Legacy build unexpectedly recovered');
    await writeFile(join(root, 'panel-probe.json'), JSON.stringify(receipt, null, 2));
    // The explicit baseline mode ends after proving the installed defect.
  } else {
    await until(() => restored('panelProbeBlackBlank'), 45000);
    receipt.blackOnBlackContentRecovered = true;
    receipt.nativePillShape = (await native()).cornerClipped && (await native()).centreIncluded;
    await main.evaluate('window.__TAURI__.core.invoke("session_waiting", {count:1})');
    await until(async () => (await native()).badgeIncluded);
    receipt.nativeBadgeShape = true;
    await main.evaluate('window.__TAURI__.core.invoke("session_waiting", {count:0})');
    // Repeated real native invalidations and size changes, with a settled
    // challenge in each size, exercise presentation rather than just the DOM.
    let nonactivating = !(await native()).foregroundBelongsToApp;
    for (const scale of [0.75, 1, 1.5, 2, 0.8]) {
      await main.evaluate(`window.__TAURI__.core.invoke("preferences_save", {preferences: ${JSON.stringify({...originalPreferences, scale, panel_colors:{background:'#0a0a0b',wave:'#bd93f9'}})}})`);
      await native('Redraw');
      await pause(2200);
      const inspection = await native();
      nonactivating &&= !inspection.foregroundBelongsToApp;
      if (!inspection.cornerClipped) throw Error('Native pill shape was lost on resize');
    }
    receipt.scaleAndRedrawPreservedShape = true;
    receipt.panelStayedNonactivating = nonactivating;
    const actions = JSON.parse(await readFile(join(root,'State','panel-diagnostics.json'),'utf8'));
    receipt.failureAndRecoveryRetained = actions.some(x=>x.reason === 'presentation_challenge_missing') && actions.some(x=>x.reason === 'presentation_challenge_recovered');
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
    console.log('Cycling 100 native appearances without starting capture');
    const cyclePreferences = {...originalPreferences, visibility:'recording', scale:0.8, panel_colors:{background:'#0a0a0b',wave:'#bd93f9'}};
    await main.evaluate(`window.__TAURI__.core.invoke("preferences_save", {preferences:${JSON.stringify(cyclePreferences)}})`);
    const epoch = randomUUID(), session = randomUUID();
    async function stateAt(state, revision) {
      // Presentation snapshots only: no capture_start, shortcuts or insertion.
      return main.evaluate(`window.__TAURI__.core.invoke("session_event", {message:${JSON.stringify({version:1,kind:'snapshot',epoch,session_id:session,revision,state,audio_saved:false})}})`);
    }
    for (let cycle = 0; cycle < 100; cycle++) {
      await stateAt('requesting', cycle * 2);
      await stateAt('idle', cycle * 2 + 1);
    }
    await stateAt('requesting', 200);
    await until(async () => (await native()).visible);
    await pause(10000); // A settled screen challenge must not reload a healthy panel.
    receipt.hundredAppearancesPreservedWindow = await panel.evaluate('document.body.dataset.state === "requesting"');
    await stateAt('idle', 201);
    await until(async () => !(await native()).visible);
    receipt.hiddenBetweenDictations = true;
    receipt.ownerSurvivedAppearances = await main.evaluate('window.panelProbeOwner === "unchanged"');
    receipt.captureStayedIdle = await main.evaluate('window.__TAURI__.core.invoke("capture_status").then(s => !s.active)');
  }
  if (Object.values(receipt).some(value => value !== true)) throw Error('Panel recovery assertion failed');
  await writeFile(join(root, 'panel-probe.json'), JSON.stringify(receipt, null, 2));
  console.log(JSON.stringify({ root, ...receipt }));
} catch (error) {
  console.error(JSON.stringify({ root, ...receipt, error: String(error) })); process.exitCode = 1;
} finally {
  for (const socket of sockets) socket.close();
  backdrop?.kill();
  app.kill();
}
