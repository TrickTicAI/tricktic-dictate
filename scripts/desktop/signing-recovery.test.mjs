// Offline drill: fresh synthetic keys only; never read release-host credentials.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync, createPrivateKey, createPublicKey, randomBytes, createHash, sign, verify } from 'node:crypto';
import { mkdtemp, writeFile, readFile, copyFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { verifySignature } from './signature.mjs';

const publicDer = key => createPublicKey(key).export({ format: 'der', type: 'spki' });
function signer(privateKey, id = randomBytes(8)) {
  const pub = Buffer.concat([Buffer.from('Ed'), id, publicDer(privateKey).subarray(-32)]);
  return {
    publicKey: Buffer.from(`untrusted comment: recovery drill\n${pub.toString('base64')}\n`).toString('base64'),
    sign(bytes) {
      const raw = sign(null, createHash('blake2b512').update(bytes).digest(), privateKey);
      const comment = 'recovery-drill:synthetic';
      const global = sign(null, Buffer.concat([raw, Buffer.from(comment)]), privateKey);
      const packet = Buffer.concat([Buffer.from('ED'), id, raw]);
      return Buffer.from(`untrusted comment: drill\n${packet.toString('base64')}\ntrusted comment: ${comment}\n${global.toString('base64')}\n`).toString('base64');
    },
  };
}

test('encrypted test-key backup restores the same signer; wrong password and substituted key fail', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'writer-recovery-'));
  try {
    const password = randomBytes(32);
    let original = generateKeyPairSync('ed25519').privateKey;
    const expected = publicDer(original);
    const backup = join(directory, 'encrypted-backup.pem');
    await writeFile(backup, original.export({ format: 'pem', type: 'pkcs8', cipher: 'aes-256-cbc', passphrase: password }), { mode: 0o600 });
    original = null;
    const restoredPath = join(directory, 'restored.pem');
    await copyFile(backup, restoredPath);
    assert.equal((await stat(restoredPath)).mode & 0o077, 0);
    const encrypted = await readFile(restoredPath);
    assert.throws(() => createPrivateKey({ key: encrypted, passphrase: randomBytes(32) }));
    const restored = createPrivateKey({ key: encrypted, passphrase: password });
    assert.ok(publicDer(restored).equals(expected), 'restore must preserve public identity');
    assert.ok(!publicDer(generateKeyPairSync('ed25519').privateKey).equals(expected), 'a new key is not recovery');
    const trusted = signer(restored), bytes = Buffer.from('synthetic release after recovery');
    verifySignature(bytes, trusted.sign(bytes), trusted.publicKey);
    assert.throws(() => verifySignature(Buffer.from('tampered'), trusted.sign(bytes), trusted.publicKey));
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('rotation requires an old-key-authenticated bridge; new trust rejects the retired signer', () => {
  const oldKey = signer(generateKeyPairSync('ed25519').privateKey);
  const newKey = signer(generateKeyPairSync('ed25519').privateKey);
  const release = Buffer.from('synthetic release N+1');
  assert.throws(() => verifySignature(release, newKey.sign(release), oldKey.publicKey));
  // Models the embedded key in a bridge binary, not a production updater migration.
  const bridge = Buffer.from(JSON.stringify({ embeddedUpdaterKey: newKey.publicKey }));
  verifySignature(bridge, oldKey.sign(bridge), oldKey.publicKey);
  const migratedTrust = JSON.parse(bridge).embeddedUpdaterKey;
  verifySignature(release, newKey.sign(release), migratedTrust);
  assert.throws(() => verifySignature(release, oldKey.sign(release), migratedTrust));
  // Offline clients still trust the old key: there is no remote revocation list.
  verifySignature(release, oldKey.sign(release), oldKey.publicKey);
});

test('PKCS12 test identity restores certificate and signing capability without password argv', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'writer-p12-recovery-'));
  const password = randomBytes(32).toString('hex');
  const run = (args, input) => {
    const result = spawnSync('openssl', args, { cwd: directory, input, encoding: 'utf8' });
    // Never forward tool diagnostics that might include key material.
    assert.equal(result.status, 0, 'OpenSSL recovery operation failed');
    return result.stdout;
  };
  try {
    run(['req', '-new', '-newkey', 'rsa:2048', '-x509', '-sha256', '-nodes', '-days', '1', '-subj', '/CN=Writer recovery TEST ONLY', '-keyout', 'key.pem', '-out', 'cert.pem']);
    run(['pkcs12', '-export', '-inkey', 'key.pem', '-in', 'cert.pem', '-passout', 'stdin', '-out', 'backup.p12'], `${password}\n`);
    const certificate = run(['x509', '-in', 'cert.pem', '-outform', 'PEM']);
    await rm(join(directory, 'key.pem'));
    await rm(join(directory, 'cert.pem'));
    await copyFile(join(directory, 'backup.p12'), join(directory, 'restored.p12'));
    const wrong = spawnSync('openssl', ['pkcs12', '-in', 'restored.p12', '-passin', 'stdin', '-noout'], { cwd: directory, input: 'wrong-test-password\n', stdio: ['pipe', 'pipe', 'pipe'] });
    assert.notEqual(wrong.status, 0);
    const restoredPem = run(['pkcs12', '-in', 'restored.p12', '-passin', 'stdin', '-nodes'], `${password}\n`);
    const privateKey = createPrivateKey(restoredPem);
    const restoredCertificate = restoredPem.match(/-----BEGIN CERTIFICATE-----[\s\S]+?-----END CERTIFICATE-----/)[0];
    assert.equal(restoredCertificate.trim(), certificate.trim());
    const challenge = Buffer.from('synthetic signing identity restore probe');
    assert.ok(verify('sha256', challenge, createPublicKey(certificate), sign('sha256', challenge, privateKey)));
  } finally { await rm(directory, { recursive: true, force: true }); }
});
