import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, rm, readFile, copyFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { signingPolicy, createWindowsSigner, windowsSignCommand, signWindowsRuntime, windowsReleaseType, windowsReleaseBanner, requirePublicReleaseSource } from './windows-signing.mjs';
import { verifyRuntime } from './runtime.mjs';

const env = {
  WRITER_AUTHENTICODE_THUMBPRINT: 'A'.repeat(40),
  WRITER_AUTHENTICODE_PUBLISHER: 'CN=Fixture Publisher, O=Fixture',
  WRITER_AUTHENTICODE_TIMESTAMP_URL: 'https://timestamp.example.test',
  WRITER_SIGNTOOL_EXE: 'C:\\Program Files (x86)\\Windows Kits\\signtool.exe',
};
const hash = bytes => createHash('sha256').update(bytes).digest('hex');

test('public release policy requires exact identity, SDK path and timestamp service', () => {
  for (const name of Object.keys(env)) {
    const missing = { ...env }; delete missing[name];
    assert.throws(() => signingPolicy(missing), new RegExp(name));
  }
  assert.throws(() => signingPolicy({ ...env, WRITER_AUTHENTICODE_THUMBPRINT: 'auto' }), /exact 40/);
  for (const url of ['http://timestamp.example.test', 'https://user:pass@example.test', 'https://example.test?token=x', 'https://example.test/#x']) {
    assert.throws(() => signingPolicy({ ...env, WRITER_AUTHENTICODE_TIMESTAMP_URL: url }), /HTTPS/);
  }
  assert.throws(() => signingPolicy({ ...env, WRITER_SIGNTOOL_EXE: 'signtool.exe' }), /absolute/);
  assert.equal(signingPolicy(env).publisher, env.WRITER_AUTHENTICODE_PUBLISHER);
});

