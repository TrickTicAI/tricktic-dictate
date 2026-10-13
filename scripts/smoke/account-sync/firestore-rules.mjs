// Runs the Firestore rules/protocol suite against a private emulator on free
// ports with a demo- project ID. It never contacts or deploys to a real project.
//   npm ci --prefix infra/firebase/emulator   (once; test-only tooling)
//   node scripts/smoke/account-sync/firestore-rules.mjs [node --test options, e.g. --test-name-pattern=vector]
//   node scripts/smoke/account-sync/firestore-rules.mjs --rust   (the desktop REST adapter, MW-OUATH-6)
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const root = resolve(import.meta.dirname, '../../..');
const firebase = join(root, 'infra/firebase/emulator/node_modules/.bin/firebase');
const [major, minor] = process.versions.node.split('.').map(Number);
if (major < 22 || (major === 22 && minor < 9)) throw new Error('Node 22.9+ is required');
if (!existsSync(firebase)) throw new Error('Run `npm ci --prefix infra/firebase/emulator` first');
const java = spawnSync('java', ['-version'], { encoding: 'utf8' });
const javaMajor = Number(/version "(\d+)/.exec(java.stderr ?? '')?.[1]);
if (!(javaMajor >= 21)) throw new Error('The Firestore emulator needs Java 21+ on PATH (set JAVA_HOME/PATH)');

const freePort = () => new Promise((done, fail) => {
  const server = createServer().listen(0, '127.0.0.1', () => {
    const { port } = server.address();
    server.close(() => done(port));
  }).on('error', fail);
});
const work = mkdtempSync(join(tmpdir(), 'writer-firestore-'));
writeFileSync(join(work, 'firebase.json'), JSON.stringify({
  firestore: { rules: join(root, 'infra/firebase/firestore.rules') },
  emulators: { singleProjectMode: true, ui: { enabled: false },
    firestore: { host: '127.0.0.1', port: await freePort(), websocketPort: await freePort() } },
}));
const test = join(root, 'scripts/smoke/account-sync/firestore-rules.test.mjs');
const command = process.argv[2] === '--rust'
  ? ['cargo', 'test', '--manifest-path', join(root, 'Cargo.toml'), '--locked', '-p', 'metrix-writer-api',
    'account_sync::', '--', '--ignored', '--test-threads=1']
  : ['node', '--test', ...process.argv.slice(2), test];
const child = spawn(firebase, ['emulators:exec', '--config', join(work, 'firebase.json'),
  '--project', 'demo-metrix-writer', '--only', 'firestore', command.map(arg => JSON.stringify(arg)).join(' ')],
{ cwd: work, stdio: 'inherit', env: { ...process.env, NO_UPDATE_NOTIFIER: '1' } });
child.on('exit', code => {
  rmSync(work, { recursive: true, force: true });
  process.exit(code ?? 1);
});
