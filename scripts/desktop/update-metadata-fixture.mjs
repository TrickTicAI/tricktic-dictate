// Regenerates public-only test vectors. Ephemeral private keys never leave memory.
import { generateKeyPairSync, randomBytes, createHash, sign } from 'node:crypto';
import { writeFile } from 'node:fs/promises';
import { manifestFor, WRITER_APP, DICTATE_APP } from './update-metadata.mjs';
export function signer() {
  const { privateKey, publicKey } = generateKeyPairSync('ed25519');
  const id = randomBytes(8);
  return {
    publicKey: Buffer.from(`untrusted comment: ephemeral test key\n${Buffer.concat([Buffer.from('Ed'), id, publicKey.export({format:'der',type:'spki'}).subarray(-32)]).toString('base64')}\n`).toString('base64'),
    sign(bytes) {
      const signature = sign(null, createHash('blake2b512').update(bytes).digest(), privateKey);
      const comment = 'Writer test fixture';
      const global = sign(null, Buffer.concat([signature, Buffer.from(comment)]), privateKey);
      return Buffer.from(`untrusted comment: fixture\n${Buffer.concat([Buffer.from('ED'), id, signature]).toString('base64')}\ntrusted comment: ${comment}\n${global.toString('base64')}\n`).toString('base64');
    },
  };
}
if (process.argv[2] === '--generate') {
  const key = signer(), bytes = Buffer.from('Writer truncated installer fixture; no executable header');
  const manifest = manifestFor(bytes, '0.21.0', 'windows-x86_64', 1800000000, DICTATE_APP);
  const envelope = value => {
    const payload = Buffer.from(JSON.stringify(value));
    return { payload: payload.toString('base64'), signature: key.sign(payload) };
  };
  const fixture = { public_key: key.publicKey, artifact: bytes.toString('base64'), artifact_signature: key.sign(bytes),
    valid: envelope(manifest),
    older: envelope({...manifest, version:'0.19.0'}),
    newer: envelope({...manifest, version:'0.22.0', issued_at:1800000010}),
    renewed: envelope({...manifest, issued_at:1800000100, expires_at:1800000100+2592000}),
    replaced: envelope({...manifest, sha256:'a'.repeat(64)}),
    mac: envelope({...manifest, platform:'darwin-aarch64'}),
    wrong_app: envelope({...manifest, app:'com.other.app'}),
    // The same release bound to the Metrix Writer identity (`writer_release`).
    legacy: envelope({...manifest, app:WRITER_APP}),
    legacy_newer: envelope({...manifest, app:WRITER_APP, version:'0.22.0', issued_at:1800000010}),
    too_large: envelope({...manifest, size:300*1024*1024+1}),
    too_long: envelope({...manifest, expires_at:1800000000+2592001}),
  };
  await writeFile(new URL('../../apps/desktop/tauri/tests/fixtures/update-release.json', import.meta.url), `${JSON.stringify(fixture, null, 2)}\n`);
}
