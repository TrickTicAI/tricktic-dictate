// MW-SEC-21: drive an INSTALLED Writer's own update commands in a disposable
// Windows account. Run with native Windows Node as that account:
//   node update-driver.mjs --exe <installed exe> --out <dir> --steps check,download,install [--expect 0.22.0]
// Steps: status | check | download | cancel (during a download) | install | interrupt (kill the
// installer shortly after it starts) | capture (microphone capture that consent should refuse). A temporary WebView2 debugging port is
// test-only instrumentation; it is never enabled by the product itself.
import { spawn, execFileSync } from 'node:child_process';
import { existsSync, rmSync } from 'node:fs';
import { readFile, writeFile, stat } from 'node:fs/promises';
import { join } from 'node:path';

const args = Object.fromEntries(process.argv.slice(2).reduce((pairs, arg, index, all) =>
  arg.startsWith('--') ? [...pairs, [arg.slice(2), all[index + 1]]] : pairs, []));
if (process.platform !== 'win32' || !args.exe || !args.out || !args.steps) throw Error('usage: --exe <path> --out <dir> --steps a,b');
// The Metrix Writer folder is in use until the first new-identity launch moves it.
const root = [join(process.env.LOCALAPPDATA, 'com.tricktic.dictate'), join(process.env.LOCALAPPDATA, 'MetrixWriter')].find(existsSync) ?? join(process.env.LOCALAPPDATA, 'com.tricktic.dictate');
const portFile = join(root, 'TauriWebView2', 'EBWebView', 'DevToolsActivePort');
const log = [];
const runId = Date.now();
let screenshotIndex = 0;
async function screenshot(page, label) {
  const reply = await page.send('Page.captureScreenshot', { format: 'png' });
  if (!reply.result?.data) throw Error(`Screenshot failed: ${label}`);
  const file = `update-${runId}-${++screenshotIndex}-${label}.png`;
  await writeFile(join(args.out, file), Buffer.from(reply.result.data, 'base64'));
  note('screenshot', file);
}
const note = (event, detail) => { const entry = { at: new Date().toISOString(), event, ...(detail === undefined ? {} : { detail }) }; log.push(entry); console.log(JSON.stringify(entry)); };
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, timeout, what) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { try { const value = await check(); if (value) return value; } catch {} await pause(250); }
  throw Error(`timed out waiting for ${what}`);
}
const ps = script => execFileSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script], { encoding: 'utf8' }).trim();
const registered = () => ps(`Get-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*' | Where-Object { $_.DisplayName -match 'Writer|Dictate' } | ForEach-Object { $_.DisplayName + '|' + $_.DisplayVersion + '|' + $_.InstallLocation }`);
const processes = () => ps(`Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and ($_.ExecutablePath -match 'metrix-writer|setup|Dictate|Writer') } | ForEach-Object { '' + $_.ProcessId + '|' + $_.ParentProcessId + '|' + $_.ExecutablePath }`);
const version = path => existsSync(path) ? ps(`(Get-Item -LiteralPath '${path.replaceAll("'", "''")}').VersionInfo.ProductVersion`) : null;

class Page {
  constructor(socket) { this.socket = socket; this.id = 0; this.pending = new Map();
    socket.addEventListener('message', event => { const m = JSON.parse(event.data); const p = this.pending.get(m.id); if (p) { this.pending.delete(m.id); p(m); } }); }
  send(method, params = {}) { const id = ++this.id; this.socket.send(JSON.stringify({ id, method, params })); return new Promise(resolve => this.pending.set(id, resolve)); }
  async invoke(command, timeout = 20 * 60_000, payload = {}) {
    const reply = await Promise.race([
      this.send('Runtime.evaluate', { expression: `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(payload)}).then(v => ({ ok: v }), e => ({ error: String(e) }))`, awaitPromise: true, returnByValue: true }),
      pause(timeout).then(() => ({ result: { result: { value: { error: 'driver timeout' } } } }))]);
    return reply.result?.result?.value ?? { error: reply.error?.message ?? 'evaluation failed' };
  }
}

async function launch() {
  rmSync(portFile, { force: true });
  const child = spawn(args.exe, [], { detached: true, stdio: 'ignore',
    env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: '--remote-debugging-port=0' } });
  child.unref();
  note('launched', { pid: child.pid, exe: args.exe, product_version: version(args.exe) });
  const port = await until(async () => (await readFile(portFile, 'utf8')).split(/\r?\n/)[0], 60_000, 'DevToolsActivePort');
  const target = await until(async () => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json())
    .find(t => t.type === 'page' && /index\.html|tauri\.localhost\/?$/.test(t.url)), 60_000, 'main page');
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  const page = new Page(socket);
  await until(async () => (await page.send('Runtime.evaluate', { expression: '!!window.__TAURI_INTERNALS__ && document.readyState === "complete"', returnByValue: true })).result?.result?.value, 60_000, 'Tauri bridge');
  return { page, pid: child.pid };
}

