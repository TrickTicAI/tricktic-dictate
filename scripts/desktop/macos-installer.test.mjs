import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, symlink, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { inventory } from './macos-installer.mjs';
import { identityHash, developerId } from './macos-signing.mjs';

test('signing requires a unique valid named identity, never ad-hoc or another certificate', () => {
  const hash = 'A'.repeat(40), identity = `  1) ${hash} "TrickTic Dictate Dev"\n     1 valid identities found\n`;
  assert.equal(identityHash(identity), hash);
  for (const output of ['', '0 valid identities found', identity.replace('TrickTic Dictate Dev', 'Someone Else'), identity + identity, identity.replace('"\n', '" (CSSMERR_TP_CERT_EXPIRED)\n')]) {
    assert.throws(() => identityHash(output), /Expected exactly one/);
  }
});
test('Developer ID selection requires that exact certificate and leaves the development identity separate', () => {
  const dev = 'A'.repeat(40), release = 'B'.repeat(40);
  const both = `  1) ${dev} "TrickTic Dictate Dev"\n  2) ${release} "${developerId.identity}"\n     2 valid identities found\n`;
  assert.equal(identityHash(both), dev);
  assert.equal(identityHash(both, developerId.identity), release);
  assert.throws(() => identityHash(both.replace(`(${developerId.teamId})`, '(XXXXXXXXXX)'), developerId.identity), /Expected exactly one/);
  assert.throws(() => identityHash(both.replace(`${release} "${developerId.identity}"`, `${release} "${developerId.identity}" (CSSMERR_TP_CERT_REVOKED)`), developerId.identity), /Expected exactly one/);
});
test('app inventory detects content and permission changes and records symlinks without following them', async () => {
  const root = await mkdtemp(join(tmpdir(), 'writer-inventory-'));
  try {
    await mkdir(join(root, 'Contents'));
    await writeFile(join(root, 'Contents/app'), 'binary', { mode: 0o755 });
    await symlink('/Applications', join(root, 'Applications'));
    const before = await inventory(root);
    assert.equal(before[0].symlink, '/Applications');
    assert.equal(before[1].mode, 0o755);
    assert.deepEqual(before, await inventory(root));
    await writeFile(join(root, 'Contents/app'), 'changed binary');
    assert.notEqual(before[1].sha256, (await inventory(root))[1].sha256);
  } finally { await rm(root, { recursive: true, force: true }); }
});
