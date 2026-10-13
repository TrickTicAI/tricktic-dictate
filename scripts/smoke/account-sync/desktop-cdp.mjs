// Drives a Writer desktop preview through WebView2 remote debugging for the
// MW-OUATH-11 account/sync acceptance. Run with native Windows Node; no
// dependencies. It calls the app's own Tauri commands and in-process API from
// the main window, so it exercises the real stores, sync engine and Firestore.
//
//   node desktop-cdp.mjs start <app.exe> <user-root> [--offline]  launch hidden, record the PID;
//     --offline sends every non-loopback HTTP(S) request to a dead local proxy
//   node desktop-cdp.mjs run <user-root> <steps.json>  run steps against it, print JSON results
//   node desktop-cdp.mjs stop <user-root> [--kill]     close it (--kill: simulate process death)
//
// Steps: {"invoke": "<command>", "args": {...}} | {"api": "GET /api/v1/vocabulary"} |
// {"api": "PUT /api/v1/vocabulary", "body": {...}} | {"wait": <ms>} | {"eval": "<expression>"}.
// Results can include user text; the caller decides what to record. Never print
// tokens: commands that return them do not exist on the page (sign-in stays native).
import { spawn, execFileSync } from 'node:child_process';
import { readFile, writeFile, rm } from 'node:fs/promises';
import { join, resolve } from 'node:path';

if (process.platform !== 'win32') throw Error('Run with native Windows Node');
const [mode, ...rest] = process.argv.slice(2);
const pause = ms => new Promise(done => setTimeout(done, ms));
const receipt = root => join(resolve(root), '..', `${resolve(root).split(/[\\/]/).pop()}.cdp.json`);

async function until(check, timeout = 45000) {
  const end = Date.now() + timeout;
  let last;
  while (Date.now() < end) {
    try { const value = await check(); if (value) return value; } catch (error) { last = error; }
    await pause(250);
  }
  throw Error(`Timed out${last ? `: ${last.message}` : ''}`);
}

async function connect(root) {
  const target = await until(async () => {
    const port = Number((await readFile(join(root, 'TauriWebView2', 'EBWebView', 'DevToolsActivePort'), 'utf8')).split('\n')[0]);
    const tabs = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
    return tabs.find(tab => tab.type === 'page' && !tab.url.includes('desktop-overlay'));
  });
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((done, fail) => { socket.onopen = done; socket.onerror = fail; });
  let serial = 0;
  const waiting = new Map();
  socket.onmessage = event => {
    const data = JSON.parse(event.data);
    const pending = waiting.get(data.id);
    if (!pending) return;
    waiting.delete(data.id);
    data.error ? pending.fail(Error(data.error.message)) : pending.done(data.result);
  };
  const call = (method, params = {}) => new Promise((done, fail) => {
    const id = ++serial;
    waiting.set(id, { done, fail });
    socket.send(JSON.stringify({ id, method, params }));
    setTimeout(() => { if (waiting.delete(id)) fail(Error(`${method} timed out`)); }, 120000);
  });
  const evaluate = async expression => {
    const result = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) {
      throw Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    }
    return result.result.value;
  };
  await until(() => evaluate('Boolean(window.__TAURI_INTERNALS__) && document.readyState === "complete"'));
  return { evaluate, close: () => socket.close() };
}

if (mode === 'start') {
  const [exe, rootArg, flag] = rest;
  const root = resolve(rootArg);
  const offline = flag === '--offline'
    ? { HTTPS_PROXY: 'http://127.0.0.1:9', HTTP_PROXY: 'http://127.0.0.1:9', NO_PROXY: '127.0.0.1,localhost' }
    : {};
  const app = spawn(resolve(exe), ['--hidden', '--user-root', root], {
    env: { ...process.env, ...offline, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: '--remote-debugging-port=0' },
    stdio: 'ignore', detached: true,
  });
  app.unref();
  await writeFile(receipt(root), JSON.stringify({ pid: app.pid, root, offline: Boolean(flag), started: new Date().toISOString() }));
  const page = await connect(root);
  page.close();
  console.log(JSON.stringify({ started: true, pid: app.pid, offline: flag === '--offline' }));
} else if (mode === 'run') {
  const [rootArg, stepsFile] = rest;
  const page = await connect(resolve(rootArg));
  const steps = JSON.parse(await readFile(stepsFile, 'utf8'));
  const results = [];
  for (const step of steps) {
    try {
      if (step.wait) { await pause(step.wait); results.push({ wait: step.wait }); continue; }
      const expression = step.invoke
        ? `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(step.invoke)}, ${JSON.stringify(step.args ?? {})})`
        : step.api
          ? `(async () => { const [method, path] = ${JSON.stringify(step.api.split(' '))};
              const r = await fetch(path, { method, headers: { 'Content-Type': 'application/json', ...(${JSON.stringify(step.headers ?? {})}) },
                body: ${step.body === undefined ? 'undefined' : JSON.stringify(JSON.stringify(step.body))} });
              const text = await r.text(); let json = null; try { json = JSON.parse(text); } catch {}
              return { status: r.status, etag: r.headers.get('ETag'), json }; })()`
          : step.eval;
      results.push({ step: step.label ?? step.invoke ?? step.api ?? 'eval', ok: true, value: await page.evaluate(expression) });
    } catch (error) {
      results.push({ step: step.label ?? step.invoke ?? step.api ?? 'eval', ok: false, error: String(error.message).slice(0, 300) });
      if (step.required) break;
    }
  }
  page.close();
  console.log(JSON.stringify(results, null, 1));
} else if (mode === 'stop') {
  const [rootArg, flag] = rest;
  const { pid } = JSON.parse(await readFile(receipt(resolve(rootArg)), 'utf8'));
  // --kill is process death (no shutdown path); otherwise ask it to close.
  execFileSync('taskkill', flag === '--kill' ? ['/PID', String(pid), '/F'] : ['/PID', String(pid)], { stdio: 'ignore' });
  if (flag !== '--kill') {
    try { await until(() => { try { process.kill(pid, 0); return false; } catch { return true; } }, 15000); }
    catch { execFileSync('taskkill', ['/PID', String(pid), '/F'], { stdio: 'ignore' }); }
  }
  await rm(receipt(resolve(rootArg)), { force: true });
  console.log(JSON.stringify({ stopped: pid, killed: flag === '--kill' }));
} else {
  throw Error('usage: start <exe> <root> | run <root> <steps.json> | stop <root> [--kill]');
}