test('bridge preserves paths and identity as individual arguments and validates returned evidence', async () => {
  const root = await mkdtemp(join(tmpdir(), 'writer-signing-'));
  try {
    const artifact = join(root, 'Writer setup.exe');
    const bytes = Buffer.from('signed PE fixture');
    await writeFile(artifact, bytes);
    const proof = { status: 'Valid', publisher: env.WRITER_AUTHENTICODE_PUBLISHER, thumbprint: env.WRITER_AUTHENTICODE_THUMBPRINT, timestampThumbprint: 'B'.repeat(40), sha256: hash(bytes) };
    let response = proof;
    const calls = [];
    const execute = (command, args) => {
      calls.push({ command, args });
      return command === 'wslpath' ? `Z:${args[1]}` : JSON.stringify(response);
    };
    const signer = createWindowsSigner({ env, platform: 'linux', execute });
    await signer.preflight();
    assert.deepEqual(await signer.sign(artifact), proof);
    assert.deepEqual(await signer.verify(artifact), proof);
    const call = calls.at(-1);
    assert.equal(call.args[call.args.indexOf('-Artifact') + 1], `Z:${artifact}`);
    assert.equal(call.args[call.args.indexOf('-Publisher') + 1], env.WRITER_AUTHENTICODE_PUBLISHER);
    assert.equal(call.args[call.args.indexOf('-SignTool') + 1], env.WRITER_SIGNTOOL_EXE);
    for (const patch of [{ publisher: 'CN=Other' }, { thumbprint: 'C'.repeat(40) }, { status: 'NotTrusted' }, { timestampThumbprint: null }, { sha256: '0'.repeat(64) }]) {
      response = { ...proof, ...patch };
      await assert.rejects(signer.verify(artifact));
    }
    response = proof;
    await writeFile(artifact, 'tampered after native verification');
    await assert.rejects(signer.verify(artifact), /changed after/);
    assert.throws(() => createWindowsSigner({ env, platform: 'darwin', execute }), /Windows or WSL/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('Windows process failure and malformed evidence fail closed', async () => {
  for (const output of ['', 'not JSON', '{}']) {
    const signer = createWindowsSigner({ env, platform: 'win32', execute: () => output });
    await assert.rejects(signer.preflight());
  }
  const signer = createWindowsSigner({ env, platform: 'win32', execute: () => { throw Error('native verifier failed'); } });
  await assert.rejects(signer.preflight(), /native verifier/);
  const command = windowsSignCommand();
  assert.equal(command.cmd, process.execPath);
  assert.deepEqual(command.args.slice(-2), ['sign', '%1']);
});

test('runtime manifest seals signed bytes and rejects later bundler mutations', async () => {
  const root = await mkdtemp(join(tmpdir(), 'writer-signed-runtime-'));
  try {
    await mkdir(join(root, 'runtime')); await mkdir(join(root, 'notices'));
    const bytes = Buffer.from('pinned fixture');
    const entries = ['runtime/helper.exe', 'runtime/helper.dll', 'notices/license.txt'].map(path => ({ path, bytes: bytes.length, sha256: hash(bytes) }));
    for (const entry of entries) await writeFile(join(root, entry.path), bytes);
    const calls = [];
    const signer = { sign: async path => {
      calls.push(path);
      const signed = Buffer.concat([await readFile(path), Buffer.from('signature fixture')]);
      await writeFile(path, signed);
      return { sha256: hash(signed) };
    } };
    const signed = await signWindowsRuntime(root, entries, signer);
    assert.equal(calls.length, 2);
    assert.deepEqual(signed[2], entries[2]);
    assert.notEqual(signed[0].sha256, entries[0].sha256);
    await verifyRuntime(root, { version: 1, files: signed });
    await writeFile(join(root, signed[0].path), 'later mutation');
    await assert.rejects(verifyRuntime(root, { version: 1, files: signed }), /Pinned runtime mismatch/);
    await assert.rejects(signWindowsRuntime(root, signed, signer), /Pinned runtime mismatch/);
    assert.equal(calls.length, 2, 'reject corrupted inputs before invoking signer');
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('both packaging entry points reject an unconfigured public Windows release before building', () => {
  if (process.platform !== 'linux' || process.arch !== 'x64') return;
  const missing = { ...process.env };
  for (const name of Object.keys(env)) delete missing[name];
  for (const [script, args] of [['installer.mjs', ['--check', '--release']], ['build.mjs', ['--target', 'x86_64-pc-windows-msvc', '--release']]]) {
    const result = spawnSync(process.execPath, [fileURLToPath(new URL(script, import.meta.url)), ...args], { env: missing, encoding: 'utf8' });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /WRITER_AUTHENTICODE_THUMBPRINT|WRITER_GOOGLE_SIGN_IN_CONFIG/);
    assert.doesNotMatch(result.stdout, /Preflight passed|Compiling|Receipt:/);
  }
});

test('developer previews keep the existing unelevated installation configuration', async () => {
  const config = JSON.parse(await readFile(new URL('../../apps/desktop/tauri/tauri.conf.json', import.meta.url)));
  assert.equal(config.bundle.windows.nsis.installMode, 'currentUser');
  assert.equal(config.bundle.windows.signCommand, undefined);
});

test('public release without Authenticode is a named mode, never a developer preview', () => {
  assert.equal(windowsReleaseType({ release: false, withoutAuthenticode: false }), 'unsigned-developer-preview');
  assert.equal(windowsReleaseType({ release: true, withoutAuthenticode: false }), 'public-windows');
  assert.equal(windowsReleaseType({ release: true, withoutAuthenticode: true }), 'public-windows-without-authenticode');
  assert.throws(() => windowsReleaseType({ release: false, withoutAuthenticode: true }), /only to --release/);
  assert.match(windowsReleaseBanner('unsigned-developer-preview'), /not a public Windows release/);
  assert.match(windowsReleaseBanner('public-windows-without-authenticode'), /WITHOUT AUTHENTICODE: intentionally omitted/);
});

test('public release source gate requires the minisign key pair and a clean tree', async () => {
  const root = await mkdtemp(join(tmpdir(), 'writer-release-gate-'));
  try {
    const keyFile = join(root, 'writer.key');
    const clean = { keyFile, updaterPublicKey: 'fixture-public-key', gitStatus: () => '' };
    await assert.rejects(requirePublicReleaseSource({ ...clean, keyFile: undefined }), /Set WRITER_SIGNING_KEY_FILE/);
    await assert.rejects(requirePublicReleaseSource(clean), /Restore the protected Writer signing key/);
    await writeFile(keyFile, 'private fixture');
    await assert.rejects(requirePublicReleaseSource(clean), /writer\.key\.pub/);
    await writeFile(`${keyFile}.pub`, 'other-public-key\n');
    await assert.rejects(requirePublicReleaseSource(clean), /does not match/);
    await writeFile(`${keyFile}.pub`, 'fixture-public-key\n');
    await assert.rejects(requirePublicReleaseSource({ ...clean, gitStatus: () => ' M apps/web/src/main.tsx' }), /clean committed checkout/);
    await requirePublicReleaseSource(clean);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('installer refuses a dirty tree or missing key without Authenticode and labels previews truthfully', async () => {
  if (process.platform !== 'linux' || process.arch !== 'x64') return;
  const root = await mkdtemp(join(tmpdir(), 'writer-release-mode-'));
  try {
    // Copy the installer entry point into a scratch Git repository so tree state is controlled.
    await mkdir(join(root, 'scripts/desktop'), { recursive: true });
    await mkdir(join(root, 'apps/desktop/tauri'), { recursive: true });
    for (const file of ['installer.mjs', 'windows-signing.mjs', 'runtime.mjs', 'signature.mjs', 'update-metadata.mjs', 'notices.mjs', 'security-artifact.mjs']) await copyFile(fileURLToPath(new URL(file, import.meta.url)), join(root, 'scripts/desktop', file));
    await writeFile(join(root, 'apps/desktop/tauri/tauri.conf.json'), JSON.stringify({ plugins: { updater: { pubkey: 'fixture-public-key' } } }));
    const git = (...args) => spawnSync('git', ['-C', root, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test', ...args], { encoding: 'utf8' });
    git('init', '-q'); git('add', '.'); git('commit', '-qm', 'fixture');
    const keyDir = await mkdtemp(join(tmpdir(), 'writer-release-key-'));
    const keyFile = join(keyDir, 'writer.key');
    await writeFile(keyFile, 'private fixture'); await writeFile(`${keyFile}.pub`, 'fixture-public-key\n');
    const signInConfig = join(keyDir, 'account.json');
    await writeFile(signInConfig, JSON.stringify({ projectId: 'writer-fixture', apiKey: 'synthetic-key', desktopClientId: 'synthetic.apps.googleusercontent.com', accountServiceUrl: 'https://cleanup.example.test' }));
    const base = { ...process.env, WRITER_GOOGLE_SIGN_IN_CONFIG: signInConfig };
    for (const name of [...Object.keys(env), 'WRITER_SIGNING_KEY_FILE']) delete base[name];
    const installer = (args, extra = {}) => spawnSync(process.execPath, [join(root, 'scripts/desktop/installer.mjs'), '--check', ...args], { env: { ...base, ...extra }, encoding: 'utf8' });
    const publicMode = ['--release', '--without-authenticode'];

    let result = installer(publicMode);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Set WRITER_SIGNING_KEY_FILE/);
    assert.doesNotMatch(result.stderr, /WRITER_AUTHENTICODE/);
    result = installer(publicMode, { WRITER_SIGNING_KEY_FILE: join(keyDir, 'absent.key') });
    assert.match(result.stderr, /Restore the protected Writer signing key/);

    await writeFile(join(root, 'uncommitted.txt'), 'dirty');
    result = installer(publicMode, { WRITER_SIGNING_KEY_FILE: keyFile });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /clean committed checkout/);
    await rm(join(root, 'uncommitted.txt'));

    // A clean tree with the key passes the release gate and stops at the next prerequisite.
    result = installer(publicMode, { WRITER_SIGNING_KEY_FILE: keyFile });
    assert.match(result.stdout, /PUBLIC WINDOWS RELEASE WITHOUT AUTHENTICODE/);
    assert.match(result.stderr, /Obtain the retained Writer runtime package/);

    result = installer(['--without-authenticode'], { WRITER_SIGNING_KEY_FILE: keyFile });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /only to --release/);
    assert.doesNotMatch(result.stdout, /PUBLIC/);
    result = installer([]);
    assert.match(result.stdout, /UNSIGNED DEVELOPER PREVIEW: not a public Windows release/);
    assert.doesNotMatch(result.stdout, /PUBLIC/);
    await rm(keyDir, { recursive: true, force: true });
  } finally { await rm(root, { recursive: true, force: true }); }
});
