// Run with native Windows Node from WSL. Requires a built Writer API on localhost.
// Only starts/stops an isolated Edge instance with synthetic microphone hardware.
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
const url = process.argv[2] || 'http://localhost:18791';
const profile = await mkdtemp(join(tmpdir(), 'writer-edge-proof-'));
const edge = spawn('C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe', [
  '--headless=new', '--autoplay-policy=no-user-gesture-required', '--window-size=1280,1000', '--no-first-run', '--no-default-browser-check', '--remote-debugging-port=0',
  `--user-data-dir=${profile}`, '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream', 'about:blank',
], { stdio: 'ignore' });
let socket;
let serial = 0;
const waiting = new Map();
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, timeout = 20000) { const end = Date.now() + timeout; while (Date.now() < end) { try { const value = await check(); if (value) return value; } catch {} await pause(200); } throw Error('Browser condition timed out'); }
function cdp(method, params = {}) { return new Promise((resolve, reject) => { const id = ++serial; waiting.set(id, { resolve, reject }); socket.send(JSON.stringify({ id, method, params })); }); }
async function evaluate(expression) { const result = await cdp('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }); if (result.exceptionDetails) throw Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text); return result.result.value; }
try {
  const debugPort = await until(async () => Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]));
  const tabs = await until(async () => { const response = await fetch(`http://127.0.0.1:${debugPort}/json`); return response.ok && response.json(); });
  socket = new WebSocket(tabs.find(tab => tab.type === 'page').webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  socket.onmessage = event => { const data = JSON.parse(event.data); const pending = waiting.get(data.id); if (pending) { waiting.delete(data.id); data.error ? pending.reject(Error(data.error.message)) : pending.resolve(data.result); } };
  await cdp('Page.enable'); await cdp('Runtime.enable'); await cdp('Page.navigate', { url });
  await until(() => evaluate(`document.body.innerText.includes('API connected') && [...document.querySelectorAll('button')].some(b=>b.textContent==='Start recording'&&!b.disabled)`));
  const browser = await evaluate('navigator.userAgent');
  const secure = await evaluate('window.isSecureContext');
  await evaluate(`[...document.querySelectorAll('button')].find(b=>b.textContent==='Start recording').click()`);
  await until(() => evaluate(`document.body.innerText.includes('Stop recording')`)); await pause(2200);
  await evaluate(`[...document.querySelectorAll('button')].find(b=>b.textContent==='Stop recording').click()`);
  await until(() => evaluate(`document.querySelectorAll('.recording-row').length===1 && document.querySelector('audio')?.readyState>0`));
  const before = await evaluate(`(async()=>{const audio=document.querySelector('audio');const blob=await(await fetch(audio.src)).arrayBuffer();return {bytes:blob.byteLength,sha:Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',blob))).map(x=>x.toString(16).padStart(2,'0')).join('')}})()`);
  await cdp('Page.reload');
  await until(() => evaluate(`document.querySelectorAll('.recording-row').length===1 && document.querySelector('audio')?.readyState>0`));
  const after = await evaluate(`(async()=>{const audio=document.querySelector('audio');const blob=await(await fetch(audio.src)).arrayBuffer();await audio.play();return {bytes:blob.byteLength,sha:Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',blob))).map(x=>x.toString(16).padStart(2,'0')).join('')}})()`);
  if (!secure || before.bytes <= 0 || before.sha !== after.sha) throw Error('Saved capture proof failed');
  const screenshot = await cdp('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true });
  await writeFile(join(profile, 'writer.png'), Buffer.from(screenshot.data, 'base64'));
  const result = { passed: true, browser, url, secure, before, after, realMicrophone: false, liveTranscription: false, screenshot: join(profile, 'writer.png'), profile };
  await writeFile(join(profile, 'result.json'), JSON.stringify(result, null, 2)); console.log(JSON.stringify(result));
} finally { socket?.close(); edge.kill(); }
