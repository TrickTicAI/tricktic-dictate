import { readFile, realpath, mkdir, writeFile, chmod } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { resolve, relative, isAbsolute, dirname, sep } from 'node:path';
export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
export async function verifyRuntime(source, lock) {
  const root = await realpath(source);
  const seen = new Set();
  if (lock.version !== 1 || !Array.isArray(lock.files) || !lock.files.length) throw Error('Invalid runtime lock');
  for (const entry of lock.files) {
    if (!/^(runtime|notices)\/[a-zA-Z0-9._/-]+$/.test(entry.path) || entry.path.split('/').some(p => p === '..' || !p) || seen.has(entry.path)) throw Error('Invalid runtime lock path');
    seen.add(entry.path);
    let bytes;
    try {
      const file = await realpath(resolve(root, entry.path));
      const rel = relative(root, file);
      if (rel === '..' || rel.startsWith(`..${sep}`) || isAbsolute(rel)) throw Error('Path escapes runtime root');
      bytes = await readFile(file);
    } catch (error) { throw Error(`Missing or unsafe runtime input ${entry.path}: ${error.message}`); }
    if (bytes.length !== entry.bytes || sha256(bytes) !== entry.sha256) throw Error(`Pinned runtime mismatch: ${entry.path}. Restore the approved package or deliberately review/update the platform runtime lock.`);
  }
}
export async function stageRuntime(source, destination, lock) {
  await verifyRuntime(source, lock);
  for (const entry of lock.files) {
    // Verify the bytes actually written, even if the source changed after preflight.
    const bytes = await readFile(resolve(source, entry.path));
    if (sha256(bytes) !== entry.sha256 || bytes.length !== entry.bytes) throw Error(`Runtime changed during staging: ${entry.path}`);
    const target = resolve(destination, entry.path);
    await mkdir(dirname(target), { recursive: true });
    await writeFile(target, bytes);
    if (entry.executable === true) await chmod(target, 0o755);
  }
}
