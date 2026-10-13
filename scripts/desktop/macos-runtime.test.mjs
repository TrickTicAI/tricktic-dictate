import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { stageRuntime, sha256, verifyRuntime } from './runtime.mjs';
import { signMacRuntime } from './macos-runtime.mjs';
import { developerId } from './macos-signing.mjs';

test('manifest seals signed bytes while preserving approved input pins and notices', async () => {
  const root = await mkdtemp(join(tmpdir(), 'writer-mac-signing-'));
  try {
    const source = join(root, 'source'), staged = join(root, 'staged');
    const files = [];
    for (const [path, role] of [['runtime/worker', 'runtime'], ['notices/license.txt', 'notice']]) {
      const bytes = Buffer.from(path);
      await mkdir(join(source, path, '..'), { recursive: true });
      await writeFile(join(source, path), bytes);
      files.push({ path, role, bytes: bytes.length, sha256: sha256(bytes) });
    }
    const lock = { version: 1, files };
    await stageRuntime(source, staged, lock);
    const certificate = 'A'.repeat(40), calls = [];
    const run = async (command, args) => {
      calls.push([command, args]);
      if (command.endsWith('/security')) return { stdout: `  1) ${certificate} "TrickTic Dictate Dev"\n` };
      if (args.includes('--sign')) await writeFile(args.at(-1), 'signed bytes');
      return { stdout: '' };
    };
    const signed = await signMacRuntime(staged, lock, run);
    assert.equal(signed.certificateSha1, certificate);
    assert.equal(calls.length, 3);
    assert.ok(calls[2][1].includes(`=certificate leaf = H"${certificate}"`));
    assert.notEqual(signed.files[0].sha256, files[0].sha256);
    assert.equal(signed.files[1].sha256, files[1].sha256);
    await verifyRuntime(source, lock);
    await verifyRuntime(staged, { version: 1, files: signed.files });
    assert.equal((await readFile(join(staged, 'runtime/worker'))).toString(), 'signed bytes');
    assert.ok(calls[1][1].includes('--timestamp=none') && !calls[1][1].includes('runtime'), 'development helpers keep their previous form');
    await stageRuntime(source, staged, lock);
    calls.length = 0;
    const release = await signMacRuntime(staged, lock, async (command, args) => {
      calls.push([command, args]);
      if (command.endsWith('/security')) return { stdout: `  1) ${certificate} "TrickTic Dictate Dev"\n  2) ${'B'.repeat(40)} "${developerId.identity}"\n` };
      if (args.includes('--sign')) await writeFile(args.at(-1), 'notarizable bytes');
      return { stdout: '' };
    }, developerId.identity);
    assert.equal(release.certificateSha1, 'B'.repeat(40));
    assert.equal(release.identity, developerId.identity);
    assert.deepEqual(calls[1][1].slice(0, 6), ['--force', '--sign', 'B'.repeat(40), '--options', 'runtime', '--timestamp']);
    let invoked = false;
    await assert.rejects(signMacRuntime(staged, lock, async () => { invoked = true; }), /Pinned runtime mismatch/);
    assert.equal(invoked, false, 'reject tampering before accessing the signing identity');
  } finally { await rm(root, { recursive: true, force: true }); }
});
