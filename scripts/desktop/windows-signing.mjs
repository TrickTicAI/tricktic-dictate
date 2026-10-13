// Authenticode uses a protected Windows certificate-store key, never a PFX/password.
import { spawnSync } from 'node:child_process';
import { readFile, access } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { verifyRuntime } from './runtime.mjs';

const bridge = fileURLToPath(new URL('./windows-signing.ps1', import.meta.url));

// Publishing without Authenticode is a named mode, never a fallback (decision D7).
export const WITHOUT_AUTHENTICODE = '--without-authenticode';
export function windowsReleaseType({ release, withoutAuthenticode }) {
  if (withoutAuthenticode && !release) throw Error(`${WITHOUT_AUTHENTICODE} applies only to --release; developer previews are never public releases.`);
  return !release ? 'unsigned-developer-preview' : withoutAuthenticode ? 'public-windows-without-authenticode' : 'public-windows';
}
export function windowsReleaseBanner(type) {
  return {
    'unsigned-developer-preview': 'UNSIGNED DEVELOPER PREVIEW: not a public Windows release.',
    'public-windows-without-authenticode': 'PUBLIC WINDOWS RELEASE WITHOUT AUTHENTICODE: intentionally omitted; SmartScreen will show an unknown publisher. See docs/security/windows-signing.md.',
    'public-windows': 'PUBLIC WINDOWS RELEASE: Authenticode publisher signing required.',
  }[type];
}

// Both public modes need the minisign key and a clean committed checkout.
export async function requirePublicReleaseSource({ keyFile, updaterPublicKey, gitStatus }) {
  if (!keyFile) throw Error('Set WRITER_SIGNING_KEY_FILE to the private Writer update signing key.');
  try { await access(keyFile); } catch { throw Error(`Missing ${keyFile}. Restore the protected Writer signing key from your backup.`); }
  let publicKey;
  try { publicKey = (await readFile(`${keyFile}.pub`, 'utf8')).trim(); } catch { throw Error(`Missing ${keyFile}.pub. Restore it with the private key.`); }
  if (publicKey !== updaterPublicKey) throw Error('Signing key public half does not match the installed updater trust key.');
  if (gitStatus()) throw Error('Release builds require a clean committed checkout.');
}

export function signingPolicy(env = process.env) {
  const required = name => {
    const value = env[name]?.trim();
    if (!value || /[\r\n\0]/.test(value)) throw Error(`Set ${name} for public Windows releases. See docs/security/windows-signing.md.`);
    return value;
  };
  const thumbprint = required('WRITER_AUTHENTICODE_THUMBPRINT').toUpperCase();
  if (!/^[A-F0-9]{40}$/.test(thumbprint)) throw Error('WRITER_AUTHENTICODE_THUMBPRINT must be the exact 40-digit certificate thumbprint.');
  const publisher = required('WRITER_AUTHENTICODE_PUBLISHER');
  const timestampUrl = required('WRITER_AUTHENTICODE_TIMESTAMP_URL');
  const url = new URL(timestampUrl);
  if (url.protocol !== 'https:' || url.username || url.password || url.search || url.hash) throw Error('Authenticode timestamp URL must be HTTPS without credentials, query or fragment.');
  const signtool = required('WRITER_SIGNTOOL_EXE');
  if (!/^[A-Za-z]:\\.*\\signtool\.exe$/i.test(signtool)) throw Error('WRITER_SIGNTOOL_EXE must be an absolute Windows SDK signtool.exe path.');
  return { thumbprint, publisher, timestampUrl, signtool };
}

function run(command, args) {
  const result = spawnSync(command, args, { encoding: 'utf8', timeout: 180000, windowsHide: true, maxBuffer: 1024 * 1024 });
  // Do not echo inherited environment, command lines or signing-provider diagnostics.
  if (result.error || result.status !== 0) throw Error(`Authenticode ${command === 'wslpath' ? 'path conversion' : 'Windows verification/signing'} failed; check Windows SDK, certificate access, trust and timestamp service (exit ${result.status ?? 'unavailable'}).`);
  return result.stdout.trim();
}

export function createWindowsSigner({ env = process.env, platform = process.platform, execute = run } = {}) {
  const policy = signingPolicy(env);
  if (!['linux', 'win32'].includes(platform)) throw Error('Windows Authenticode requires Windows or WSL with Windows interop.');
  const windowsPath = path => platform === 'win32' ? resolve(path) : execute('wslpath', ['-w', resolve(path)]);
  const powershell = env.WRITER_POWERSHELL_EXE || 'powershell.exe';
  const script = windowsPath(bridge);
  async function invoke(action, path) {
    const args = ['-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', script,
      '-Action', action, '-Thumbprint', policy.thumbprint, '-Publisher', policy.publisher,
      '-TimestampUrl', policy.timestampUrl, '-SignTool', policy.signtool];
    if (path) args.push('-Artifact', windowsPath(path));
    const proof = JSON.parse(execute(powershell, args).replace(/^\uFEFF/, ''));
    if (proof.publisher !== policy.publisher || proof.thumbprint !== policy.thumbprint || proof.status !== 'Valid') throw Error('Authenticode publisher/certificate verification failed.');
    if (action !== 'preflight') {
      if (!/^[A-F0-9]{40}$/.test(proof.timestampThumbprint ?? '')) throw Error('Authenticode trusted timestamp is missing.');
      const sha256 = createHash('sha256').update(await readFile(path)).digest('hex');
      if (proof.sha256 !== sha256) throw Error('Artifact changed after Windows Authenticode verification.');
    }
    return proof;
  }
  return { policy, preflight: () => invoke('preflight'), sign: path => invoke('sign', path), verify: path => invoke('verify', path) };
}

export function windowsSignCommand() {
  return { cmd: process.execPath, args: [fileURLToPath(import.meta.url), 'sign', '%1'] };
}

export async function signWindowsRuntime(root, entries, signer) {
  await verifyRuntime(root, { version: 1, files: entries });
  const signed = [];
  for (const entry of entries) {
    const path = resolve(root, entry.path);
    const proof = /\.(exe|dll)$/i.test(entry.path) ? await signer.sign(path) : null;
    const bytes = await readFile(path);
    const sha256 = createHash('sha256').update(bytes).digest('hex');
    if (proof && proof.sha256 !== sha256) throw Error('Runtime changed after Authenticode verification.');
    signed.push({ ...entry, bytes: bytes.length, sha256 });
  }
  return signed;
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try {
    const [action, artifact, ...extra] = process.argv.slice(2);
    if (!['preflight', 'sign', 'verify'].includes(action) || extra.length || (action !== 'preflight' && !artifact) || (action === 'preflight' && artifact)) throw Error('Usage: windows-signing.mjs preflight | sign <artifact> | verify <artifact>');
    const signer = createWindowsSigner();
    console.log(JSON.stringify(await signer[action](artifact)));
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
