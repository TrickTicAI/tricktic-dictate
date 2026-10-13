// REST exchange probe only. Credential Manager + Firebase SDK native proof is separate.
import { readFile } from 'node:fs/promises';
import { exchangeGoogle, refreshSession, validateConfig, ProofError } from './auth-proof.mjs';

try {
  if (process.argv.length !== 3 || process.stdin.isTTY) {
    throw new ProofError('usage: node android-token.mjs <private-config.json> < private-google-id-token.txt');
  }
  const config = validateConfig(JSON.parse(await readFile(process.argv[2], 'utf8')));
  let token = '';
  for await (const chunk of process.stdin) {
    token += chunk;
    if (token.length > 32768) throw new ProofError('invalid_token');
  }
  const session = await exchangeGoogle(config, token.trim(), config.webClientId);
  await refreshSession(config, session);
  console.log(JSON.stringify({ proof: 'google-token-live', login: 'passed', refresh: 'passed',
    audience: 'configured-web-client', nativeCredentialManager: 'not-tested', sync: 'off' }));
} catch (error) {
  console.error(error instanceof ProofError ? error.code : 'proof_failed');
  process.exitCode = 1;
}
