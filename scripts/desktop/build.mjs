// Bundled resources are constrained by the committed platform runtime lock.
import { readFile, writeFile, chmod, mkdir, realpath, mkdtemp } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { resolve, dirname, relative, sep, join } from 'node:path';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { verifyRuntime } from './runtime.mjs';
import { verifySpeechProtocol } from './speech-runtime.mjs';
import { writeNotices } from './notices.mjs';
import { createWindowsSigner, windowsSignCommand, signWindowsRuntime, windowsReleaseType, windowsReleaseBanner, WITHOUT_AUTHENTICODE } from './windows-signing.mjs';
import { remapEnv } from './path-remap.mjs';
import { assertSupportedDesktopResources, assertNoDesktopFixture, assertNoGoogleSignIn, assertGoogleSignIn, desktopSignInConfig, googleSignInEnv, GOOGLE_SIGN_IN_ENV } from './security-artifact.mjs';
const repo=resolve(dirname(fileURLToPath(import.meta.url)),'../..');
const args=process.argv.slice(2), option=name=>{const i=args.indexOf(name);return i<0?undefined:args[i+1];};
const resources=option('--resources');
const target=option('--target');
const windows=target?.includes('windows') ?? process.platform==='win32';
if (!windows && args.includes(WITHOUT_AUTHENTICODE)) throw Error(`${WITHOUT_AUTHENTICODE} applies only to Windows releases.`);
const releaseType = windows ? windowsReleaseType({ release: args.includes('--release'), withoutAuthenticode: args.includes(WITHOUT_AUTHENTICODE) }) : null;
const publicWindowsRelease = releaseType === 'public-windows';
const windowsSigner = publicWindowsRelease ? createWindowsSigner() : null;
if (windows && args.includes('--release') && (args.includes('--no-bundle') || args.includes('--debug'))) throw Error('Public Windows releases require a complete release bundle. Omit --release for unsigned developer previews.');
if (windowsSigner) await windowsSigner.preflight();
// Google sign-in is required in desktop releases. The private
// configuration is compiled in through the environment, never through source.
if(args.includes('--google-sign-in')&&!option('--google-sign-in'))throw Error('--google-sign-in requires a private configuration path.');
const signInConfig=desktopSignInConfig({path:option('--google-sign-in'),release:args.includes('--release')});
const signInEnv=signInConfig?await googleSignInEnv({path:resolve(signInConfig),windows,release:args.includes('--release')}):{};
const cargoEnv={...process.env};
for(const name of GOOGLE_SIGN_IN_ENV)delete cargoEnv[name];
Object.assign(cargoEnv,signInEnv);
if(args.includes('--release'))cargoEnv.WRITER_PUBLIC_RELEASE='1';else delete cargoEnv.WRITER_PUBLIC_RELEASE;
// Neutral source, registry, toolchain and target paths in every desktop build (MW-RELEASE-14).
Object.assign(cargoEnv,remapEnv({repo,env:cargoEnv,target,windows}));
if (windows) console.log(windowsReleaseBanner(releaseType));
await mkdir(resolve(repo,'.local'),{recursive:true});
const staging=await mkdtemp(resolve(repo,'.local/tauri-package-'));
const files={}, verified=[], included=new Set();
if(resources){
  const source=await realpath(resources);
  const manifest=JSON.parse(await readFile(resolve(repo, windows ? 'scripts/desktop/windows-runtime.lock.json' : 'scripts/desktop/macos-runtime.lock.json'),'utf8'));
  await verifyRuntime(source,manifest);
  if (!windows) await verifySpeechProtocol(source, manifest, resolve(repo, 'runtime/whisper/writer-auth.h'));
  if(manifest.version!==1 || !Array.isArray(manifest.files))throw new Error('Invalid runtime manifest.');
  for(const entry of manifest.files){
    if(!['runtime','notice'].includes(entry.role) && !entry.path.startsWith('notices/'))continue;
    if(!/^(runtime|notices)\/[a-zA-Z0-9._/-]+$/.test(entry.path)||entry.path.split('/').includes('..')||included.has(entry.path))throw new Error('Invalid or duplicate runtime path.');
    included.add(entry.path);
    const file=await realpath(resolve(source,entry.path));
    if(relative(source,file).startsWith(`..${sep}`))throw new Error('Runtime escapes its package.');
    const bytes=await readFile(file);
    if(bytes.length!==entry.bytes||createHash('sha256').update(bytes).digest('hex')!==entry.sha256)throw new Error(`Runtime hash mismatch: ${entry.path}`);
    const destination=resolve(staging,entry.path);await mkdir(dirname(destination),{recursive:true});await writeFile(destination,bytes);if(entry.executable === true)await chmod(destination,0o755);
    files[destination]=entry.path;verified.push({path:entry.path,bytes:entry.bytes,sha256:entry.sha256});
  }
  if(windows)for(const required of ['runtime/whisper/whisper-server.exe','runtime/whisper/ggml-vulkan.dll','runtime/ffmpeg/ffmpeg.exe','runtime/whisper/cpu/whisper-server.exe','runtime/whisper/cpu/ggml-cpu-x64.dll'])if(!included.has(required))throw new Error(`Missing Windows runtime resource: ${required}`);
}else if(windows&&!args.includes('--no-bundle'))throw new Error('Pass --resources <verified Windows runtime package>. Model weights are never bundled.');
if (resources && windowsSigner) {
  const signed = await signWindowsRuntime(staging, verified, windowsSigner);
  verified.splice(0, verified.length, ...signed);
}
if (resources && !windows) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64' || (target && target !== 'aarch64-apple-darwin')) throw Error('Local macOS runtime packaging requires an Apple Silicon Mac.');
  const { signMacRuntime } = await import('./macos-runtime.mjs');
  const { signingIdentity, developerId } = await import('./macos-signing.mjs');
  // Nested runtimes use the same identity Tauri receives for the app bundle.
  const identity = process.env.APPLE_SIGNING_IDENTITY ?? signingIdentity;
  if (![signingIdentity, developerId.identity].includes(identity)) throw Error(`Unsupported macOS signing identity: ${identity}`);
  const lock = JSON.parse(await readFile(resolve(repo, 'scripts/desktop/macos-runtime.lock.json')));
  const signed = await signMacRuntime(staging, lock, undefined, identity);
  verified.splice(0, verified.length, ...signed.files);
}
// Rust, web, model and FFmpeg-source notices beside the runtime's own (docs/release/notices.md).
Object.assign(files,await writeNotices({repo,staging,target:target ?? (windows?'x86_64-pc-windows-msvc':'aarch64-apple-darwin'),runtime:[...included],ffmpegArchive:option('--ffmpeg-source')&&resolve(option('--ffmpeg-source'))}));
if(resources){const manifest=resolve(staging,'runtime-manifest.json');await writeFile(manifest,JSON.stringify(verified));files[manifest]='runtime-manifest.json';}
assertSupportedDesktopResources(Object.values(files));
// Tauri signs the app before bundling, then the final NSIS installer before minisign.
const config=resolve(staging,'bundle.json');await writeFile(config,JSON.stringify({bundle:{resources:files,targets:windows?['nsis']:['app','dmg'],createUpdaterArtifacts:args.includes('--release'),...(publicWindowsRelease?{windows:{signCommand:windowsSignCommand()}}:{})}}));
async function run(command,argv,cwd=repo,env=process.env){await new Promise((ok,fail)=>{const child=spawn(command,argv,{cwd,env,stdio:'inherit'});child.on('error',fail);child.on('exit',code=>code===0?ok():fail(new Error(`Build exited ${code}.`)));});}
const pnpm=process.env.npm_execpath;
if(pnpm)await run(process.execPath,[pnpm,'build:web']);
else { const { run: start }=await import('../dev/process.mjs'); const child=start('pnpm',['build:web']); await new Promise((ok,fail)=>{child.on('error',fail);child.on('exit',c=>c===0?ok():fail(new Error('Web build failed.')));}); }
const cli=[resolve(repo,'node_modules/@tauri-apps/cli/tauri.js'),'build','--config',config,'--ci'];
if(target)cli.push('--target',target);if(option('--runner'))cli.push('--runner',option('--runner'));if(args.includes('--debug'))cli.push('--debug');if(args.includes('--no-bundle'))cli.push('--no-bundle');
if(signInConfig)cli.push('--features','google-sign-in');
cli.push('--','--locked');
await run(process.execPath,cli,resolve(repo,'apps/desktop/tauri'),cargoEnv);
// A later bundler mutation must not silently invalidate the shipped runtime manifest.
if (windowsSigner) await verifyRuntime(staging, { version: 1, files: verified });

const profile = args.includes('--debug') ? 'debug' : 'release';
const output = resolve(process.env.CARGO_TARGET_DIR ?? resolve(repo, 'target'), ...(target ? [target] : []), profile, windows ? 'tricktic-dictate.exe' : 'tricktic-dictate');
await assertNoDesktopFixture(output);
// No build-machine paths or owner names in the executable or shipped resources (MW-RELEASE-14).
// The web bundle is embedded Brotli-compressed, so scan it before compression too.
// Releases always run this; other builds run it when the owner identity is set up.
if (args.includes('--release') || existsSync(process.env.WRITER_RELEASE_IDENTITY ?? join(homedir(), '.config/tricktic/release-identity.json'))) {
  await run(process.env.WRITER_PYTHON ?? (process.platform === 'win32' ? 'python' : 'python3'), [resolve(repo, 'scripts/security/personal_paths.py'), resolve(repo, 'apps/web/dist'), output, ...Object.keys(files)]);
}
if (signInConfig) await assertGoogleSignIn(output);
else await assertNoGoogleSignIn(output);
