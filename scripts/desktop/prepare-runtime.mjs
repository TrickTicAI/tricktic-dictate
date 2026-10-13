// Combine retained GPU/ffmpeg resources with the independently built CPU package.
// All bytes are checked against the committed lock; weights are never copied.
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { sha256, verifyRuntime } from './runtime.mjs';
const args = process.argv.slice(2);
const option = name => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : undefined; };
const retained = option('--resources'), cpu = option('--cpu'), output = option('--output');
const nemo = option('--nemo'), text = option('--text'), notices = option('--notices');
if (!retained || !cpu || !output || !nemo || !text || !notices) throw Error('Usage: node scripts/desktop/prepare-runtime.mjs --resources <retained runtime root> --cpu <build-cpu-runtime package> --nemo <verified NeMo packages> --text <verified llama packages> --notices <llama license directory> --output <new private directory>');
const lock = JSON.parse(await readFile(new URL('./windows-runtime.lock.json', import.meta.url)));
const destination = resolve(output);
const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
if (destination === repo || destination.startsWith(repo + '/')) throw Error('Keep runtime resources outside source.');
// Refuse replacing an existing directory, including another task package.
await mkdir(destination);
for (const entry of lock.files) {
  const cpuFile = entry.path.startsWith('runtime/whisper/cpu/') ? entry.path.split('/').at(-1)
    : entry.path === 'notices/whisper-cpu-LICENSE.txt' ? 'LICENSE-whisper.txt' : undefined;
  let source = cpuFile ? resolve(cpu, cpuFile) : resolve(retained, entry.path);
  if (entry.path.startsWith('runtime/nemo/')) source = resolve(nemo, entry.path.slice('runtime/nemo/'.length));
  if (entry.path.startsWith('runtime/text/')) source = resolve(text, entry.path.slice('runtime/text/'.length));
  for (const backend of ['cpu', 'vulkan']) {
    if (entry.path.startsWith(`notices/nemo-${backend}/`)) source = resolve(nemo, backend, 'share/licenses', entry.path.slice(`notices/nemo-${backend}/`.length));
    if (entry.path.startsWith(`notices/text-${backend}/`)) source = resolve(text, backend, entry.path.slice(`notices/text-${backend}/`.length));
  }
  if (entry.path === 'notices/llama-LICENSE.txt') source = resolve(notices, 'llama-LICENSE.txt');
  const bytes = await readFile(source);
  if (bytes.length !== entry.bytes || sha256(bytes) !== entry.sha256) throw Error(`Pinned resource mismatch: ${entry.path}`);
  const target = resolve(destination, entry.path);
  await mkdir(dirname(target), { recursive: true });
  await writeFile(target, bytes, { flag: 'wx' });
}
await verifyRuntime(destination, lock);
console.log(`Prepared ${lock.files.length} verified CPU/GPU runtime and notice files.`);
