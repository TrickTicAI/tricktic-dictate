// WSL entry point: an isolated browser page acknowledges its actual text change.
// Only synthetic expected text and boolean/count diagnostics cross this fixture.
import { createServer } from 'node:http';
import { mkdir, readFile, writeFile, rename } from 'node:fs/promises';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile, execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const option = name => args[args.indexOf(name) + 1];
if (!args.includes('--output-directory') || (!args.includes('--probe') && !args.includes('--check'))) {
  throw Error('Pass --probe <Windows probe EXE> --output-directory <fresh WSL path>, or --check.');
}
const output = resolve(option('--output-directory'));
await mkdir(output, { recursive: false });
const token = randomUUID();
const title = `Writer clipboard check - ${token}`;
const html = `<!doctype html><meta charset="utf-8"><title>${title}</title>
<style>body{font:18px system-ui;padding:32px;max-width:900px;margin:auto;background:#f5f4ef;color:#202225}button{padding:14px 20px;font:inherit;cursor:pointer}textarea{display:block;width:95%;height:35vh;font-size:22px;margin-top:20px}p{line-height:1.5}</style>
<h1>Writer clipboard check</h1>
<p>Click the button, then leave the keyboard and mouse alone until the result appears. This uses synthetic text and temporarily replaces your clipboard. The check restores it afterwards.</p>
<button id="run">Run clipboard check</button><p id="progress">Ready.</p>
<textarea autofocus aria-label="Synthetic dictation target"></textarea><script>
const field=document.querySelector('textarea');
const button=document.querySelector('#run');
button.addEventListener('click',async()=>{button.disabled=true;document.querySelector('#progress').textContent='Checking paste and clipboard preservation…';field.focus();await fetch('start',{method:'POST'});});
const state={pasteEvents:0,pasteOnField:false,inputEvents:0,focused:false};
const report=()=>{state.focused=document.activeElement===field;fetch('status',{method:'POST',body:JSON.stringify(state)}).catch(()=>{});};
document.addEventListener('paste',e=>{state.pasteEvents++;state.pasteOnField=e.target===field;report();},true);
field.addEventListener('input',()=>{ window.receiptDone=(async()=>{
  const sequence=++state.inputEvents;const value=field.value;report();
  const expected=await fetch('expected').then(r=>r.text());
  const exact=value.replace(/\\r\\n/g,'\\n')===expected.replace(/\\r\\n/g,'\\n');
  await fetch('received',{method:'POST',body:JSON.stringify({exact,sequence})});
})();});
window.addEventListener('load',()=>{field.focus();report();});
window.addEventListener('focus',()=>{report();});
const timer=setInterval(async()=>{
  report();
  if(!button.disabled)return;
  const response=await fetch('result').catch(()=>null);
  if(response?.status!==200)return;
  const result=await response.json();
  clearInterval(timer);
  document.querySelector('#progress').textContent=result.passed
    ? (result.restored?'Passed: paste and clipboard preservation verified.':'Paste verified; this browser used the clipboard fallback.')
    : 'The check did not pass. Its receipt is saved for inspection.';
},500);
</script>`;
let lastReceived = 0;
let writes = Promise.resolve();
const server = createServer(async (request, response) => {
  try {
    const route = request.url?.replace(`/${token}/`, '');
    if (!request.url?.startsWith(`/${token}/`)) { response.writeHead(404).end(); return; }
    if (request.method === 'GET' && route === 'target') {
      response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' }).end(html); return;
    }
    if (request.method === 'GET' && route === 'expected') {
      const expected = await readFile(resolve(output, 'expected-browser.txt'));
      response.writeHead(200, { 'Content-Type': 'text/plain; charset=utf-8' }).end(expected); return;
    }
    if (request.method === 'GET' && route === 'result') {
      try {
        const result = JSON.parse((await readFile(resolve(output, 'browser.json'), 'utf8')).replace(/^\uFEFF/, ''));
        response.writeHead(200, { 'Content-Type': 'application/json' }).end(JSON.stringify(result));
      } catch { response.writeHead(204).end(); }
      return;
    }
    if (request.method === 'POST' && route === 'start') {
      await writeFile(resolve(output, 'start-browser.json'), '{}');
      response.writeHead(204).end(); return;
    }
    if (request.method === 'POST' && ['received', 'status'].includes(route)) {
      let body = '';
      for await (const chunk of request) { body += chunk; if (body.length > 1024) throw Error('Oversized receipt'); }
      const value = JSON.parse(body);
      if (route === 'received') {
        const sequence = Number(value.sequence);
        if (!Number.isInteger(sequence) || sequence <= lastReceived) { response.writeHead(204).end(); return; }
        lastReceived = sequence;
      }
      const receipt = route === 'received' ? { exact: value.exact === true } : {
        pasteEvents: Number(value.pasteEvents) || 0, pasteOnField: value.pasteOnField === true,
        inputEvents: Number(value.inputEvents) || 0, focused: value.focused === true,
      };
      const destination = resolve(output, route === 'received' ? 'received-browser.json' : 'browser-status.json');
      writes = writes.then(async () => {
        const temporary = `${destination}.${randomUUID()}.tmp`;
        await writeFile(temporary, JSON.stringify(receipt));
        await rename(temporary, destination);
      });
      await writes;
      response.writeHead(204).end(); return;
    }
    response.writeHead(404).end();
  } catch { response.writeHead(400).end(); }
});
await new Promise(resolveReady => server.listen(0, '127.0.0.1', resolveReady));
const url = `http://127.0.0.1:${server.address().port}/${token}/target`;
try {
  if (args.includes('--check')) {
    await new Promise((ok, fail) => execFile('/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe', [
      '-NoProfile', '-NonInteractive', '-Command', `[int](Invoke-WebRequest -UseBasicParsing -TimeoutSec 5 -Uri '${url}').StatusCode`,
    ], (error, stdout) => error || stdout.trim() !== '200' ? fail(Error('Windows could not reach the WSL fixture')) : ok()));
    const { chromium } = await import('../../apps/web/node_modules/@playwright/test/index.mjs');
    const browser = await chromium.launch();
    try {
      const expected = 'MW67 fixture Hebrew שלום English\nsecond line';
      await writeFile(resolve(output, 'expected-browser.txt'), expected);
      const page = await browser.newPage();
      await page.goto(url);
      for (const value of [expected, 'incorrect fixture']) {
        await page.locator('textarea').fill(value);
        await page.evaluate(() => window.receiptDone);
        const { exact } = JSON.parse(await readFile(resolve(output, 'received-browser.json'), 'utf8'));
        if (exact !== (value === expected)) throw Error('Browser receipt did not distinguish exact and incorrect text');
      }
      console.log('Browser receipt check passed: exact mixed-language text accepted; incorrect text rejected.');
    } finally { await browser.close(); }
  } else {
    const windowsPath = path => execFileSync('wslpath', ['-w', resolve(path)], { encoding: 'utf8' }).trim();
    await new Promise((ok, fail) => {
      const child = execFile('/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe', [
        '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', windowsPath(resolve(here, 'smoke-clipboard-browser.ps1')),
        '-Probe', windowsPath(option('--probe')), '-OutputDirectory', windowsPath(output), '-FixtureUrl', url, '-FixtureTitle', title, ...(args.includes('--interactive') ? ['-Interactive'] : []),
      ], error => error ? fail(Error(`Native browser fixture failed; inspect ${output}`)) : ok());
      child.stdout.pipe(process.stdout); child.stderr.pipe(process.stderr);
    });
  }
} finally { server.closeAllConnections(); await new Promise(resolveClosed => server.close(resolveClosed)); }
