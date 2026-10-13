import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, mkdir, rm, readFile, realpath } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { generateKeyPairSync, randomBytes, createHash, sign } from 'node:crypto';
import { assertReleaseSource, releaseKey, releaseArtifacts, archiveRelease } from './macos-release.mjs';

function signed(bytes) {
  const keys = generateKeyPairSync('ed25519'), id = randomBytes(8);
  const pub = Buffer.concat([Buffer.from('Ed'), id, keys.publicKey.export({ format: 'der', type: 'spki' }).subarray(-32)]);
  const raw = sign(null, createHash('blake2b512').update(bytes).digest(), keys.privateKey);
  const sig = Buffer.concat([Buffer.from('ED'), id, raw]);
  const comment = 'timestamp:1';
  const global = sign(null, Buffer.concat([raw, Buffer.from(comment)]), keys.privateKey);
  return {
    publicKey: Buffer.from(`untrusted comment: fixture\n${pub.toString('base64')}\n`).toString('base64'),
    signature: Buffer.from(`untrusted comment: fixture\n${sig.toString('base64')}\ntrusted comment: ${comment}\n${global.toString('base64')}\n`).toString('base64'),
  };
}
test('release receipts reject dirty and changed source snapshots', () => {
  const before = { status: '', commit: 'a'.repeat(40), sourceSha256: 'before' };
  assertReleaseSource(before, { ...before });
  assert.throws(() => assertReleaseSource({ ...before, status: ' M source' }), /clean committed/);
  for (const field of ['status', 'commit', 'sourceSha256']) assert.throws(() => assertReleaseSource(before, { ...before, [field]: 'changed' }), /Source changed/);
});
test('only an explicit external signing key with the installed public half is accepted', async () => {
  const root = await mkdtemp(join(tmpdir(), 'writer-key-gate-'));
  try {
    const repo = join(root, 'repo'), key = join(root, 'writer.key'); await mkdir(repo);
    await writeFile(key, 'fixture, never a live key'); await writeFile(`${key}.pub`, 'expected');
    await assert.rejects(releaseKey(repo, {}, 'expected'), /WRITER_SIGNING_KEY_FILE/);
    await assert.rejects(releaseKey(repo, { WRITER_SIGNING_KEY_FILE: key }, 'wrong'), /public half/);
    assert.equal(await releaseKey(repo, { WRITER_SIGNING_KEY_FILE: key }, 'expected'), await realpath(key));
    await writeFile(join(repo, 'writer.key'), 'fixture');
    await assert.rejects(releaseKey(repo, { WRITER_SIGNING_KEY_FILE: join(repo, 'writer.key') }, 'expected'), /outside/);
  } finally { await rm(root, { recursive: true, force: true }); }
});
test('archive signatures reject tampering and wrong keys; retained releases never overwrite', async () => {
  const root = await mkdtemp(join(tmpdir(), 'writer-release-'));
  try {
    const archive = join(root, 'Writer.app.tar.gz'), bytes = Buffer.from('signed archive fixture');
    const fixture = signed(bytes);
    await writeFile(archive, bytes); await writeFile(`${archive}.sig`, fixture.signature);
    const result = await releaseArtifacts(archive, fixture.publicKey);
    assert.equal(result.artifact.bytes, bytes.length);
    await assert.rejects(releaseArtifacts(archive, signed(bytes).publicKey), /wrong signing key/);
    const options = { version: '0.14.0', commit: 'a'.repeat(40), paths: [archive, `${archive}.sig`], output: join(root, 'retained') };
    const first = await archiveRelease(options), second = await archiveRelease(options);
    assert.notEqual(first[0], second[0]); assert.deepEqual(await readFile(first[0]), bytes);
    await writeFile(archive, 'tampered archive');
    await assert.rejects(releaseArtifacts(archive, fixture.publicKey), /verification failed/);
    assert.deepEqual(await readFile(first[0]), bytes);
  } finally { await rm(root, { recursive: true, force: true }); }
});
