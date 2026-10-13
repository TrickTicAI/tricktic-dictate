// Developer ID notarization for macOS releases (MW-54). notarytool authenticates
// only through the stored Keychain profile; the app-specific password never
// appears in this repository, argv, environment or logs.
import { rm, rename } from 'node:fs/promises';
import { join, basename } from 'node:path';
import { spawn } from 'node:child_process';
import { developerId } from './macos-signing.mjs';

// Resolves { status, stdout, stderr }; never rejects on a non-zero exit.
export function capture(command, argv) {
  return new Promise((ok, fail) => {
    const child = spawn(command, argv, { stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '', stderr = '';
    child.stdout.on('data', chunk => { stdout += chunk; });
    child.stderr.on('data', chunk => { stderr += chunk; });
    child.on('error', fail);
    child.on('close', status => ok({ status, stdout, stderr }));
  });
}
async function required(run, command, argv) {
  const result = await run(command, argv);
  if (result.status !== 0) throw Error(`${basename(command)} ${argv[0]} failed: ${(result.stderr || result.stdout).trim()}`);
  return result;
}
export function submissionResult(stdout) {
  let parsed;
  try { parsed = JSON.parse(stdout); } catch { throw Error('notarytool returned no JSON result.'); }
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(parsed?.id ?? '')) throw Error('notarytool returned no submission id.');
  return { submissionId: parsed.id, status: parsed.status ?? 'unknown' };
}
export function notaryArgs(command, ...rest) {
  return ['notarytool', command, ...rest, '--keychain-profile', developerId.notaryProfile];
}
export async function notarize(path, run = capture) {
  const result = await run('/usr/bin/xcrun', notaryArgs('submit', path, '--wait', '--output-format', 'json'));
  const submission = submissionResult(result.stdout);
  if (submission.status !== 'Accepted') {
    // Apple's developer log names each rejected binary and reason.
    const log = await run('/usr/bin/xcrun', notaryArgs('log', submission.submissionId));
    throw Error(`Notarization ${submission.submissionId} of ${basename(path)} ended ${submission.status}.\n${log.stdout.trim()}`);
  }
  return submission;
}
export function gatekeeperSource(output) {
  const source = /^source=(.+)$/m.exec(output)?.[1]?.trim();
  if (!/accepted/.test(output) || source !== 'Notarized Developer ID') throw Error(`Gatekeeper did not accept a notarized Developer ID artifact:\n${output.trim()}`);
  return source;
}
async function staple(path, run) {
  await required(run, '/usr/bin/xcrun', ['stapler', 'staple', path]);
  await required(run, '/usr/bin/xcrun', ['stapler', 'validate', path]);
}
async function gatekeeper(path, type, run) {
  const argv = ['--assess', '-vv', '--type', type, ...(type === 'open' ? ['--context', 'context:primary-signature'] : []), path];
  const result = await run('/usr/sbin/spctl', argv);
  const output = `${result.stdout}${result.stderr}`;
  if (result.status !== 0) throw Error(`spctl rejected ${basename(path)}:\n${output.trim()}`);
  return { source: gatekeeperSource(output), assessment: output.trim() };
}
// Replace the DMG's app with the stapled copy while keeping Tauri's layout.
async function restapleInstaller({ app, dmg, staging, run }) {
  const writable = join(staging, 'notarize-rw.dmg'), mount = join(staging, 'notarize-mount'), packed = join(staging, 'notarize-final.dmg');
  await required(run, '/usr/bin/hdiutil', ['convert', dmg, '-quiet', '-format', 'UDRW', '-o', writable]);
  await required(run, '/usr/bin/hdiutil', ['attach', writable, '-quiet', '-nobrowse', '-noautoopen', '-noverify', '-mountpoint', mount]);
  try {
    const target = join(mount, basename(app));
    await rm(target, { recursive: true });
    await required(run, '/usr/bin/ditto', [app, target]);
  } finally {
    await required(run, '/usr/bin/hdiutil', ['detach', mount, '-quiet']);
  }
  await required(run, '/usr/bin/hdiutil', ['convert', writable, '-quiet', '-format', 'UDZO', '-imagekey', 'zlib-level=9', '-o', packed]);
  await rm(writable);
  await rename(packed, dmg);
}
// Notarizes and staples the signed app, rebuilds the DMG around it, then signs,
// notarizes and staples the DMG. Both must pass Gatekeeper as notarized.
export async function notarizeMacRelease({ app, dmg, staging, certificateSha1, run = capture }) {
  const zip = join(staging, 'notarize-app.zip');
  await required(run, '/usr/bin/ditto', ['-c', '-k', '--sequesterRsrc', '--keepParent', app, zip]);
  const appSubmission = await notarize(zip, run);
  await rm(zip);
  await staple(app, run);
  await restapleInstaller({ app, dmg, staging, run });
  await required(run, '/usr/bin/codesign', ['--force', '--sign', certificateSha1, '--timestamp', dmg]);
  const installerSubmission = await notarize(dmg, run);
  await staple(dmg, run);
  return {
    teamId: developerId.teamId,
    keychainProfile: developerId.notaryProfile,
    app: { ...appSubmission, stapled: true, gatekeeper: await gatekeeper(app, 'exec', run) },
    installer: { ...installerSubmission, stapled: true, gatekeeper: await gatekeeper(dmg, 'open', run) },
  };
}
