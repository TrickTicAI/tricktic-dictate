// Tauri/minisign Ed25519 verification; kept compatible with Tricktic release tooling.
import { createHash, createPublicKey, verify } from 'node:crypto';
export function verifySignature(bytes, encodedSignature, encodedPublicKey) {
  const pubLines = Buffer.from(encodedPublicKey.trim(), 'base64').toString('utf8').trim().split(/\r?\n/);
  const sigLines = Buffer.from(encodedSignature.trim(), 'base64').toString('utf8').trim().split(/\r?\n/);
  const pub = Buffer.from(pubLines[1] ?? '', 'base64'), sig = Buffer.from(sigLines[1] ?? '', 'base64');
  if (pub.length !== 42 || sig.length !== 74 || pub.subarray(0,2).toString() !== 'Ed' || !pub.subarray(2,10).equals(sig.subarray(2,10)) || !sigLines[2]?.startsWith('trusted comment: ')) throw Error('Malformed signature or wrong signing key');
  const algorithm = sig.subarray(0,2).toString();
  if (!['Ed','ED'].includes(algorithm)) throw Error('Unsupported signature algorithm');
  const key = createPublicKey({key:Buffer.concat([Buffer.from('302a300506032b6570032100','hex'),pub.subarray(10)]),format:'der',type:'spki'});
  const data = algorithm === 'ED' ? createHash('blake2b512').update(bytes).digest() : bytes;
  const trusted = Buffer.concat([sig.subarray(10), Buffer.from(sigLines[2].slice('trusted comment: '.length))]);
  if (!verify(null,data,key,sig.subarray(10)) || !verify(null,trusted,key,Buffer.from(sigLines[3] ?? '', 'base64'))) throw Error('Installer signature verification failed');
}
