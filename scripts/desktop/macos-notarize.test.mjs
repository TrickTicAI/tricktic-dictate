import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { notarize, notarizeMacRelease, submissionResult, gatekeeperSource } from './macos-notarize.mjs';
import { developerId } from './macos-signing.mjs';

const id = '2f6a1c3e-9b7d-4e21-8a55-0c4d3e2f1a90';
const accepted = path => `${path}: accepted\nsource=Notarized Developer ID\norigin=${developerId.identity}\n`;

test('notarytool authenticates only through the Keychain profile', async () => {
  const calls = [];
  const result = await notarize('/tmp/app.zip', async (command, argv) => {
    calls.push([command, argv]);
    return { status: 0, stdout: JSON.stringify({ id, status: 'Accepted', message: 'Processing complete' }), stderr: '' };
  });
  assert.deepEqual(result, { submissionId: id, status: 'Accepted' });
  const [command, argv] = calls[0];
  assert.equal(command, '/usr/bin/xcrun');
  assert.deepEqual(argv, ['notarytool', 'submit', '/tmp/app.zip', '--wait', '--output-format', 'json', '--keychain-profile', 'writer-notary']);
  for (const forbidden of ['--password', '-p', '--apple-id', '--key', '--issuer', '--team-id']) assert.ok(!argv.includes(forbidden), forbidden);
});
test('a rejected submission fails with its id and the developer log', async () => {
  const calls = [];
  await assert.rejects(notarize('/tmp/app.zip', async (command, argv) => {
    calls.push(argv);
    return argv[1] === 'submit'
      ? { status: 1, stdout: JSON.stringify({ id, status: 'Invalid' }), stderr: '' }
      : { status: 0, stdout: '{"issues":[{"path":"runtime/whisper/whisper-server","message":"The executable does not have the hardened runtime enabled."}]}', stderr: '' };
  }), new RegExp(`${id}.*Invalid[\\s\\S]*hardened runtime`));
  assert.deepEqual(calls[1], ['notarytool', 'log', id, '--keychain-profile', 'writer-notary']);
});
test('submission output must carry a submission id', () => {
  assert.throws(() => submissionResult('Error: HTTP status code: 401'), /no JSON/);
  assert.throws(() => submissionResult('{"status":"Accepted"}'), /no submission id/);
});
test('Gatekeeper must report a notarized Developer ID source', () => {
  assert.equal(gatekeeperSource(accepted('/A.app')), 'Notarized Developer ID');
  assert.throws(() => gatekeeperSource('/A.app: accepted\nsource=Developer ID\n'), /notarized/);
  assert.throws(() => gatekeeperSource('/A.app: rejected\nsource=Notarized Developer ID\n'), /notarized/);
});
test('release flow notarizes and staples the app, rebuilds the DMG around it, then notarizes the DMG', async () => {
  const root = await mkdtemp(join(tmpdir(), 'writer-notarize-'));
  try {
    const app = join(root, 'bundle/Metrix Writer.app'), dmg = join(root, 'bundle/Writer.dmg'), staging = join(root, 'staging');
    await mkdir(app, { recursive: true }); await mkdir(staging);
    await writeFile(dmg, 'tauri dmg');
    const calls = [];
    const run = async (command, argv) => {
      calls.push([command.split('/').at(-1), ...argv]);
      if (argv[0] === 'attach') { await mkdir(join(argv.at(-1), 'Metrix Writer.app'), { recursive: true }); }
      if (argv[0] === 'convert' && argv.includes('UDRW')) await writeFile(argv.at(-1), 'writable');
      if (argv[0] === 'convert' && argv.includes('UDZO')) await writeFile(argv.at(-1), 'stapled app dmg');
      if (command.endsWith('ditto') && argv[0] === '-c') await writeFile(argv.at(-1), 'zip');
      if (argv[1] === 'submit') return { status: 0, stdout: JSON.stringify({ id, status: 'Accepted' }), stderr: '' };
      if (command.endsWith('spctl')) return { status: 0, stdout: '', stderr: accepted(argv.at(-1)) };
      return { status: 0, stdout: '', stderr: '' };
    };
    const result = await notarizeMacRelease({ app, dmg, staging, certificateSha1: 'A'.repeat(40), run });
    assert.equal(result.teamId, developerId.teamId);
    assert.equal(result.keychainProfile, 'writer-notary');
    assert.equal(result.app.submissionId, id); assert.equal(result.installer.status, 'Accepted');
    assert.equal(result.installer.gatekeeper.source, 'Notarized Developer ID');
    assert.equal(await readFile(dmg, 'utf8'), 'stapled app dmg');
    const order = calls.map(c => c.slice(0, 4).join(' '));
    const at = text => { const i = order.findIndex(line => line.startsWith(text)); assert.ok(i >= 0, text); return i; };
    assert.ok(at('xcrun notarytool submit') < at(`xcrun stapler staple ${app}`));
    assert.ok(at(`xcrun stapler staple ${app}`) < at('hdiutil attach'));
    assert.ok(at('hdiutil detach') < at('codesign --force --sign'));
    assert.ok(at('codesign --force --sign') < order.findLastIndex(line => line.startsWith('xcrun notarytool submit')));
    assert.ok(order.includes(`xcrun stapler staple ${dmg}`) && order.includes(`xcrun stapler validate ${dmg}`));
    assert.ok(calls.find(c => c[0] === 'codesign').includes('--timestamp'));
  } finally { await rm(root, { recursive: true, force: true }); }
});
