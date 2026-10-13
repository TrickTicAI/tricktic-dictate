// Isolated nginx TLS/configuration proof; never reloads an installed service.
import { expect } from '@playwright/test';
import { spawn, execFileSync } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:net';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

export async function assertUnusedPort(port) {
  const probe = createServer();
  probe.listen(Number(port), '127.0.0.1');
  await once(probe, 'listening');
  await new Promise(resolve => probe.close(resolve));
}

export async function verifyProxy(browser, upstream, stopApi) {
  const port = process.env.WRITER_SECURITY_TLS_TEST_PORT ?? '18806';
  await assertUnusedPort(port);
  const temporary = await mkdtemp(join(tmpdir(), 'writer-tls-policy-'));
  let proxy;
  let context;
  try {
    execFileSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
      '-subj', '/CN=localhost', '-keyout', join(temporary, 'key.pem'), '-out', join(temporary, 'cert.pem')], { stdio: 'ignore' });
    const template = await readFile(fileURLToPath(new URL('../../../docs/security/self-hosted-nginx.conf', import.meta.url)), 'utf8');
    const config = template.replace('listen 443 ssl;', `listen 127.0.0.1:${port} ssl;`)
      .replace('/etc/writer/tls/fullchain.pem', join(temporary, 'cert.pem'))
      .replace('/etc/writer/tls/privkey.pem', join(temporary, 'key.pem'))
      .replace('http://127.0.0.1:8791', upstream);
    const configPath = join(temporary, 'nginx.conf');
    await writeFile(configPath, `daemon off; pid ${temporary}/nginx.pid; error_log ${temporary}/error.log; events {} http { access_log off; client_body_temp_path ${temporary}/body; proxy_temp_path ${temporary}/proxy; ${config} }`);
    execFileSync('nginx', ['-t', '-p', temporary, '-c', configPath], { stdio: 'ignore' });
    proxy = spawn('nginx', ['-p', temporary, '-c', configPath], { stdio: 'ignore' });
    context = await browser.newContext({ ignoreHTTPSErrors: true, permissions: ['microphone'] });
    const url = `https://127.0.0.1:${port}`;
    let ready = false;
    for (let i = 0; i < 100; i++) {
      if (proxy.exitCode !== null) throw Error('Isolated nginx exited; choose an unused WRITER_SECURITY_TLS_TEST_PORT');
      try { ready = (await context.request.get(`${url}/api/v1/health`)).ok(); } catch {}
      if (ready) break;
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    if (!ready) throw Error('Isolated TLS proxy startup timed out');
    const direct = await fetch(upstream);
    for (const path of ['/', '/missing', '/api/v1/health']) {
      const response = await context.request.get(`${url}${path}`);
      expect(response.headers()['strict-transport-security']).toBe('max-age=31536000');
      expect(response.headers()['cache-control']).toBe('no-store');
      expect(response.headers()['content-security-policy']).toBe(direct.headers.get('content-security-policy'));
    }
    const page = await context.newPage();
    await page.goto(url);
    // Real secure-context microphone API through TLS, with Chromium's fake device.
    expect(await page.evaluate(async () => {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      stream.getTracks().forEach(track => track.stop());
      return window.isSecureContext;
    })).toBe(true);
    await stopApi();
    const unavailable = await context.request.get(`${url}/api/v1/health`);
    expect(unavailable.status()).toBe(502);
    expect(unavailable.headers()['cache-control']).toBe('no-store');
    expect(unavailable.headers()['strict-transport-security']).toBe('max-age=31536000');
    expect(unavailable.headers()['x-content-type-options']).toBe('nosniff');
    expect(unavailable.headers()['content-security-policy']).toContain("frame-ancestors 'none'");
    console.log('PASS: nginx configuration, TLS headers, secure-context microphone and protected proxy-generated 502. Temporary test certificate only.');
  } finally {
    await context?.close();
    if (proxy && proxy.exitCode === null) {
      const stopped = once(proxy, 'exit'); proxy.kill('SIGQUIT'); await stopped;
    }
    await rm(temporary, { recursive: true, force: true });
  }
}
