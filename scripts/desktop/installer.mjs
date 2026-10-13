// Installer entry point: Linux x64 -> Windows NSIS; native macOS -> app/DMG.
import { readFile, writeFile, mkdir, mkdtemp, symlink, access } from 'node:fs/promises';
import { resolve, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync, spawn } from 'node:child_process';
import { verifyRuntime, stageRuntime, sha256 } from './runtime.mjs';
import { verifySignature } from './signature.mjs';
import { createWindowsSigner, windowsReleaseType, windowsReleaseBanner, requirePublicReleaseSource, WITHOUT_AUTHENTICODE } from './windows-signing.mjs';
import { writeReleaseEnvelopes } from './update-metadata.mjs';
import { previewNotices } from './notices.mjs';
import { desktopSignInConfig, googleSignInEnv } from './security-artifact.mjs';
const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const target = 'x86_64-pc-windows-msvc';
const help = 'macOS: pnpm desktop:installer [--resources <verified Apple Silicon runtime directory> [--ffmpeg-source <ffmpeg-8.0.3.tar.xz>]] [--google-sign-in <private config>] [--check] [--release [--developer-id]] (first run pnpm desktop:signing:setup; --developer-id signs, notarizes and staples: docs/features/macos-release.md#developer-id-notarization-mw-54)\nWindows: pnpm desktop:installer --resources <directory containing runtime/ and notices/> [--ffmpeg-source <ffmpeg-8.0.3.tar.xz>] [--google-sign-in <private config>] [--check] [--release [--without-authenticode]]\nSetup: docs/features/tauri-desktop.md#repeatable-windows-installer';
const args = process.argv.slice(2);
let source, ffmpegSource, signInConfig, check = false, release = false, withoutAuthenticode = false, developerIdRelease = false;
const env = { ...process.env };
const versions = {};
function probe(command, argv, remedy) {
  const result = spawnSync(command, argv, { cwd: repo, env, encoding: 'utf8', timeout: 30000 });
  if (result.error || result.status !== 0) throw Error(`${command} is missing or unusable. ${remedy}\n${result.error?.message ?? result.stderr?.trim()}`);
  return result.stdout.trim();
}
async function required(path, remedy) {
  try { await access(path); } catch { throw Error(`Missing ${path}. ${remedy}`); }
}
async function run(command, argv) {
  await new Promise((ok, fail) => {
    const child = spawn(command, argv, { cwd: repo, env, stdio: 'inherit' });
    child.on('error', fail);
    child.on('exit', code => code === 0 ? ok() : fail(Error(`${command} failed (${code})`)));
  });
}
try {
  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--resources' && args[i + 1] && !args[i + 1].startsWith('--')) source = resolve(args[++i]);
    else if (args[i] === '--ffmpeg-source' && args[i + 1] && !args[i + 1].startsWith('--')) ffmpegSource = resolve(args[++i]);
    else if (args[i] === '--google-sign-in' && args[i + 1] && !args[i + 1].startsWith('--')) signInConfig = resolve(args[++i]);
    else if (args[i] === '--check') check = true;
    else if (args[i] === '--release') release = true;
    else if (args[i] === '--developer-id') developerIdRelease = true;
    else if (args[i] === WITHOUT_AUTHENTICODE) withoutAuthenticode = true;
    else if (args[i] === '--help') { console.log(help); process.exit(0); }
    else throw Error(help);
  }
  signInConfig = desktopSignInConfig({ path: signInConfig, release, env });
  if (signInConfig) {
    signInConfig = resolve(signInConfig);
    await googleSignInEnv({ path: signInConfig, release });
  }
  if (process.platform === 'darwin') {
    if (withoutAuthenticode) throw Error(`${WITHOUT_AUTHENTICODE} applies only to Windows releases.`);
    if (developerIdRelease && !release) throw Error('--developer-id is a release mode; pass --release --developer-id.');
    const { buildMacInstaller } = await import('./macos-installer.mjs');
    await buildMacInstaller({ repo, check, source, release, ffmpegSource, signInConfig, developerIdRelease });
    process.exit(0);
  }
  if (developerIdRelease) throw Error('--developer-id applies only to macOS releases.');
  if (process.platform !== 'linux' || process.arch !== 'x64') throw Error('desktop:installer supports native macOS or Linux/WSL x64. Build Windows installers inside an x64 Linux/WSL host; build macOS installers on a Mac.');
  const releaseType = windowsReleaseType({ release, withoutAuthenticode });
  const windowsSigner = releaseType === 'public-windows' ? createWindowsSigner({ env }) : null;
  if (windowsSigner) await windowsSigner.preflight();
  console.log(windowsReleaseBanner(releaseType));
  const config = JSON.parse(await readFile(join(repo, 'apps/desktop/tauri/tauri.conf.json')));
  if (release) {
    await requirePublicReleaseSource({ keyFile: env.WRITER_SIGNING_KEY_FILE, updaterPublicKey: config.plugins?.updater?.pubkey,
      gitStatus: () => probe('git', ['status', '--porcelain'], 'Build from a Git checkout.') });
    env.TAURI_SIGNING_PRIVATE_KEY = resolve(env.WRITER_SIGNING_KEY_FILE);
    env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ??= '';
  }
  if (!source) throw Error(`${help}\nObtain the retained Writer runtime package described in the setup guide; no backend build or resources.json is needed.`);
  if (Number(process.versions.node.split('.')[0]) < 22 || (Number(process.versions.node.split('.')[0]) === 22 && Number(process.versions.node.split('.')[1]) < 9)) throw Error('Install Node >=22.9 (Node 24 recommended).');
  versions.node = process.version;
  for (const name of ['WRITER_LLVM_ROOT', 'WRITER_WINDOWS_SDK']) {
    if (!env[name]) throw Error(`Set ${name}. See ${help.split('\n').at(-1)}`);
    env[name] = resolve(env[name]);
    if (/\s/.test(env[name])) throw Error(`${name} must have no whitespace (MSVC compiler flags).`);
  }
  env.PATH = `${env.WRITER_LLVM_ROOT}/bin:${env.PATH}`;
  env.LD_LIBRARY_PATH = `${env.WRITER_LLVM_ROOT}/lib:${env.LD_LIBRARY_PATH ?? ''}`;
  for (const tool of ['clang-cl', 'lld-link', 'llvm-lib', 'llvm-rc']) versions[tool] = probe(join(env.WRITER_LLVM_ROOT, 'bin', tool), [tool === 'llvm-rc' ? '/?' : '--version'], 'Install clang, lld and llvm (Ubuntu: clang-18 lld-18 llvm-18); set WRITER_LLVM_ROOT=/usr/lib/llvm-18.').split('\n')[0];
  for (const file of ['crt/include/vcruntime.h', 'sdk/include/ucrt/stdio.h', 'sdk/include/um/Windows.h', 'sdk/include/shared/windef.h', 'crt/lib/x86_64/libcmt.lib', 'sdk/lib/ucrt/x86_64/ucrt.lib', 'sdk/lib/um/x86_64/kernel32.lib']) await required(join(env.WRITER_WINDOWS_SDK, file), 'Provision CRT/SDK using xwin splat; see setup guide.');
  versions.cc = probe('cc', ['--version'], 'Ubuntu: sudo apt install build-essential').split('\n')[0];
  versions.rust = probe('rustc', ['--version'], 'Install stable Rust from https://rustup.rs.');
  versions.cargo = probe('cargo', ['--version'], 'Install stable Rust from https://rustup.rs.');
  if (!probe('rustup', ['target', 'list', '--installed'], 'Install rustup.').split('\n').includes(target)) throw Error(`Run rustup target add ${target}`);
  versions.pnpm = probe('pnpm', ['--version'], 'Run corepack enable and corepack prepare pnpm@9.0.0 --activate.');
  if (versions.pnpm !== '9.0.0') throw Error('Use the repository packageManager pin: corepack prepare pnpm@9.0.0 --activate.');
  versions.pkgConfig = probe('pkg-config', ['--version'], 'Ubuntu: sudo apt install pkg-config libayatana-appindicator3-dev');
  probe('pkg-config', ['--libs', 'ayatana-appindicator3-0.1'], 'Ubuntu: sudo apt install libayatana-appindicator3-dev (Tauri CLI cross-bundle discovery).');
  env.TAURI_LINUX_AYATANA_APPINDICATOR = '1';
  env.WRITER_MAKENSIS = env.WRITER_MAKENSIS ? resolve(env.WRITER_MAKENSIS) : probe('sh', ['-c', 'command -v makensis'], 'Ubuntu: sudo apt install nsis nsis-common');
  env.WRITER_NSIS_ROOT = resolve(env.WRITER_NSIS_ROOT ?? '/usr/share/nsis');
  env.NSISDIR = env.WRITER_NSIS_ROOT;
  env.NSISCONFDIR = env.WRITER_NSIS_ROOT;
  versions.nsis = probe(env.WRITER_MAKENSIS, ['-VERSION'], 'Ubuntu: sudo apt install nsis nsis-common; set WRITER_NSIS_ROOT to its complete share/nsis directory.');
  for (const file of ['Stubs/zlib-x86-unicode', 'Stubs/lzma-x86-unicode', 'Include/MUI2.nsh', 'Include/MultiUser.nsh', 'Plugins/x86-unicode/System.dll']) await required(join(env.WRITER_NSIS_ROOT, file), 'Install the full nsis and nsis-common packages.');
  // NSIS before 3.12 is affected by CVE-2026-42171 (MW-SEC-26 W-08); releases need 3.12+.
  const [major = 0, minor = 0] = (versions.nsis.match(/^v?(\d+)\.(\d+)/) ?? []).slice(1).map(Number);
  if (release && (major < 3 || (major === 3 && minor < 12))) throw Error(`Release installers need NSIS 3.12 or later; found ${versions.nsis}. See docs/features/tauri-desktop.md#nsis-312.`);
  const lockBytes = await readFile(join(repo, 'scripts/desktop/windows-runtime.lock.json'));
  const lock = JSON.parse(lockBytes);
  await verifyRuntime(source, lock);
  const cargo = await readFile(join(repo, 'apps/desktop/tauri/Cargo.toml'),'utf8');
  if (!cargo.includes(`version = "${config.version}"`)) throw Error('Desktop Cargo and Tauri versions differ. Update both before building.');
  console.log(`Preflight passed: toolchain ready; ${lock.files.length} pinned runtime/notice inputs verified.`);
  // The full build generates the same set in build.mjs, after pnpm install.
  if (check) { await previewNotices({ repo, target, lock, ffmpegArchive: ffmpegSource }); process.exit(0); }
  const local = join(repo, '.local');
  await mkdir(local, { recursive: true });
  const staging = await mkdtemp(join(local, 'windows-installer-'));
  await stageRuntime(source, staging, lock);
  const bin = join(staging, 'bin');
  await mkdir(bin);
  await symlink(join(repo, 'scripts/desktop/nsis.sh'), join(bin, 'makensis'));
  env.PATH = `${bin}:${env.PATH}`;
  // Separate output per invocation: stale installers can never get a fresh receipt.
  env.CARGO_TARGET_DIR = join(staging, 'target');
  const head = probe('git', ['rev-parse', 'HEAD'], 'Build from a Git checkout.');
  const diff = probe('git', ['diff', 'HEAD', '--binary'], 'Git diff must work.');
  const status = probe('git', ['status', '--porcelain'], 'Git status must work.');
  await run('pnpm', ['install', '--frozen-lockfile']);
  if (release) {
    const signingProbe = join(staging,'signing-check.txt');
    await writeFile(signingProbe,'Writer release signing preflight');
    delete env.TAURI_SIGNING_PRIVATE_KEY;
    env.TAURI_SIGNING_PRIVATE_KEY_PATH = resolve(env.WRITER_SIGNING_KEY_FILE);
    probe(process.execPath,[join(repo,'node_modules/@tauri-apps/cli/tauri.js'),'signer','sign',signingProbe],'Signing preflight failed; check the private key and password.');
    verifySignature(await readFile(signingProbe),await readFile(`${signingProbe}.sig`,'utf8'),config.plugins.updater.pubkey);
    delete env.TAURI_SIGNING_PRIVATE_KEY_PATH;
    env.TAURI_SIGNING_PRIVATE_KEY = resolve(env.WRITER_SIGNING_KEY_FILE);
  }
  await run(process.execPath, [join(repo, 'scripts/desktop/build.mjs'), '--resources', staging, '--target', target, '--runner', join(repo, 'scripts/desktop/cross-cargo.sh'), ...(ffmpegSource ? ['--ffmpeg-source', ffmpegSource] : []), ...(signInConfig ? ['--google-sign-in', signInConfig] : []), ...(release ? ['--release'] : []), ...(withoutAuthenticode ? [WITHOUT_AUTHENTICODE] : [])]);
  const installer = join(env.CARGO_TARGET_DIR, target, 'release/bundle/nsis', `${config.productName}_${config.version}_x64-setup.exe`);
  const executable = join(env.CARGO_TARGET_DIR, target, 'release/tricktic-dictate.exe');
  // Read-only checks after all resource edits, NSIS compression and signing.
  const authenticode = windowsSigner ? { executable: await windowsSigner.verify(executable), installer: await windowsSigner.verify(installer) } : null;
  const bytes = await readFile(installer);
  const executableBytes = await readFile(executable);
  if (authenticode && (authenticode.installer.sha256 !== sha256(bytes) || authenticode.executable.sha256 !== sha256(executableBytes))) throw Error('Artifacts changed after final Authenticode verification; no receipt emitted.');
  const sourceChanged = probe('git',['rev-parse','HEAD'],'Git unavailable') !== head || probe('git',['diff','HEAD','--binary'],'Git unavailable') !== diff || probe('git',['status','--porcelain'],'Git unavailable') !== status;
  if (release && sourceChanged) throw Error('Source changed during release build; no release receipt emitted.');
  if (release) verifySignature(bytes,await readFile(`${installer}.sig`,'utf8'),config.plugins.updater.pubkey);
  const signature = release ? { bytes: (await readFile(`${installer}.sig`)).length, sha256: sha256(await readFile(`${installer}.sig`)) } : null;
  const receipt = { version: 1, googleSignIn: !!signInConfig, releaseType, authenticode, ...(releaseType === 'public-windows-without-authenticode' ? { authenticodeOmitted: true } : {}), appVersion: config.version, sourceChanged, signature, builtAt: new Date().toISOString(), commit: head, dirty: !!status, diffSha256: sha256(diff), target, tools: versions, runtimeLockSha256: sha256(lockBytes), inputs: lock.files, artifact: { path: installer, bytes: bytes.length, sha256: sha256(bytes) }, executable: { bytes: executableBytes.length, sha256: sha256(executableBytes) } };
  if (release) {
    Object.assign(receipt, await writeReleaseEnvelopes({ artifact: installer, version: config.version,
      platform: 'windows-x86_64', publicKey: config.plugins.updater.pubkey,
      sign: async path => {
        delete env.TAURI_SIGNING_PRIVATE_KEY;
        env.TAURI_SIGNING_PRIVATE_KEY_PATH = resolve(env.WRITER_SIGNING_KEY_FILE);
        probe(process.execPath, [join(repo, 'node_modules/@tauri-apps/cli/tauri.js'), 'signer', 'sign', path], 'Release metadata signing failed.');
      } }));
  }
  const receiptPath = `${installer}.receipt.json`;
  await writeFile(receiptPath, JSON.stringify(receipt, null, 2) + '\n');
  console.log(`Installer: ${installer}\nReceipt: ${receiptPath}\nSHA-256: ${receipt.artifact.sha256}`);
} catch (error) {
  console.error(`Desktop installer: ${error.message}`);
  process.exitCode = 1;
}
