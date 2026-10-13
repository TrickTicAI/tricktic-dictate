import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
const read = path => readFile(new URL(`../../${path}`, import.meta.url), 'utf8');

test('every registered custom command has an explicit window policy', async () => {
  const shell = await read('apps/desktop/tauri/src/shell.rs');
  const handler = shell.match(/tauri::generate_handler!\[([^\]]+)\]/)[1];
  const registered = handler.split(',').map(name => name.trim().split('::').at(-1));
  const policy = await read('apps/desktop/tauri/src/shell/security.rs');
  const declared = [...policy.slice(0, policy.indexOf('pub fn local_url')).matchAll(/"([a-z_]+)"/g)].map(match => match[1]);
  declared.push('overlay_action');
  assert.equal(new Set(registered).size, registered.length);
  assert.deepEqual(registered.sort(), declared.sort());
});

test('packaged capabilities and CSP keep frames and general OS access closed', async () => {
  const config = JSON.parse(await read('apps/desktop/tauri/tauri.conf.json'));
  const capability = JSON.parse(await read('apps/desktop/tauri/capabilities/desktop.json'));
  assert.deepEqual(capability.windows, ['main', 'overlay', 'tray-menu']);
  assert.deepEqual(capability.permissions, ['core:event:allow-listen', 'core:event:allow-unlisten', 'core:window:allow-start-dragging']);
  assert.equal(capability.remote, undefined);
  for (const directive of ["frame-src 'none'", "object-src 'none'", "base-uri 'none'", "form-action 'none'", "script-src 'self'"]) assert.ok(config.app.security.csp.includes(directive));
  assert.equal(config.app.security.dangerousDisableAssetCspModification, undefined);
});

test('ordinary packaging does not enable security or delivery fixtures or the retired C# build', async () => {
  for (const file of ['scripts/desktop/build.mjs', 'scripts/desktop/installer.mjs', 'scripts/desktop/macos-installer.mjs']) {
    const source = await read(file);
    assert.doesNotMatch(source, /security-probe|latency-probe|scripts\/windows|apps\/desktop\/windows|csc\.exe/);
  }
  const cargo = await read('apps/desktop/tauri/Cargo.toml');
  assert.doesNotMatch(cargo, /^default\s*=.*(?:security-probe|latency-probe)/m);
});


test('artifact gate rejects legacy entry points and instrumented executables', async () => {
  const { assertSupportedDesktopResources, assertNoDesktopFixture } = await import('./security-artifact.mjs');
  const { mkdtemp, writeFile, rm } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  assertSupportedDesktopResources(['runtime/whisper/whisper-server.exe', 'notices/RUST-NOTICES.txt', 'runtime-manifest.json']);
  for (const path of ['MetrixWriter.exe', 'MetrixWriter.Launcher.exe', 'MetrixWriter.exe.config', 'Microsoft.Web.WebView2.Core.dll', 'backend/metrix-writer-api.exe', 'windows/Writer.cs']) assert.throws(() => assertSupportedDesktopResources([path]));
  const root = await mkdtemp(join(tmpdir(), 'writer-artifact-security-'));
  try {
    const binary = join(root, 'fixture.exe');
    await writeFile(binary, Buffer.from('ordinary Tauri application'));
    await assertNoDesktopFixture(binary);
    for (const encoding of ['utf8', 'utf16le']) {
      await writeFile(binary, Buffer.from('MZ security_probe_receipt', encoding));
      await assert.rejects(assertNoDesktopFixture(binary));
    }
  } finally { await rm(root, { recursive: true }); }
});

test('desktop releases require configured sign-in; optional development builds stay account-free', async () => {
  const { assertNoGoogleSignIn, assertGoogleSignIn, desktopSignInConfig, googleSignInEnv } = await import('./security-artifact.mjs');
  const { mkdtemp, writeFile, rm } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const cargo = await read('apps/desktop/tauri/Cargo.toml');
  assert.doesNotMatch(cargo, /^default\s*=.*google-sign-in/m);
  assert.equal(desktopSignInConfig({ release: false, env: {} }), undefined);
  assert.throws(() => desktopSignInConfig({ release: true, env: {} }), /refusing a release/);
  assert.equal(desktopSignInConfig({ release: true, env: { WRITER_GOOGLE_SIGN_IN_CONFIG: '/private/account.json' } }), '/private/account.json');
  assert.equal(desktopSignInConfig({ path: '/override.json', release: true, env: { WRITER_GOOGLE_SIGN_IN_CONFIG: '/private/account.json' } }), '/override.json');
  for (const path of ['', '--release']) assert.throws(() => desktopSignInConfig({ path, release: false, env: {} }));
  const root = await mkdtemp(join(tmpdir(), 'writer-sign-in-gate-'));
  try {
    const binary = join(root, 'app.exe');
    await writeFile(binary, Buffer.from('ordinary Tauri application'));
    await assertNoGoogleSignIn(binary);
    await assert.rejects(assertGoogleSignIn(binary));
    for (const encoding of ['utf8', 'utf16le']) {
      await writeFile(binary, Buffer.from('MZ https://accounts.google.com/o/oauth2/v2/auth', encoding));
      await assert.rejects(assertNoGoogleSignIn(binary));
      await assert.rejects(assertGoogleSignIn(binary));
      await writeFile(binary, Buffer.from('MZ https://accounts.google.com/o/oauth2/v2/auth https://securetoken.googleapis.com', encoding));
      await assertGoogleSignIn(binary);
    }
    const config = join(root, 'config.json');
    const values = { projectId: 'metrix-writer', apiKey: 'synthetic-key', desktopClientId: 'synthetic.apps.googleusercontent.com', desktopClientSecret: 'synthetic-secret' };
    await writeFile(config, JSON.stringify(values));
    await assert.rejects(googleSignInEnv({ path: config, windows: true, release: true }), /accountServiceUrl/);
    const env = await googleSignInEnv({ path: config, windows: true, release: false });
    assert.equal(env.WRITER_GOOGLE_DESKTOP_CLIENT_ID, values.desktopClientId);
    await assert.rejects(googleSignInEnv({ path: config, windows: false, release: true }), /accountServiceUrl/);
    assert.equal(env.WRITER_ACCOUNT_SERVICE_URL, '');
    await writeFile(config, JSON.stringify({ ...values, accountServiceUrl: 'https://cleanup.example.test' }));
    for (const windows of [true, false]) {
      assert.equal((await googleSignInEnv({ path: config, windows, release: true })).WRITER_ACCOUNT_SERVICE_URL, 'https://cleanup.example.test');
    }
    for (const accountServiceUrl of ['http://localhost:8787', 'https://localhost', 'https://127.0.0.1']) {
      await writeFile(config, JSON.stringify({ ...values, accountServiceUrl }));
      await assert.rejects(googleSignInEnv({ path: config, release: true }), /public HTTPS/);
    }
    for (const accountServiceUrl of ['http://cleanup.example.test', 'https://user@cleanup.example.test', 'not a url']) {
      await writeFile(config, JSON.stringify({ ...values, accountServiceUrl }));
      await assert.rejects(googleSignInEnv({ path: config, windows: false, release: false }));
    }
    await writeFile(config, JSON.stringify({ ...values, apiKey: 'REPLACE_LOCALLY' }));
    await assert.rejects(googleSignInEnv({ path: config, windows: false, release: false }), (error) => !error.message.includes('synthetic'));
  } finally { await rm(root, { recursive: true }); }
});
