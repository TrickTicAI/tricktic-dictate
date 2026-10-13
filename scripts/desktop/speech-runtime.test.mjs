import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { sha256 } from './runtime.mjs';
import { verifySpeechProtocol } from './speech-runtime.mjs';

test('rejects legacy pinned workers and stale protocol receipts before signing', async () => {
  const root = await mkdtemp(join(tmpdir(), 'writer-protocol-'));
  const paths = ['runtime/whisper/whisper-server', 'runtime/whisper/cpu/whisper-server', 'runtime/nemo/metal/bin/nemo-speech', 'runtime/nemo/cpu/bin/nemo-speech'];
  const header = join(root, 'auth.h');
  const pin = {revision: 'reviewed-revision', sha256: sha256('parser')};
  const limits = Buffer.from('reviewed limits');
  const bytes = Buffer.from('current protocol');
  const lock = {files: [...paths, 'notices/writer-whisper-source.txt', 'notices/writer-nemo-source.txt'].map(path => ({path}))};
  try {
    await writeFile(header, bytes);
    await writeFile(join(root, 'httplib-source.json'), JSON.stringify(pin));
    await writeFile(join(root, 'writer-http-limits.h'), limits);
    for (const {path} of lock.files) {
      await mkdir(join(root, path, '..'), {recursive: true});
      await writeFile(join(root, path), path.startsWith('notices/') ? `${path.includes('whisper') ? 'examples/server/writer-auth.h' : 'writer_auth_sha256'}=${sha256(bytes)}\n` : 'writer: private listener\0X-Writer-Instance\0writer: pre-auth limits line=');
    }
    const source = `examples/server/writer-auth.h=${sha256(bytes)}\nhttplib=${pin.revision}\nexamples/server/httplib.h=${pin.sha256}\nexamples/server/writer-http-limits.h=${sha256(limits)}\n`;
    await writeFile(join(root, 'notices/writer-whisper-source.txt'), source);
    for (const backend of ['metal', 'cpu']) {
      const path = `notices/whisper-${backend}-build.json`;
      const binary = paths[backend === 'cpu' ? 1 : 0];
      const hash = sha256('writer: private listener\0X-Writer-Instance\0writer: pre-auth limits line=');
      lock.files.find(entry => entry.path === binary).sha256 = hash;
      lock.files.push({path});
      await writeFile(join(root, path), JSON.stringify({backend, source, files: {'whisper-server': hash}}));
    }
    await verifySpeechProtocol(root, lock, header);
    await writeFile(join(root, 'notices/writer-whisper-source.txt'), source.replace(pin.sha256, 'old-parser'));
    await assert.rejects(verifySpeechProtocol(root, lock, header), /Stale Whisper pre-authentication/);
    await writeFile(join(root, 'notices/writer-whisper-source.txt'), source);
    const buildPath = join(root, 'notices/whisper-cpu-build.json');
    const validBuild = await readFile(buildPath, 'utf8');
    await writeFile(buildPath, validBuild.replace('"cpu"', '"metal"'));
    await assert.rejects(verifySpeechProtocol(root, lock, header), /build\/source receipt mismatch/);
    await writeFile(buildPath, validBuild);
    lock.files.find(entry => entry.path === paths[1]).sha256 = 'changed';
    await assert.rejects(verifySpeechProtocol(root, lock, header), /input differs/);
    lock.files.find(entry => entry.path === paths[1]).sha256 = JSON.parse(validBuild).files['whisper-server'];
    await writeFile(join(root, paths[1]), 'old binary with writer_protocol warmup');
    await assert.rejects(verifySpeechProtocol(root, lock, header), /Legacy speech worker/);
    await writeFile(join(root, paths[1]), 'writer: private listener\0X-Writer-Instance\0writer: pre-auth limits line=');
    await writeFile(header, 'changed protocol');
    await assert.rejects(verifySpeechProtocol(root, lock, header), /protocol differs/);
    await assert.rejects(verifySpeechProtocol(root, {files: []}, header), /source receipt/);
  } finally { await rm(root, {recursive:true, force:true}); }
});
