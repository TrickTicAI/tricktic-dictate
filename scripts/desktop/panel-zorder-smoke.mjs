// Run with native Windows Node. A private root keeps the deliberately buried
// panel away from the installed app, microphone and models.
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
if (process.platform !== 'win32' || !process.argv[2]) throw Error('Run on Windows: node panel-zorder-smoke.mjs <executable> [--expect-legacy]');
const legacy = process.argv.includes('--expect-legacy');
const nativeProbe = join(dirname(fileURLToPath(import.meta.url)), 'panel-window-probe.ps1');
const root = await mkdtemp(join(tmpdir(), 'writer-panel-recovery-'));
await mkdir(join(root, 'State'));
await writeFile(join(root, 'State', 'onboarding.json'), '{"completed_version":1}');
await writeFile(join(root, 'State', 'tauri-preferences.json'), JSON.stringify({
  recording: 'Ctrl+Alt+Shift+F11', cancel: 'Ctrl+Alt+Shift+F10', visibility: 'always',
}));
const app = spawn(resolve(process.argv[2]), ['--hidden', '--user-root', root], { stdio: 'ignore' });
const covers = [];
const args = action => ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', nativeProbe, '-ProcessId', String(app.pid), '-UserRoot', root, '-Action', action];
function inspect() {
  return new Promise((resolve, reject) => {
    const child = spawn('powershell.exe', args('Inspect'));
    let output = '', errors = '';
    child.stdout.on('data', chunk => { output += chunk; }); child.stderr.on('data', chunk => { errors += chunk; });
    child.on('error', reject); child.on('exit', code => code === 0 ? resolve(JSON.parse(output.trim())) : reject(Error(errors)));
  });
}
// Keeps an ordinary window over the panel, with the panel moved beneath it.
// Resolves whether the panel was beneath it immediately after the move.
function bury() {
  const cover = spawn('powershell.exe', args('Bury'), { stdio: ['ignore', 'pipe', 'pipe'] });
  covers.push(cover);
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(Error('Cover did not appear')), 10000);
    cover.stdout.on('data', chunk => { const ready = /READY buried=(\w+)/.exec(String(chunk)); if (ready) { clearTimeout(timer); resolve(ready[1] === 'True'); } });
    cover.stderr.on('data', chunk => { clearTimeout(timer); reject(Error(String(chunk))); });
    cover.on('error', reject);
  });
}
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, timeout) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try { const value = await check(); if (value) return value; } catch {}
    await pause(200);
  }
  return false;
}
const diagnostics = () => readFile(join(root, 'State', 'panel-diagnostics.json'), 'utf8').then(JSON.parse, () => []);
const receipt = { executable: resolve(process.argv[2]), legacy };
try {
  receipt.panelShown = Boolean(await until(async () => { const s = await inspect(); return s.visible && !s.coveredByNormal; }, 60000));
  if (!receipt.panelShown) throw Error('Private panel never appeared');
  await pause(3000);
  for (let round = 1; round <= 3; round++) {
    const started = Date.now();
    receipt[`round${round}Buried`] = await bury();
    if (!receipt[`round${round}Buried`]) throw Error('Cover did not bury the panel');
    const raised = await until(async () => { const s = await inspect(); return s.visible && !s.coveredByNormal && s.topmost && s; }, legacy ? 12000 : 8000);
    receipt[`round${round}RaisedMs`] = raised ? Date.now() - started : null;
    receipt[`round${round}NonActivating`] = raised ? !raised.foregroundBelongsToApp : null;
    covers.pop().kill();
    if (legacy) break;
    if (!raised) throw Error(`Panel stayed beneath an ordinary window (round ${round})`);
  }
  const actions = await diagnostics();
  receipt.restoredRecorded = actions.filter(x => x.reason === 'topmost_order_restored').length;
  receipt.recreatedOrReloaded = actions.some(x => /recreate|reload|destroy/.test(x.reason));
  if (legacy) {
    receipt.legacyStayedBuried = receipt.round1RaisedMs === null;
    if (!receipt.legacyStayedBuried) throw Error('Legacy build unexpectedly raised the panel');
  } else if (receipt.restoredRecorded < 1 || receipt.recreatedOrReloaded) {
    throw Error('Recovery was not recorded as a Z-order restore');
  }
  receipt.passed = true;
} finally {
  for (const cover of covers) cover.kill();
  app.kill();
  await writeFile(join(root, 'zorder-probe.json'), JSON.stringify(receipt, null, 2));
  console.log(JSON.stringify({ root, ...receipt }, null, 2));
}
