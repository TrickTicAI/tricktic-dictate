// Native macOS bundle with optional verified local runtimes. Each invocation owns a fresh target directory.
import { readFile, writeFile, mkdir, mkdtemp, readdir, lstat, readlink, rm } from 'node:fs/promises';
import { join, relative, dirname, basename } from 'node:path';
import { spawnSync, spawn } from 'node:child_process';
import { sha256, verifyRuntime } from './runtime.mjs';
import { identityHash, signingIdentity, developerId } from './macos-signing.mjs';
import { notarizeMacRelease } from './macos-notarize.mjs';
import { assertReleaseSource, releaseKey, releaseArtifacts, archiveRelease } from './macos-release.mjs';
import { verifySpeechProtocol } from './speech-runtime.mjs';
import { verifySignature } from './signature.mjs';
import { writeReleaseEnvelopes } from './update-metadata.mjs';
import { previewNotices } from './notices.mjs';

export async function inventory(root, current = root) {
  const files = [];
  for (const name of (await readdir(current)).sort()) {
    const path = join(current, name), info = await lstat(path), entry = { path: relative(root, path), mode: info.mode & 0o777 };
    if (info.isSymbolicLink()) files.push({ ...entry, symlink: await readlink(path) });
    else if (info.isDirectory()) files.push(...await inventory(root, path));
    else if (info.isFile()) files.push({ ...entry, bytes: info.size, sha256: sha256(await readFile(path)) });
    else throw Error(`Unexpected bundle file type: ${path}`);
  }
  return files;
}
export async function buildMacInstaller({ repo, check, source, release, ffmpegSource, signInConfig, developerIdRelease = false }) {
  if (developerIdRelease && !release) throw Error('Developer ID signing is a release mode.');
  // The development identity stays the default for local work (MW-53).
  const identity = developerIdRelease ? developerId.identity : signingIdentity;
  if (source && process.arch !== 'arm64') throw Error('Local macOS runtimes require Apple Silicon.');
  const lockBytes = source ? await readFile(join(repo, 'scripts/desktop/macos-runtime.lock.json')) : null;
  const lock = lockBytes ? JSON.parse(lockBytes) : null;
  if (source) {
    await verifyRuntime(source, lock);
    await verifySpeechProtocol(source, lock, join(repo, 'runtime/whisper/writer-auth.h'));
  }
  if (release && process.arch !== 'arm64') throw Error('macOS releases require an Apple Silicon Mac.');
  if (!['arm64', 'x64'].includes(process.arch)) throw Error('macOS installer requires an arm64 or x64 host.');
  const env = { ...process.env, APPLE_SIGNING_IDENTITY: identity, MACOSX_DEPLOYMENT_TARGET: '15.0' };
  if (process.env.APPLE_SIGNING_IDENTITY && process.env.APPLE_SIGNING_IDENTITY !== identity) throw Error(`This ${developerIdRelease ? 'Developer ID' : 'development'} installer requires APPLE_SIGNING_IDENTITY=${identity}.`);
  // This entry point never imports a certificate, and Tauri never notarizes:
  // Apple credentials are removed so only the writer-notary Keychain profile is
  // used, by macos-notarize.mjs. Release signing uses only the explicitly
  // supplied Writer key after validating its public half.
  for (const name of ['APPLE_CERTIFICATE', 'APPLE_CERTIFICATE_PASSWORD', 'APPLE_ID', 'APPLE_PASSWORD', 'APPLE_API_KEY', 'APPLE_API_KEY_PATH', 'APPLE_API_ISSUER', 'TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PATH']) delete env[name];
  function probe(command, argv) {
    const r = spawnSync(command, argv, { cwd: repo, env, encoding: 'utf8', timeout: 30000, maxBuffer: 32 * 1024 * 1024 });
    if (r.error || r.status !== 0) throw Error(`${command} failed: ${r.error?.message ?? r.stderr.trim()}`);
    return r.stdout.trim();
  }
  function run(command, argv) {
    return new Promise((ok, fail) => {
      const child = spawn(command, argv, { cwd: repo, env, stdio: 'inherit' });
      child.on('error', fail);
      child.on('exit', code => code === 0 ? ok() : fail(Error(`${command} failed (${code})`)));
    });
  }
  const tools = { node: process.version, pnpm: probe('pnpm', ['--version']), rust: probe('rustc', ['--version']), cargo: probe('cargo', ['--version']), xcode: probe('xcodebuild', ['-version']), sdk: probe('xcrun', ['--show-sdk-version']), macOS: probe('sw_vers', ['-productVersion']), codesign: `system tool (${probe('sw_vers', ['-buildVersion'])})` };
  const [major, minor] = process.versions.node.split('.').map(Number);
  if (major < 22 || (major === 22 && minor < 9)) throw Error('Install Node >=22.9.');
  if (tools.pnpm !== '9.0.0') throw Error('Use the repository pnpm@9.0.0 pin.');
  const certificateSha1 = identityHash(probe('/usr/bin/security', ['find-identity', '-v', '-p', 'codesigning']), identity);
  if (developerIdRelease) {
    tools.notarytool = probe('xcrun', ['notarytool', '--version']);
    // Proves the stored profile authenticates; history is not printed.
    probe('xcrun', ['notarytool', 'history', '--keychain-profile', developerId.notaryProfile, '--output-format', 'json']);
  }
  const target = `${process.arch === 'arm64' ? 'aarch64' : 'x86_64'}-apple-darwin`;
  if (!probe('rustup', ['target', 'list', '--installed']).split('\n').includes(target)) throw Error(`Run rustup target add ${target}.`);
  const config = JSON.parse(await readFile(join(repo, 'apps/desktop/tauri/tauri.conf.json')));
  if (!(await readFile(join(repo, 'apps/desktop/tauri/Cargo.toml'), 'utf8')).includes(`version = "${config.version}"`)) throw Error('Desktop Cargo and Tauri versions differ.');
  if (release && source && (!env.WRITER_RELEASE_WHISPER_MODEL || !env.WRITER_RELEASE_NEMO_MODEL)) throw Error('Local Mac releases require WRITER_RELEASE_WHISPER_MODEL and WRITER_RELEASE_NEMO_MODEL for native speech acceptance.');
  if (release) {
    assertReleaseSource({ status: probe('git', ['status', '--porcelain', '--untracked-files=all']) });
    env.WRITER_SIGNING_KEY_FILE = await releaseKey(repo, env, config.plugins.updater.pubkey);
    env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ??= '';
  }
  console.log(`Preflight passed: ${target}; ${identity} (${certificateSha1})${developerIdRelease ? `; notarytool profile ${developerId.notaryProfile}` : ''}; ${source ? `${lock.files.length} pinned local runtime/notice inputs` : 'cloud-only'}.`);
  if (check) { await previewNotices({ repo, target, lock, ffmpegArchive: ffmpegSource }); return; }
  const local = join(repo, '.local');
  await mkdir(local, { recursive: true });
  const staging = await mkdtemp(join(local, 'macos-installer-'));
  env.CARGO_TARGET_DIR = join(staging, 'target');
  const snapshot = async () => {
    const paths = probe('git', ['ls-files', '-z', '--cached', '--others', '--exclude-standard']).split('\0').filter(Boolean).sort();
    const files = [];
    for (const path of paths) {
      const absolute = join(repo, path);
      try {
        const info = await lstat(absolute);
        files.push({ path, mode: info.mode & 0o777, sha256: sha256(info.isSymbolicLink() ? await readlink(absolute) : await readFile(absolute)) });
      } catch (error) { if (error.code === 'ENOENT') files.push({ path, deleted: true }); else throw error; }
    }
    return { commit: probe('git', ['rev-parse', 'HEAD']), diff: probe('git', ['diff', 'HEAD', '--binary']), status: probe('git', ['status', '--porcelain', '--untracked-files=all']), sourceSha256: sha256(JSON.stringify(files)) };
  };
  const before = await snapshot();
  if (release) assertReleaseSource(before);
  await run('pnpm', ['install', '--frozen-lockfile']);
  if (release) {
    const signingProbe = join(staging, 'signing-check.txt');
    await writeFile(signingProbe, 'Writer macOS release signing preflight');
    env.TAURI_SIGNING_PRIVATE_KEY_PATH = env.WRITER_SIGNING_KEY_FILE;
    probe(process.execPath, [join(repo, 'node_modules/@tauri-apps/cli/tauri.js'), 'signer', 'sign', signingProbe]);
    verifySignature(await readFile(signingProbe), await readFile(`${signingProbe}.sig`, 'utf8'), config.plugins.updater.pubkey);
    delete env.TAURI_SIGNING_PRIVATE_KEY_PATH;
    env.TAURI_SIGNING_PRIVATE_KEY = env.WRITER_SIGNING_KEY_FILE;
  }
  await run(process.execPath, [join(repo, 'scripts/desktop/build.mjs'), '--target', target, ...(source ? ['--resources', source, ...(ffmpegSource ? ['--ffmpeg-source', ffmpegSource] : [])] : []), ...(signInConfig ? ['--google-sign-in', signInConfig] : []), ...(release ? ['--release'] : [])]);
  const bundle = join(env.CARGO_TARGET_DIR, target, 'release/bundle');
  const app = join(bundle, 'macos', `${config.productName}.app`);
  const dmgNames = (await readdir(join(bundle, 'dmg'))).filter(name => name.endsWith('.dmg'));
  if (dmgNames.length !== 1) throw Error('Expected exactly one fresh DMG.');
  const dmg = join(bundle, 'dmg', dmgNames[0]);
  probe('/usr/bin/codesign', ['--verify', '--deep', '--strict', '--verbose=2', app]);
  // Require the exact certificate, not merely a matching display name or ad-hoc signature.
  for (const artifact of [app, dmg]) probe('/usr/bin/codesign', ['--verify', '--strict', '-R', `=certificate leaf = H"${certificateSha1}"`, artifact]);
  const sign = path => {
    delete env.TAURI_SIGNING_PRIVATE_KEY;
    env.TAURI_SIGNING_PRIVATE_KEY_PATH = env.WRITER_SIGNING_KEY_FILE;
    probe(process.execPath, [join(repo, 'node_modules/@tauri-apps/cli/tauri.js'), 'signer', 'sign', path]);
  };
  let notarization = null;
  if (developerIdRelease) {
    // Hardened Runtime is already required at signing: Tauri's macOS
    // hardenedRuntime default and the Developer ID mode of macos-runtime.mjs.
    probe('/usr/bin/codesign', ['--verify', '--deep', '--strict', '-R', `=anchor apple generic and certificate leaf[subject.OU] = "${developerId.teamId}"`, app]);
    notarization = await notarizeMacRelease({ app, dmg, staging, certificateSha1 });
    // Tauri archived the app before stapling. Re-archive the stapled app; the
    // inventory comparison below proves the archive matches it exactly.
    const archive = `${app}.tar.gz`;
    await rm(archive); await rm(`${archive}.sig`);
    await run('/usr/bin/tar', ['--no-mac-metadata', '-czf', archive, '-C', dirname(app), basename(app)]);
    sign(archive);
    for (const artifact of [app, dmg]) probe('/usr/bin/codesign', ['--verify', '--strict', '-R', `=certificate leaf = H"${certificateSha1}"`, artifact]);
    probe('/usr/bin/codesign', ['--verify', '--deep', '--strict', app]);
  }
  let packagedRuntime = null;
  if (source) {
    const resources = join(app, 'Contents/Resources');
    const manifestBytes = await readFile(join(resources, 'runtime-manifest.json'));
    const files = JSON.parse(manifestBytes);
    if (JSON.stringify(files.map(f => f.path)) !== JSON.stringify(lock.files.map(f => f.path))) throw Error('Packaged runtime paths differ from approved inputs.');
    await verifyRuntime(resources, { version: 1, files });
    for (const entry of lock.files) {
      if (entry.role === 'runtime') probe('/usr/bin/codesign', ['--verify', '--strict', '-R', `=certificate leaf = H"${certificateSha1}"`, join(resources, entry.path)]);
      else {
        const bytes = await readFile(join(resources, entry.path));
        if (sha256(bytes) !== entry.sha256 || bytes.length !== entry.bytes) throw Error(`Packaged notice changed: ${entry.path}`);
      }
    }
    packagedRuntime = { manifestSha256: sha256(manifestBytes), files };
  }
  let speechAcceptance = null;
  if (release && source) {
    const evidence = join(staging, 'speech-acceptance.json');
    await run('python3', [join(repo, 'scripts/desktop/smoke-macos-speech.py'),
      '--resources', join(app, 'Contents/Resources'), '--whisper-model', env.WRITER_RELEASE_WHISPER_MODEL,
      '--nemo-model', env.WRITER_RELEASE_NEMO_MODEL, '--output', evidence]);
    speechAcceptance = JSON.parse(await readFile(evidence, 'utf8'));
  }
  const display = spawnSync('/usr/bin/codesign', ['-d', '-r-', '--verbose=4', app], { encoding: 'utf8' });
  if (display.status !== 0) throw Error('Could not inspect app signature.');
  const plist = name => probe('/usr/libexec/PlistBuddy', ['-c', `Print :${name}`, join(app, 'Contents/Info.plist')]);
  if (plist('LSMinimumSystemVersion') !== '15.0' || plist('CFBundleIdentifier') !== config.identifier || plist('CFBundleShortVersionString') !== config.version) throw Error('Packaged app identity or deployment floor does not match.');
  const executable = join(app, 'Contents/MacOS', plist('CFBundleExecutable'));
  const executableBytes = await readFile(executable), dmgBytes = await readFile(dmg);
  const files = await inventory(app);
  const after = await snapshot();
  if (release) assertReleaseSource(before, after);
  const receipt = { version: 1, googleSignIn: !!signInConfig, appVersion: config.version, builtAt: new Date().toISOString(), commit: before.commit, dirty: !!before.status, diffSha256: sha256(before.diff), sourceSha256: before.sourceSha256, sourceChanged: JSON.stringify(before) !== JSON.stringify(after), target, tools, cloudOnly: !source, speechAcceptance, runtimeLockSha256: lockBytes ? sha256(lockBytes) : null, inputs: lock?.files ?? [], packagedRuntime, signature: { identity, certificateSha1, ...(developerIdRelease ? { teamId: developerId.teamId } : {}), details: display.stderr.trim() }, ...(developerIdRelease ? { signingMode: 'developer-id', notarization } : {}), artifact: { path: dmg, bytes: dmgBytes.length, sha256: sha256(dmgBytes) }, app: { path: app, sha256: sha256(JSON.stringify(files)), files }, executable: { path: executable, bytes: executableBytes.length, sha256: sha256(executableBytes) } };
  let receiptPath = `${dmg}.receipt.json`;
  if (release) {
    const archive = `${app}.tar.gz`;
    const signed = await releaseArtifacts(archive, config.plugins.updater.pubkey);
    // Ensure Tauri archived the exact verified signed app, including executable modes.
    const extracted = join(staging, 'archive-check');
    await mkdir(extracted);
    probe('/usr/bin/tar', ['-xzf', archive, '-C', extracted]);
    if (JSON.stringify(await inventory(join(extracted, `${config.productName}.app`))) !== JSON.stringify(files)) throw Error('Updater archive differs from the verified app bundle.');
    probe('/usr/bin/codesign', ['--verify', '--deep', '--strict', '-R', `=certificate leaf = H"${certificateSha1}"`, join(extracted, `${config.productName}.app`)]);
    const metadata = await writeReleaseEnvelopes({ artifact: archive, version: config.version,
      platform: 'darwin-aarch64', publicKey: config.plugins.updater.pubkey,
      sign: async path => sign(path) });
    const [retainedDmg, retainedArchive, retainedSignature, retainedMetadata, retainedDictate] = await archiveRelease({ version: config.version, commit: before.commit, paths: [dmg, archive, `${archive}.sig`, metadata.writerRelease, metadata.dictateRelease] });
    receipt.writerRelease = retainedMetadata;
    receipt.dictateRelease = retainedDictate;
    receipt.codeSignature = receipt.signature;
    receipt.installer = { ...receipt.artifact, path: retainedDmg };
    receipt.artifact = { ...signed.artifact, path: retainedArchive };
    receipt.signature = { ...signed.signature, path: retainedSignature };
    receipt.platform = 'darwin-aarch64';
    receipt.release = true;
    receiptPath = `${retainedArchive}.receipt.json`;
    // Check again after verification/copying: a source edit must not acquire a receipt.
    assertReleaseSource(before, await snapshot());
    console.log(`Retained DMG: ${retainedDmg}\nUpdater: ${retainedArchive}\nSignature: ${retainedSignature}`);
  }
  await writeFile(receiptPath, JSON.stringify(receipt, null, 2) + '\n');
  console.log(`App: ${app}\nInstaller: ${dmg}\nReceipt: ${receiptPath}\nSHA-256: ${receipt.artifact.sha256}`);
}
