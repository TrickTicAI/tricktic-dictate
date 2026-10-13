import { mkdir, copyFile, writeFile, readFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { spawnSync, spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
if (process.platform !== 'darwin' || process.argv.length !== 4) throw Error('Run on macOS: node security-smoke-macos.mjs <instrumented-executable> <new-output-directory>');
const executable = resolve(process.argv[2]), output = resolve(process.argv[3]);
await mkdir(output); // Refuse existing profiles/artifacts.
const app = join(output, 'WriterSecurity.app');
await mkdir(join(app, 'Contents/MacOS'), { recursive: true });
await mkdir(join(app, 'Contents/Resources'));
const copied = join(app, 'Contents/MacOS/tricktic-dictate');
await copyFile(executable, copied);
await writeFile(join(app, 'Contents/Info.plist'), `<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleExecutable</key><string>tricktic-dictate</string><key>CFBundleIdentifier</key><string>com.tricktic.dictate.security-fixture</string><key>CFBundleName</key><string>Writer Security Fixture</string><key>CFBundlePackageType</key><string>APPL</string><key>LSMinimumSystemVersion</key><string>15.0</string></dict></plist>`);
const signed = spawnSync('codesign', ['--force', '--sign', '-', app], { encoding: 'utf8' });
if (signed.status !== 0) throw Error('Could not ad-hoc sign private fixture bundle.');
const root = join(output, 'profile');
const env = { ...process.env };
for (const key of ['OPENAI_API_KEY', 'GEMINI_API_KEY', 'XAI_API_KEY', 'OPENROUTER_API_KEY']) delete env[key];
const child = spawn(copied, ['--security-probe', root], { env, stdio: ['ignore', 'pipe', 'pipe'] });
let stdout = '', stderr = '';
child.stdout.on('data', bytes => { stdout += bytes; });
child.stderr.on('data', bytes => { stderr += bytes; });
let timedOut = false;
const timeout = setTimeout(() => { timedOut = true; child.kill('SIGKILL'); }, 65000);
const { code, signal } = await new Promise((ok, fail) => {
  child.once('error', fail);
  child.once('exit', (code, signal) => ok({ code, signal }));
});
clearTimeout(timeout);
await writeFile(join(output, 'exit.json'), JSON.stringify({ code, signal, timedOut }));
await writeFile(join(output, 'stdout.txt'), stdout);
await writeFile(join(output, 'stderr.txt'), stderr);
let receipt;
try {
  receipt = JSON.parse(await readFile(join(root, 'security-probe.json'), 'utf8'));
} catch (error) {
  if (error.code !== 'ENOENT') throw error;
  // Preserve the last completed step and every finished window, even if a
  // navigation or timeout prevented the aggregate receipt from being written.
  const partial = {};
  for (const label of ['main', 'overlay', 'tray-menu', 'settings']) {
    for (const kind of ['progress', 'result']) {
      try {
        partial[`${kind}:${label}`] = JSON.parse(await readFile(join(root, `security-${kind}-${label}.json`), 'utf8'));
      } catch (error) {
        if (error.code !== 'ENOENT') throw error;
      }
    }
  }
  receipt = { passed: false, final_receipt_missing: true, code, signal, timedOut, partial };
}
receipt.executable_sha256 = createHash('sha256').update(await readFile(copied)).digest('hex');
await writeFile(join(output, 'receipt.json'), `${JSON.stringify(receipt, null, 2)}\n`);
if (code !== 0 || !receipt.passed || receipt.popup_count !== 0) throw Error('WKWebView security fixture failed; inspect receipt.json.');
console.log('Packaged WKWebView security fixture passed.');
