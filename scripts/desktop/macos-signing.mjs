// One-time, user-scoped development identity. No Apple account or global trust.
// Releases may instead select the Developer ID identity (MW-54); see developerId.
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { randomBytes } from 'node:crypto';
import { tmpdir, homedir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

export const signingIdentity = 'TrickTic Dictate Dev';
// Public identifiers only. The notary credential stays in the Keychain profile;
// its app-specific password never enters this repository, argv or logs.
// The release Mac supplies its certificate name in WRITER_APPLE_DEVELOPER_ID,
// e.g. "Developer ID Application: Name (TEAMID1234)"; the team ID is its
// parenthesised suffix. The example default matches no real certificate.
const developerIdIdentity = process.env.WRITER_APPLE_DEVELOPER_ID || 'Developer ID Application: Example Developer (ABCDE12345)';
export const developerId = {
  identity: developerIdIdentity,
  teamId: /\(([A-Z0-9]{10})\)$/.exec(developerIdIdentity)?.[1] ?? '',
  notaryProfile: 'writer-notary',
};
export function identityHash(output, identity = signingIdentity) {
  const name = identity.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const matches = [...output.matchAll(new RegExp(`^\\s*\\d+\\) ([A-Fa-f0-9]{40}) "${name}"\\s*$`, 'gm'))];
  if (matches.length !== 1) throw Error(`Expected exactly one valid ${identity} code-signing identity; found ${matches.length}. ${identity === signingIdentity ? 'Run pnpm desktop:signing:setup or inspect' : 'Inspect'} login Keychain certificates.`);
  return matches[0][1].toUpperCase();
}
function run(command, args) {
  const result = spawnSync(command, args, { encoding: 'utf8' });
  if (result.error || result.status !== 0) throw Error(`${command} failed: ${result.error?.message ?? result.stderr.trim()}`);
  return result.stdout;
}
async function setup() {
  if (process.platform !== 'darwin') throw Error('Run signing setup on the Mac.');
  const keychain = join(homedir(), 'Library/Keychains/login.keychain-db');
  const identities = run('/usr/bin/security', ['find-identity', '-v', '-p', 'codesigning', keychain]);
  if (identities.includes(`"${signingIdentity}"`)) {
    console.log(`Ready: ${signingIdentity} (${identityHash(identities)}). Existing identity retained.`);
    return;
  }
  const existing = spawnSync('/usr/bin/security', ['find-certificate', '-c', signingIdentity, keychain]);
  if (existing.status === 0) throw Error(`A ${signingIdentity} certificate already exists but is not a valid signing identity. In Keychain Access, check its private key, expiration and Code Signing trust. Setup will not replace it and invalidate existing grants.`);
  const temporary = await mkdtemp(join(tmpdir(), 'writer-signing-'));
  try {
    const password = randomBytes(24).toString('hex');
    const config = join(temporary, 'certificate.cnf');
    await writeFile(config, '[req]\nprompt = no\ndistinguished_name = subject\nx509_extensions = signing\n[subject]\nCN = ' + signingIdentity + '\n[signing]\nbasicConstraints = critical,CA:false\nkeyUsage = critical,digitalSignature\nextendedKeyUsage = critical,codeSigning\n', { mode: 0o600 });
    run('/usr/bin/openssl', ['req', '-new', '-newkey', 'rsa:2048', '-x509', '-sha256', '-nodes', '-days', '3650', '-config', config, '-keyout', join(temporary, 'private.key'), '-out', join(temporary, 'certificate.pem')]);
    run('/usr/bin/openssl', ['pkcs12', '-export', '-keypbe', 'PBE-SHA1-3DES', '-certpbe', 'PBE-SHA1-3DES', '-macalg', 'sha1', '-inkey', join(temporary, 'private.key'), '-in', join(temporary, 'certificate.pem'), '-name', signingIdentity, '-passout', `pass:${password}`, '-out', join(temporary, 'identity.p12')]);
    // Only codesign is pre-authorized to use this new private key, never all apps.
    run('/usr/bin/security', ['import', join(temporary, 'identity.p12'), '-k', keychain, '-P', password, '-T', '/usr/bin/codesign']);
    console.log('The new identity is in your login Keychain. macOS may ask you to authorize trust for code signing.');
    run('/usr/bin/security', ['add-trusted-cert', '-r', 'trustRoot', '-p', 'codeSign', '-k', keychain, join(temporary, 'certificate.pem')]);
    console.log(`Ready: ${signingIdentity} (${identityHash(run('/usr/bin/security', ['find-identity', '-v', '-p', 'codesigning', keychain]))}).`);
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  setup().catch(error => { console.error(`macOS signing: ${error.message}`); process.exitCode = 1; });
}
