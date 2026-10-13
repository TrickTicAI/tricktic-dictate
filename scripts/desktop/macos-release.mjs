// macOS release gates; the Windows builder and its v1 receipt stay independent.
import { access, readFile, realpath, stat, mkdir, mkdtemp, copyFile } from 'node:fs/promises';
import { join, resolve, relative, basename, sep } from 'node:path';
import { homedir } from 'node:os';
import { sha256 } from './runtime.mjs';
import { verifySignature } from './signature.mjs';

export function assertReleaseSource(before, after = before) {
  if (before.status) throw Error('Release builds require a clean committed checkout.');
  if (JSON.stringify(before) !== JSON.stringify(after)) throw Error('Source changed during release build; no release receipt emitted.');
}
export async function releaseKey(repo, env, publicKey) {
  if (!env.WRITER_SIGNING_KEY_FILE) throw Error('Set WRITER_SIGNING_KEY_FILE to the privately transferred Writer update key; never put it in source or chat.');
  const path = await realpath(env.WRITER_SIGNING_KEY_FILE);
  const local = relative(await realpath(repo), path);
  if (local === '' || (!local.startsWith(`..${sep}`) && local !== '..' && !local.startsWith(sep))) throw Error('The private update key must be outside the source checkout.');
  await access(path);
  if (!(await stat(path)).isFile()) throw Error('The private update key must be a file.');
  if ((await readFile(`${path}.pub`, 'utf8')).trim() !== publicKey) throw Error('Signing key public half does not match the installed updater trust key.');
  return path;
}
export async function releaseArtifacts(archive, publicKey) {
  const bytes = await readFile(archive), signatureBytes = await readFile(`${archive}.sig`);
  verifySignature(bytes, signatureBytes.toString('utf8'), publicKey);
  return {
    artifact: { path: archive, bytes: bytes.length, sha256: sha256(bytes) },
    signature: { path: `${archive}.sig`, bytes: signatureBytes.length, sha256: sha256(signatureBytes) },
  };
}
export async function archiveRelease({ version, commit, paths, output = process.env.WRITER_MACOS_OUTPUT_DIR ?? join(homedir(), 'Metrix-Writer/Releases') }) {
  const root = resolve(output);
  await mkdir(root, { recursive: true });
  // A retained directory independent of the disposable build/worktree; never overwrite a release.
  const destination = await mkdtemp(join(root, `${version}-${commit.slice(0, 12)}-`));
  const result = [];
  for (const path of paths) {
    const retained = join(destination, basename(path));
    await copyFile(path, retained);
    if (sha256(await readFile(path)) !== sha256(await readFile(retained))) throw Error('Retained release copy hash mismatch.');
    result.push(retained);
  }
  return result;
}
