// Built UI and real isolated API, with synthetic microphone audio. No provider calls.
import { verifyProxy, assertUnusedPort } from './proxy.mjs';
import { chromium, expect } from '@playwright/test';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:http';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../..', import.meta.url));
const port = process.env.WRITER_SECURITY_TEST_PORT ?? '18805';
const origin = `http://127.0.0.1:${port}`;
await assertUnusedPort(port);
const data = await mkdtemp(join(tmpdir(), 'writer-browser-policy-'));
const api = spawn(join(root, 'target/debug/metrix-writer-api'), [], {
  cwd: root, stdio: 'ignore',
  // Do not inherit provider keys, runtime endpoints, or another instance's storage.
  env: { PATH: process.env.PATH, HOME: process.env.HOME,
    WRITER_API_HOST: '127.0.0.1', WRITER_API_PORT: port,
    WRITER_DATA_DIR: data, WRITER_WEB_DIST: join(root, 'apps/web/dist') },
});
const foreign = createServer((_request, response) => {
  response.setHeader('Content-Type', 'text/html');
  response.end(`<!doctype html><iframe src="${origin}/"></iframe><iframe src="${origin}/desktop-overlay.html?preview"></iframe>`);
});
let browser;
function headers(response) {
  expect(response.headers.get('cache-control')).toBe('no-store');
  expect(response.headers.get('x-content-type-options')).toBe('nosniff');
  expect(response.headers.get('content-security-policy')).toContain("frame-ancestors 'self'");
  expect(response.headers.get('x-frame-options')).toBe('SAMEORIGIN');
  expect(response.headers.get('strict-transport-security')).toBeNull();
}
try {
  let ready = false;
  for (let i = 0; i < 100; i++) {
    if (api.exitCode !== null) throw Error('Isolated API exited; choose an unused WRITER_SECURITY_TEST_PORT');
    try { ready = (await fetch(`${origin}/api/v1/health`)).ok; } catch {}
    if (ready) break;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  if (!ready) throw Error('Isolated API startup timed out');
  const providers = await (await fetch(`${origin}/api/v1/providers`)).json();
  expect(providers.providers.some(provider => provider.configured)).toBe(false);
  for (const path of ['/', '/desktop-overlay.html?preview', '/desktop-overlay.js', '/missing', '/api/v1/missing', '/api/v1/health']) {
    headers(await fetch(`${origin}${path}`));
  }
  browser = await chromium.launch({ args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'] });
  const context = await browser.newContext({ permissions: ['microphone'], viewport: { width: 1280, height: 1000 } });
  await context.addInitScript(() => {
    window.policyViolations = [];
    document.addEventListener('securitypolicyviolation', event => window.policyViolations.push(event.effectiveDirective));
  });
  const page = await context.newPage();
  // Only discovery is mocked: the real API rejects the unconfigured provider.
  await page.route('**/api/v1/providers', route => route.fulfill({ json: { providers: [
    { id: 'openai', name: 'Fixture discovery', configured: true, available: false, status: 'unverified' },
  ] } }));
  const submissions = [];
  page.on('request', request => {
    if (new URL(request.url()).pathname === '/api/v1/transcriptions') submissions.push(request.postDataJSON().data);
  });
  await page.goto(origin);
  await page.getByRole('button', { name: 'Record', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
  await expect(page.getByLabel('Recording duration')).not.toHaveText('00:00');
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect(page.getByText('The saved audio is untouched and can be retried.')).toBeVisible();
  const id = submissions[0].recording_id;
  await page.reload();
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await expect.poll(() => submissions.length).toBe(2);
  await expect(page.getByText('The saved audio is untouched and can be retried.')).toBeVisible();
  expect(submissions[1].recording_id).toBe(id);
  expect(submissions[1].retry_of_job_id).toBeTruthy();
  for (const path of [`/api/v1/recordings/${id}`, `/api/v1/recordings/${id}/audio`, '/api/v1/recordings/bad-id/audio']) {
    headers(await fetch(`${origin}${path}`));
  }
  const audio = page.locator('audio');
  await audio.evaluate(element => element.play());
  await expect.poll(() => audio.evaluate(element => element.currentTime)).toBeGreaterThan(0);
  expect(await page.evaluate(async id => {
    const local = await (await fetch(document.querySelector('audio').src)).arrayBuffer();
    const remote = await (await fetch(`/api/v1/recordings/${id}/audio`)).arrayBuffer();
    return local.byteLength > 0 && local.byteLength === remote.byteLength
      && new Uint8Array(local).every((byte, i) => byte === new Uint8Array(remote)[i]);
  }, id)).toBe(true);
  const download = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Audio', exact: true }).click();
  expect(await (await download).failure()).toBeNull();
  expect(await page.evaluate(async () => {
    await document.fonts.load('400 15px Inter');
    await document.fonts.load('400 15px Assistant', 'שלום');
    return document.fonts.check('400 15px Inter') && document.fonts.check('400 15px Assistant', 'שלום');
  })).toBe(true);
  // The actual production preview document must execute its script in a same-origin frame.
  await page.evaluate(() => {
    const frame = document.createElement('iframe');
    frame.title = 'Policy panel fixture'; frame.src = '/desktop-overlay.html?preview';
    document.body.append(frame);
  });
  const panel = page.frameLocator('iframe[title="Policy panel fixture"]');
  await expect(panel.locator('body')).toHaveAttribute('data-state', 'idle');
  await page.evaluate(() => document.querySelector('iframe').contentWindow.postMessage({
    source: 'writer-panel-preview', name: 'writer-session', payload: {
      version: 1, kind: 'snapshot', epoch: 'policy-test', revision: 1,
      session_id: 'policy-test', state: 'capturing', job_id: null, audio_saved: false, error_code: null,
    },
  }, location.origin));
  await expect(panel.locator('body')).toHaveAttribute('data-state', 'capturing');
  for (const frame of page.frames()) expect(await frame.evaluate(() => window.policyViolations)).toEqual([]);
  // Injection probes: no inline scripts, eval, or remote script origins are allowed.
  await page.route('**/policy-eval-probe.js', route => route.fulfill({ contentType: 'text/javascript',
    body: "try { eval('window.injected = true'); } catch { window.evalBlocked = true; }" }));
  await page.evaluate(() => {
    const evalProbe = document.createElement('script'); evalProbe.src = '/policy-eval-probe.js'; document.body.append(evalProbe);
    const inline = document.createElement('script'); inline.textContent = 'window.injected = true'; document.body.append(inline);
    const remote = document.createElement('script'); remote.src = 'https://example.invalid/probe.js'; document.body.append(remote);
  });
  await expect.poll(() => page.evaluate(() => window.policyViolations.length)).toBeGreaterThanOrEqual(3);
  expect(await page.evaluate(() => window.injected)).toBeUndefined();
  expect(await page.evaluate(() => window.evalBlocked)).toBe(true);
  foreign.listen(0, '127.0.0.1'); await once(foreign, 'listening');
  const attacker = await context.newPage();
  const rejected = [];
  attacker.on('console', message => { if (/frame-ancestors|X-Frame-Options/.test(message.text())) rejected.push(message.text()); });
  await attacker.goto(`http://127.0.0.1:${foreign.address().port}`);
  await expect.poll(() => rejected.length).toBeGreaterThanOrEqual(2);
  expect(attacker.frames().filter(frame => frame.url().startsWith(origin))).toHaveLength(0);
  await verifyProxy(browser, origin, async () => {
    const exited = once(api, 'exit'); api.kill('SIGTERM'); await exited;
  });
  console.log('PASS: response headers, capture, IndexedDB reload/retry, blob fetch/playback/export, fonts, same-origin panel, blocked injection and foreign framing. Synthetic audio; no live transcription.');
} finally {
  await browser?.close();
  foreign.close();
  const exited = api.exitCode === null && api.signalCode === null ? once(api, 'exit') : Promise.resolve();
  api.kill('SIGTERM'); await exited;
  await rm(data, { recursive: true, force: true });
}
