import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, symlink, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { verifyRuntime, stageRuntime, sha256 } from './runtime.mjs';
test('runtime pins reject tampering and stage only approved files without a backend manifest', async () => {
  const root = await mkdtemp(join(tmpdir(), 'writer-runtime-test-'));
  try {
    const source = join(root, 'source'), output = join(root, 'output');
    await mkdir(join(source, 'runtime'), { recursive: true });
    const bytes = Buffer.from('approved fixture');
    const lock = { version: 1, files: [{ path: 'runtime/test.exe', bytes: bytes.length, sha256: sha256(bytes), role: 'runtime', executable: true }] };
    await writeFile(join(source, 'runtime/test.exe'), bytes);
    await writeFile(join(source, 'runtime/unapproved.dll'), 'excluded');
    await stageRuntime(source, output, lock);
    if (process.platform !== 'win32') assert.equal((await stat(join(output, 'runtime/test.exe'))).mode & 0o777, 0o755);
    assert.deepEqual(await readFile(join(output, 'runtime/test.exe')), bytes);
    await assert.rejects(readFile(join(output, 'runtime/unapproved.dll')), /ENOENT/);
    await writeFile(join(source, 'runtime/test.exe'), 'altered fixture!');
    await assert.rejects(verifyRuntime(source, lock), /Pinned runtime mismatch/);
    await rm(join(source, 'runtime/test.exe'));
    await assert.rejects(verifyRuntime(source, lock), /Missing or unsafe/);
    await writeFile(join(root, 'outside'), bytes);
    await symlink(join(root, 'outside'), join(source, 'runtime/test.exe'));
    await assert.rejects(verifyRuntime(source, lock), /escapes/);
    await assert.rejects(verifyRuntime(source, { ...lock, files: [{ ...lock.files[0], path: 'runtime/../outside' }] }), /Invalid runtime lock path/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('installer rejects a missing host prerequisite before creating build output', async () => {
  const { spawnSync } = await import('node:child_process');
  const { fileURLToPath } = await import('node:url');
  const env = { ...process.env };
  delete env.WRITER_LLVM_ROOT;
  const result = spawnSync(process.execPath, [fileURLToPath(new URL('./installer.mjs', import.meta.url)), '--resources', '/missing-runtime-fixture', '--check'], { env, encoding: 'utf8' });
  assert.equal(result.status, 1);
  assert.match(result.stderr, process.platform === 'darwin' ? /ENOENT|Apple Silicon/ : process.platform === 'linux' && process.arch === 'x64' ? /Set WRITER_LLVM_ROOT/ : /supports native macOS or Linux\/WSL x64/);
  assert.doesNotMatch(result.stdout, /Preflight passed|Compiling/);
});
