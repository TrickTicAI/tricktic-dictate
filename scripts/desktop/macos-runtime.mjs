// Sign staged copies before sealing their hashes into the app's runtime manifest.
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { sha256, verifyRuntime } from './runtime.mjs';
import { identityHash, signingIdentity } from './macos-signing.mjs';
const execute = promisify(execFile);
export async function signMacRuntime(root, lock, run = execute, identity = signingIdentity) {
  await verifyRuntime(root, lock);
  const { stdout } = await run('/usr/bin/security', ['find-identity', '-v', '-p', 'codesigning']);
  const certificate = identityHash(stdout, identity);
  // Notarization requires every nested Mach-O to carry the Hardened Runtime and
  // a secure timestamp. Development helpers keep their previous untimestamped form.
  const options = identity === signingIdentity ? ['--timestamp=none'] : ['--options', 'runtime', '--timestamp'];
  const files = [];
  for (const entry of lock.files) {
    const path = join(root, entry.path);
    if (entry.role === 'runtime') {
      await run('/usr/bin/codesign', ['--force', '--sign', certificate, ...options, path]);
      await run('/usr/bin/codesign', ['--verify', '--strict', '-R', `=certificate leaf = H"${certificate}"`, path]);
    }
    const bytes = await readFile(path);
    files.push({ path: entry.path, bytes: bytes.length, sha256: sha256(bytes) });
  }
  return { files, identity, certificateSha1: certificate };
}
