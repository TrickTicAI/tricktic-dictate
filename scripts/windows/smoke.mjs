// Run under native Windows Node 22.9+. Serves built production UI plus explicit
// transcription fixtures. No cloud calls, real microphone, or user's app targets.
import { createServer } from 'node:http';
import { readFile, mkdtemp, writeFile } from 'node:fs/promises';
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { tmpdir } from 'node:os';
import { join, extname, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
const exe = process.argv[2];
if (!exe || !process.argv.includes('--interactive')) throw Error('This test opens Windows windows and briefly uses the clipboard. Run only on an approved interactive desktop: node scripts/windows/smoke.mjs C:\\path\\MetrixWriter.Smoke.exe --interactive');
const assets = fileURLToPath(new URL('../../apps/web/dist/', import.meta.url));
const directory = await mkdtemp(join(tmpdir(), 'writer-native-proof-'));
const records = new Map(), jobs = new Map();
let mode = 'fixture';
const server = createServer(async (req, res) => {
  const send = (data, status = 200) => { res.writeHead(status, { 'Content-Type': 'application/json' }); res.end(JSON.stringify(data)); };
  const envelope = data => ({ api_version: '1', data });
  try {
    if (mode === 'outage') return req.socket.destroy();
    const path = new URL(req.url, 'http://localhost').pathname;
    if (path.startsWith('/api/')) {
      if (mode === 'api-offline') return send({ error: 'Intentional isolated API outage' }, 503);
      if (path.endsWith('/health')) return send({ status: 'ok' });
      if (path.endsWith('/providers')) return send(path.includes('/cleanup/') ? envelope([]) : { providers: [{ id: 'openai', name: 'Fixture only', configured: true, available: false, status: 'unverified' }] });
      if (path.endsWith('/vocabulary')) return send(envelope({ hints: [], corrections: [], snippets: [] }));
      const parts = []; for await (const part of req) parts.push(part);
      const body = Buffer.concat(parts).toString();
      if (path === '/api/v1/recordings') {
        const metadata = JSON.parse(body.split('name="metadata"\r\n\r\n')[1].split('\r\n--')[0]).data;
        const recording = { metadata, storage: 'self_hosted_server', audio: { state: 'saved' } };
        records.set(metadata.id, recording); return send(envelope(recording), 201);
      }
      if (path.startsWith('/api/v1/recordings/')) {
        const record = records.get(path.split('/').at(-1));
        return send(record ? envelope(record) : { api_version: '1', error: { code: 'recording_not_found', message: 'Fixture absent', retryable: false } }, record ? 200 : 404);
      }
      if (path === '/api/v1/transcriptions') {
        const request = JSON.parse(body).data;
        const job = { id: randomUUID(), request, created_at_ms: Date.now(), updated_at_ms: Date.now(), status: { state: 'succeeded', result: { cleanup: { state: 'off' }, transcript: { raw_text: 'שלום Metrix — Windows fixture', input_language: request.options.input_language, output_language: request.options.output_language, selection: request.options.selection, execution_location: 'cloud' } } } };
        jobs.set(job.id, job); return send(envelope(job), 202);
      }
      if (path.endsWith('/processed')) return send(envelope({ text: 'שלום Metrix — Windows fixture' }));
      if (path.startsWith('/api/v1/jobs/')) return send(envelope(jobs.get(path.split('/').at(-1))));
      return send({ error: 'Unknown fixture route' }, 404);
    }
    const asset = resolve(assets, '.' + (path === '/' ? '/index.html' : path));
    if (!asset.startsWith(resolve(assets) + '\\')) return send({ error: 'Invalid asset' }, 400);
    res.writeHead(200, { 'Content-Type': ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.woff': 'font/woff', '.woff2': 'font/woff2' })[extname(asset)] || 'application/octet-stream' });
    res.end(await readFile(asset));
  } catch (error) { if (!res.headersSent) send({ error: String(error) }, 500); else res.destroy(); }
});
await new Promise((yes, no) => { server.once('error', no); server.listen(0, '127.0.0.1', yes); });
const url = `http://localhost:${server.address().port}/`;
const phases = [];
try {
  // Launch the exact shipped executable as well as the instrumented same-source shell.
  const production = spawn(join(dirname(exe), 'MetrixWriter.exe'), ['--url', url, '--profile', join(directory, 'production-profile')], { stdio: 'ignore' });
  try {
    await new Promise(yes => setTimeout(yes, 3500));
    const command = `$p = Get-Process -Id ${production.pid} -ErrorAction Stop; if ($p.MainWindowTitle -notlike 'Metrix Writer*') { throw 'Production window missing' }; if (!$p.CloseMainWindow()) { throw 'Production window did not accept close' }; if (!$p.WaitForExit(10000)) { throw 'Production window did not close' }`;
    await promisify(execFile)('powershell.exe', ['-NoProfile', '-Command', command]);
    phases.push('production-exe-launch');
    console.log('PASS production-exe-launch');
  } finally { if (production.exitCode === null) production.kill(); }
  for (const phase of ['capture', 'outage', 'recover', 'crash', 'interrupted']) {
    mode = phase === 'outage' ? 'outage' : phase === 'capture' ? 'fixture' : 'api-offline';
    const child = spawn(exe, [url, directory, phase], { stdio: 'inherit' });
    const timer = setTimeout(() => child.kill(), 90000);
    try { await new Promise((yes, no) => { child.on('error', no); child.on('exit', code => code === 0 ? yes() : no(Error(`${phase} exited ${code}`))); }); }
    finally { clearTimeout(timer); }
    phases.push(phase);
    await new Promise(yes => setTimeout(yes, 1200));
  }
  const result = { passed: true, url, phases, directory, audio: await readFile(join(directory, 'audio-hash.txt'), 'utf8'), nativeWindows: true, syntheticMicrophone: true, liveTranscription: false, clipboardTarget: 'owned native WinForms TextBox', hotkey: 'RegisterHotKey + WM_HOTKEY dispatch; no physical keyboard event' };
  await writeFile(join(directory, 'result.json'), JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result, null, 2));
} finally { server.closeAllConnections(); server.close(); }
