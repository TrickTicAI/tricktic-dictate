// Pin verification alone cannot prove the worker speaks the app's protocol.
import { readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { sha256 } from './runtime.mjs';

export async function verifySpeechProtocol(root, lock, authHeader) {
  const expected = sha256(await readFile(authHeader));
  for (const family of ['whisper', 'nemo']) {
    const receiptPath = `notices/writer-${family}-source.txt`;
    if (!lock.files.some(entry => entry.path === receiptPath)) throw Error(`Missing ${family} private-worker source receipt; rebuild the Mac speech runtimes.`);
    const receipt = await readFile(join(root, receiptPath), 'utf8');
    const key = family === 'whisper' ? 'examples/server/writer-auth.h' : 'writer_auth_sha256';
    if (!receipt.split('\n').includes(`${key}=${expected}`)) throw Error(`The ${family} runtime protocol differs from this app; rebuild the Mac speech runtimes.`);
  }
  for (const path of ['runtime/whisper/whisper-server', 'runtime/whisper/cpu/whisper-server', 'runtime/nemo/metal/bin/nemo-speech', 'runtime/nemo/cpu/bin/nemo-speech']) {
    if (!lock.files.some(entry => entry.path === path)) throw Error(`Missing private speech worker: ${path}`);
    const bytes = await readFile(join(root, path));
    if (!bytes.includes(Buffer.from('writer: private listener')) || !bytes.includes(Buffer.from('X-Writer-Instance'))) throw Error(`Legacy speech worker ${path}; rebuild with the private-worker protocol before packaging.`);
  }
  const pin = JSON.parse(await readFile(join(dirname(authHeader), 'httplib-source.json'), 'utf8'));
  const limits = sha256(await readFile(join(dirname(authHeader), 'writer-http-limits.h')));
  const source = await readFile(join(root, 'notices/writer-whisper-source.txt'), 'utf8');
  for (const entry of [`httplib=${pin.revision}`, `examples/server/httplib.h=${pin.sha256}`,
    `examples/server/writer-http-limits.h=${limits}`]) {
    if (!source.split('\n').includes(entry)) throw Error('Stale Whisper pre-authentication parser/limits receipt; rebuild both Mac Whisper backends.');
  }
  for (const backend of ['metal', 'cpu']) {
    const receiptPath = `notices/whisper-${backend}-build.json`;
    if (!lock.files.some(entry => entry.path === receiptPath)) throw Error(`Missing ${backend} Whisper build receipt.`);
    const build = JSON.parse(await readFile(join(root, receiptPath), 'utf8'));
    if (build.backend !== backend || build.source !== source) throw Error(`Whisper ${backend} build/source receipt mismatch.`);
    const prefix = `runtime/whisper/${backend === 'cpu' ? 'cpu/' : ''}`;
    const entries = lock.files.filter(entry => entry.path.startsWith(prefix) && !entry.path.slice(prefix.length).includes('/'));
    if (JSON.stringify(entries.map(entry => entry.path.slice(prefix.length)).sort()) !== JSON.stringify(Object.keys(build.files).sort())) throw Error(`Whisper ${backend} build file inventory mismatch.`);
    for (const entry of entries) {
      if (build.files[entry.path.slice(prefix.length)] !== entry.sha256) throw Error(`Whisper ${backend} input differs from its build receipt.`);
    }
    const server = await readFile(join(root, prefix, 'whisper-server'));
    if (!server.includes(Buffer.from('writer: pre-auth limits line='))) throw Error(`Whisper ${backend} lacks the pre-authentication limits marker.`);
  }
}
