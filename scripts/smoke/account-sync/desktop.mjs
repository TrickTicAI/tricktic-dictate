import http from 'node:http';
import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { beginAttempt, consumeCallback, exchangeCode, refreshSession,
  validateConfig, ProofError } from './auth-proof.mjs';

export function openBrowser(url) {
  // Run this on the native target, not WSL/SSH with a browser on another host.
  const command = process.platform === 'win32' ? ['rundll32.exe', ['url.dll,FileProtocolHandler', url]]
    : process.platform === 'darwin' ? ['open', [url]] : ['xdg-open', [url]];
  const child = spawn(command[0], command[1], { stdio: 'ignore', shell: false });
  return new Promise((resolve, reject) => {
    child.once('error', () => reject(new ProofError('browser_unavailable')));
    child.once('exit', (code) => code === 0 ? resolve() : reject(new ProofError('browser_unavailable')));
  });
}

export async function browserCode(clientId, { launch = openBrowser, timeoutMs = 180_000 } = {}) {
  let attempt;
  let complete;
  let rejectResult;
  const result = new Promise((resolve, reject) => { complete = resolve; rejectResult = reject; });
  // Register immediately so a launcher failure cannot cause an unhandled rejection.
  result.catch(() => {});
  const server = http.createServer((req, res) => {
    res.setHeader('Connection', 'close');
    res.setHeader('Cache-Control', 'no-store');
    res.setHeader('Referrer-Policy', 'no-referrer');
    res.setHeader('Content-Type', 'text/html; charset=utf-8');
    res.setHeader('Content-Security-Policy', "default-src 'none'; script-src 'unsafe-inline'; frame-ancestors 'none'");
    try {
      if (req.method !== 'GET') throw new ProofError('invalid_callback');
      const code = consumeCallback(attempt, req.url, req.headers.host);
      res.once('finish', () => complete({ attempt, code }));
      res.end('<script>history.replaceState(null,"","/done")</script>Return received. Close this tab and return to the proof terminal.');
    } catch (error) {
      res.statusCode = 400;
      // Stray/forged requests must not terminate a legitimate pending attempt.
      if (attempt?.consumed) res.once('finish', () => rejectResult(error));
      res.end('<script>history.replaceState(null,"","/done")</script>Sign-in was cancelled or the return was invalid. Return to the proof terminal.');
    }
  });
  server.requestTimeout = 30_000;
  server.headersTimeout = 30_000;
  server.on('clientError', (_error, socket) => socket.destroy());
  await new Promise((resolve, reject) => {
    server.once('error', () => reject(new ProofError('listener_unavailable')));
    server.listen(0, '127.0.0.1', resolve);
  });
  const timer = setTimeout(() => rejectResult(new ProofError('timed_out')), timeoutMs);
  try {
    const start = beginAttempt(clientId, `http://127.0.0.1:${server.address().port}/callback`);
    attempt = start.attempt;
    // Race launcher failure/timeout without logging the authorization URL/state.
    await Promise.race([launch(start.url), result]);
    return await result;
  } finally {
    clearTimeout(timer);
    // Let the callback page flush before closing its socket. Destroying every
    // connection immediately can reset the browser's successful return.
    const drainTimer = setTimeout(() => server.closeAllConnections(), 5000);
    try { await new Promise((resolve) => server.close(resolve)); }
    finally { clearTimeout(drainTimer); }
  }
}

async function main() {
  if (process.argv.length !== 3) throw new ProofError('usage: node desktop.mjs <private-config.json>');
  const config = validateConfig(JSON.parse(await readFile(process.argv[2], 'utf8')));
  console.log('Opening system browser. Complete or cancel Google sign-in; timeout is 3 minutes.');
  const { attempt, code } = await browserCode(config.desktopClientId);
  const session = await exchangeCode(config, attempt, code);
  await refreshSession(config, session);
  console.log(JSON.stringify({ proof: 'desktop-live', login: 'passed', refresh: 'passed',
    audience: 'configured-desktop-client', sync: 'off', persistence: 'not-tested' }));
  // Tokens stay in process memory and disappear on process exit. This is not
  // refresh-token revocation, native persistence proof or production sign-out.
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error instanceof ProofError ? error.code : 'proof_failed');
    process.exitCode = 1;
  });
}
