// Signed sidecar consumed by the release publisher (MW-SEC-17 / OPS).
import { readFile, writeFile, unlink } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { verifySignature } from './signature.mjs';

export const MAX_ARTIFACT = 300 * 1024 * 1024;
export const MAX_LIFETIME = 30 * 24 * 60 * 60;
// Every release carries one envelope per app id: installed 0.39-and-earlier
// copies read `writer_release` (WRITER_APP); current clients read
// `dictate_release` (DICTATE_APP). See docs/release/identity.md.
export const WRITER_APP = 'com.metrix.writer';
export const DICTATE_APP = 'com.tricktic.dictate';
export const SIDECAR = { [WRITER_APP]: '.writer-release.json', [DICTATE_APP]: '.dictate-release.json' };
export function manifestFor(bytes, version, platform, now = Math.floor(Date.now() / 1000), app = WRITER_APP) {
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)
      || version.split('.').some(part => BigInt(part) > 18446744073709551615n)
      || !['windows-x86_64', 'darwin-aarch64'].includes(platform)
      || bytes.length === 0 || bytes.length > MAX_ARTIFACT
      || !Number.isSafeInteger(now) || !Number.isSafeInteger(now + MAX_LIFETIME) || now < 0
      || !SIDECAR[app]) throw Error('Invalid Writer release metadata');
  return { schema: 1, app, platform, version,
    sha256: createHash('sha256').update(bytes).digest('hex'), size: bytes.length,
    issued_at: now, expires_at: now + MAX_LIFETIME };
}

export function verifyMetadata(envelope, bytes, version, platform, publicKey, now = Math.floor(Date.now() / 1000), app = WRITER_APP) {
  if (!envelope || Object.keys(envelope).length !== 2
      || typeof envelope?.payload !== 'string' || envelope.payload.length > 16384
      || typeof envelope.signature !== 'string' || envelope.signature.length > 4096) throw Error('Invalid Writer metadata envelope');
  const payload = Buffer.from(envelope.payload, 'base64');
  if (payload.toString('base64') !== envelope.payload
      || Buffer.from(envelope.signature.trim(),'base64').toString('base64') !== envelope.signature.trim()) throw Error('Invalid metadata base64');
  verifySignature(payload, envelope.signature, publicKey);
  const json = new TextDecoder('utf-8', {fatal:true}).decode(payload);
  const manifest = JSON.parse(json);
  // Schema 1 contains only primitive fields. Reject duplicate keys like serde's
  // deny_unknown_fields parser, while retaining the exact signed JSON bytes.
  const fields = [...json.matchAll(/("(?:[^"\\]|\\.)*")\s*:/g)].map(match => JSON.parse(match[1]));
  if (fields.length !== 8 || new Set(fields).size !== 8) throw Error('Invalid Writer metadata fields');
  const expected = manifestFor(bytes, version, platform, manifest.issued_at, app);
  if (!Number.isSafeInteger(manifest.expires_at) || manifest.expires_at <= manifest.issued_at
      || manifest.expires_at > expected.expires_at) throw Error('Invalid Writer metadata expiry');
  expected.expires_at = manifest.expires_at;
  if (Object.keys(manifest).length !== Object.keys(expected).length
      || Object.entries(expected).some(([key, value]) => manifest[key] !== value)
      || manifest.issued_at > now + 300 || manifest.expires_at <= now) throw Error('Writer metadata does not match the fresh release');
  return manifest;
}

// sign(path) must use Tauri signer sign with the same protected key as the
// artifact. It must not print key material or put it in command arguments.
export async function writeReleaseMetadata({ artifact, version, platform, publicKey, sign, now, output, app = WRITER_APP }) {
  const bytes = await readFile(artifact);
  verifySignature(bytes, await readFile(`${artifact}.sig`, 'utf8'), publicKey);
  const path = `${artifact}.writer-manifest.json`;
  const payload = Buffer.from(JSON.stringify(manifestFor(bytes, version, platform, now, app)));
  await writeFile(path, payload, { flag: 'wx', mode: 0o600 });
  try {
    await sign(path);
    const envelope = { payload: payload.toString('base64'), signature: (await readFile(`${path}.sig`, 'utf8')).trim() };
    verifyMetadata(envelope, bytes, version, platform, publicKey, now, app);
    const sidecar = output ?? `${artifact}${SIDECAR[app]}`;
    await writeFile(sidecar, `${JSON.stringify(envelope)}\n`, { flag: 'wx' });
    return sidecar;
  } finally {
    await unlink(path);
    await unlink(`${path}.sig`).catch(error => { if (error.code !== 'ENOENT') throw error; });
  }
}

// Both envelopes of a new release, signed at the same instant.
export async function writeReleaseEnvelopes(options) {
  const now = options.now ?? Math.floor(Date.now() / 1000);
  return { writerRelease: await writeReleaseMetadata({ ...options, now, app: WRITER_APP }),
    dictateRelease: await writeReleaseMetadata({ ...options, now, app: DICTATE_APP }) };
}

// Offline renewal must start with the retained clean receipt and originally
// signed sidecar, never the publisher's editable version/hash claims.
export async function renewReleaseMetadata({artifact, receipt, original, output, publicKey, sign, now = Math.floor(Date.now()/1000)}) {
  const platform = await retainedRelease(artifact, receipt, output);
  const bytes = await readFile(artifact);
  // Verify expired originals at their signed issue time; they remain the identity
  // anchor. Freshness is granted only by the new protected-key signature below.
  // The original names its app; it is renewed for that same app only.
  const { issued_at: issued, app } = JSON.parse(Buffer.from(original.payload, 'base64'));
  const old = verifyMetadata(original, bytes, receipt.appVersion, platform, publicKey, issued, app);
  if (now <= old.issued_at || now + MAX_LIFETIME <= old.expires_at) throw Error('Renewal must advance signed freshness');
  return writeReleaseMetadata({artifact, version:receipt.appVersion, platform, publicKey, sign, now, output, app});
}

async function retainedRelease(artifact, receipt, output) {
  if (!output || receipt.dirty !== false || receipt.sourceChanged !== false || !/^[a-f0-9]{40}$/.test(receipt.commit ?? '')) throw Error('Renewal requires a clean unchanged committed receipt and a new output path');
  const platform = {'x86_64-pc-windows-msvc':'windows-x86_64','aarch64-apple-darwin':'darwin-aarch64'}[receipt.target];
  const bytes = await readFile(artifact), signature = await readFile(`${artifact}.sig`, 'utf8');
  const sha256 = data => createHash('sha256').update(data).digest('hex');
  if (receipt.artifact?.bytes !== bytes.length || receipt.artifact?.sha256 !== sha256(bytes)
      || receipt.signature?.bytes !== Buffer.byteLength(signature) || receipt.signature?.sha256 !== sha256(signature)) throw Error('Renewal artifact/signature differs from clean receipt');
  return platform;
}

// Initial binding for a pre-metadata release (all of them are Metrix Writer
// identity releases, so only WRITER_APP) uses the owner's retained clean
// receipt and original artifact signature. The CLI additionally verifies the
// receipt version/key against the recorded source commit before signing.
export async function bindLegacyMetadata({artifact, receipt, output, publicKey, sign, now}) {
  const platform = await retainedRelease(artifact, receipt, output);
  return writeReleaseMetadata({artifact, version:receipt.appVersion, platform, publicKey, sign, now, output});
}