const result = { schema: 1, kind: 'mw-sec-21-update-driver', user: process.env.USERNAME, steps: args.steps, expect: args.expect ?? null };
try {
  result.before = { registered: registered(), exe_version: version(args.exe) };
  let { page, pid } = await launch();
  await screenshot(page, 'before');
  for (const step of args.steps.split(',')) {
    if (['status', 'check', 'download'].includes(step)) {
      note(step, await page.invoke(`updates_${step}`));
    } else if (step === 'cancel') {
      const download = page.invoke('updates_download');
      const during = await until(async () => { const v = await page.invoke('updates_status', 5000); return v.ok?.stage === 'downloading' && v; }, 120_000, 'download in progress');
      note('status during download', during);
      note('cancel', await page.invoke('updates_cancel'));
      note('download after cancel', await download);
      note('status after cancel', await page.invoke('updates_status'));
    } else if (step === 'install' || step === 'interrupt') {
      const install = page.invoke('updates_install', 60_000).catch(error => ({ error: String(error) }));
      if (step === 'interrupt') {
        const setup = await until(() => processes().split(/\r?\n/).find(line => /\\Temp\\|setup/i.test(line) && !line.includes(args.exe)), 120_000, 'installer process');
        await pause(Number(args['interrupt-ms'] ?? 1500));
        const setupPid = Number(setup.split('|')[0]);
        note('installer started', setup);
        try { process.kill(setupPid); note('installer killed', setupPid); } catch (error) { note('installer kill failed', String(error)); }
      }
      note('install invoked', await Promise.race([install, pause(30_000).then(() => 'no reply (app exiting)')]));
      await until(() => { try { process.kill(pid, 0); return false; } catch { return true; } }, 180_000, 'old app exit');
      note('old app exited');
      if (step === 'install') {
        await until(() => !args.expect || registered().includes(`|${args.expect}|`), 300_000, `registered version ${args.expect}`);
        await pause(5000);
      } else await pause(10_000);
      note('after install', { registered: registered(), exe_version: version(args.exe), processes: processes() });
      if (step === 'install' || args.relaunch) {
        // The updater relaunches Writer itself; stop that instance so the next
        // launch carries the debugging port instead of handing off to it.
        try { ps("Get-Process tricktic-dictate,metrix-writer-desktop -ErrorAction SilentlyContinue | ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }; exit 0"); } catch { /* nothing running */ }
        await pause(3000);
        ({ page, pid } = await launch());
        note('relaunched status', await page.invoke('updates_status'));
        note('relaunched check', await page.invoke('updates_check'));
        await screenshot(page, 'after-update');
      }
    } else if (step === 'capture') {
      // Expected to be refused (microphone consent denied). If it is not, the
      // capture is stopped and deleted so no real room audio is retained.
      const id = crypto.randomUUID();
      const started = await page.invoke('capture_start', 60_000, { id, language: 'mixed' });
      note('capture_start', started);
      await pause(3000);
      const captureStatus = await page.invoke('capture_status', 10_000);
      note('capture_status', captureStatus);
      note('capture_stop', await page.invoke('capture_stop', 60_000, { transcribe: false }));
      const captureList = await page.invoke('capture_list', 10_000);
      const captureIds = (captureList.ok ?? []).map(r => r.id ?? r.metadata?.id);
      note('capture_list_ids', captureIds);
      if (started.ok) note('capture_delete', await page.invoke('capture_delete', 30_000, { id }));
      // Device initialization can fail after Writer creates its 80-byte WAV
      // journal header. An empty interrupted journal is not captured audio.
      let emptyJournal = !captureIds.includes(id);
      if (!emptyJournal && started.error) {
        const dir = join(root, 'Data', 'native-recordings', id);
        const metadata = JSON.parse(await readFile(join(dir, 'recording.json'), 'utf8'));
        const wavBytes = (await stat(join(dir, 'audio.wav'))).size;
        const pcmBytes = await stat(join(dir, 'prepared.pcm')).then(s => s.size, e => { if (e.code === 'ENOENT') return 0; throw e; });
        emptyJournal = wavBytes === 80 && pcmBytes === 0 && metadata.byte_length === 0 && metadata.duration_ms === 0 && metadata.capture_state === 'interrupted';
        note('denied_capture_storage', { id, wav_bytes: wavBytes, pcm_bytes: pcmBytes, byte_length: metadata.byte_length, duration_ms: metadata.duration_ms, capture_state: metadata.capture_state });
      }
      if (!started.error || !/microphone|permission|denied/i.test(started.error) || captureStatus.ok?.active !== false || captureList.error || !emptyJournal) {
        throw Error('Expected microphone denial without saved audio. Any started test capture was stopped and deleted.');
      }
      await screenshot(page, 'microphone-denied');
    } else throw Error(`unknown step ${step}`);
  }
  result.passed_steps = true;
} catch (error) {
  result.error = String(error.message ?? error);
  note('error', result.error);
} finally {
  result.after = { registered: registered(), exe_version: version(args.exe), processes: processes() };
  result.log = log;
  await writeFile(join(args.out, `update-driver-${Date.now()}.json`), `${JSON.stringify(result, null, 2)}\n`);
  // The DevTools socket keeps Node alive; the app itself is left running.
  process.exit(result.error ? 1 : 0);
}
