import assert from 'node:assert/strict';
import { once } from 'node:events';
import { createServer as createHttpServer, get } from 'node:http';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createServer, loadConfigFromFile } from 'vite';

// Only synthetic files and an ephemeral API fixture; never use a running API.
test('Vite keeps loopback defaults, explicit host overrides and private files', { timeout: 15_000 }, async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'writer-vite-security-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const root = join(directory, 'web');
  await mkdir(root);
  const marker = 'synthetic-private-fixture';
  await writeFile(join(root, '.env'), `OPENAI_API_KEY=${marker}\n`);
  await writeFile(join(root, 'tls.pem'), marker);
  await writeFile(join(root, 'index.html'), '<div>public fixture</div>');
  await writeFile(join(root, 'client.js'), 'console.log(import.meta.env.OPENAI_API_KEY);');
  await writeFile(join(directory, 'outside.map'), JSON.stringify({
    version: 3, sources: [], names: [], mappings: '', sourcesContent: [marker],
  }));
  const api = createHttpServer((req, res) => {
    // Ambient provider credentials must never become proxy headers.
    assert.ok(!JSON.stringify(req.headers).includes(marker));
    assert.equal(req.headers.authorization, undefined);
    res.setHeader('Content-Type', 'application/json');
    res.end(JSON.stringify({ path: req.url }));
  });
  api.listen(0, '127.0.0.1');
  await once(api, 'listening');
  t.after(() => new Promise((resolve) => api.close(resolve)));

  const names = ['WRITER_WEB_HOST', 'WRITER_WEB_PORT', 'WRITER_API_PORT', 'OPENAI_API_KEY'];
  const saved = names.map((name) => process.env[name]);
  t.after(() => names.forEach((name, index) => {
    if (saved[index] === undefined) delete process.env[name];
    else process.env[name] = saved[index];
  }));
  delete process.env.WRITER_WEB_HOST;
  delete process.env.WRITER_WEB_PORT;
  process.env.WRITER_API_PORT = String(api.address().port);
  process.env.OPENAI_API_KEY = marker;
  const configFile = fileURLToPath(new URL('./vite.config.ts', import.meta.url));
  const load = async () => (await loadConfigFromFile({ command: 'serve', mode: 'development' }, configFile)).config;
  const defaults = await load();
  assert.equal(defaults.server.host, '127.0.0.1');
  assert.equal(defaults.server.port, 5174);
  assert.equal(defaults.server.strictPort, true);
  process.env.WRITER_WEB_HOST = '0.0.0.0';
  process.env.WRITER_WEB_PORT = '15199';
  const override = await load();
  assert.equal(override.server.host, '0.0.0.0');
  assert.equal(override.server.port, 15199);
  assert.equal(override.server.proxy['/api'].target, `http://127.0.0.1:${api.address().port}`);

  // Exercise the real middleware on loopback with both configuration variants.
  // Do not expose even a temporary fixture listener to the network.
  for (const config of [defaults, override]) {
    // Vite treats port 0 as its default port. Reserve an OS-selected port,
    // release it, and retain strictPort so a race fails without taking over.
    const reservation = createHttpServer();
    reservation.listen(0, '127.0.0.1');
    await once(reservation, 'listening');
    const port = reservation.address().port;
    await new Promise((resolve) => reservation.close(resolve));
    const server = await createServer({
      ...config, configFile: false, root, envFile: false, logLevel: 'silent',
      cacheDir: join(directory, 'cache'),
      server: { ...config.server, host: '127.0.0.1', port, fs: { allow: [root] } },
    });
    try {
      await server.listen();
      const base = `http://127.0.0.1:${server.httpServer.address().port}`;
      assert.match(await (await fetch(base)).text(), /public fixture/);
      assert.deepEqual(await (await fetch(`${base}/api/probe`)).json(), { path: '/api/probe' });
      assert.ok(!(await (await fetch(`${base}/client.js`)).text()).includes(marker));
      const foreign = await fetch(base, { headers: { Origin: 'https://untrusted.invalid' } });
      assert.equal(foreign.headers.get('access-control-allow-origin'), null);
      const reboundStatus = await new Promise((resolve, reject) => {
        get(base, { headers: { Host: 'untrusted.invalid' } }, (response) => {
          response.resume();
          resolve(response.statusCode);
        }).on('error', reject);
      });
      assert.equal(reboundStatus, 403);
      for (const path of ['/.env', '/.env?raw', '/tls.pem', `/@fs/${join(directory, 'outside.map')}`]) {
        const response = await fetch(`${base}${path}`);
        assert.equal(response.status, 403, path);
        assert.ok(!(await response.text()).includes(marker), path);
      }
      const traversal = await fetch(`${base}/node_modules/.vite/deps/..%2f..%2f..%2f..%2foutside.map`);
      assert.ok(!(await traversal.text()).includes(marker));
      // Native NTFS path semantics are only testable on Windows.
      await t.test('NTFS alternate data streams do not return private files', { skip: process.platform !== 'win32' }, async () => {
        for (const path of ['/.env::$DATA?raw', '/tls.pem::$DATA?raw']) {
          const response = await fetch(`${base}${path}`);
          assert.ok(!(await response.text()).includes(marker), path);
        }
      });
    } finally {
      await server.close();
    }
  }
});
