// License notices shipped in the desktop app's notices/ folder (MW-RELEASE-4).
// Every generator throws when a required source is missing, so a build cannot
// ship without it. docs/release/notices.md lists each component and obligation.
import { readFile, writeFile, readdir, mkdir, realpath, stat } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { resolve, dirname, join, basename } from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const LICENSE_FILE = /^(license|licence|copying|notice)([-_.].*)?$/i;
const NOTICE_TEXTS = 'apps/desktop/tauri/notices';

/** Rust crates linked into the desktop binary, with the license files each crate ships. */
export async function rustNotices(repo, target) {
  const { stdout } = await promisify(execFile)('cargo', ['metadata', '--locked', '--format-version', '1', '--filter-platform', target], { cwd: repo, maxBuffer: 32 * 1024 * 1024 });
  const out = [];
  for (const pkg of JSON.parse(stdout).packages) {
    if (!pkg.source) continue;
    const folder = dirname(pkg.manifest_path);
    const names = (await readdir(folder)).filter(name => LICENSE_FILE.test(name));
    out.push(`\n=== ${pkg.name} ${pkg.version} (${pkg.license ?? 'see upstream license'}) ===\n${pkg.repository ?? ''}\n`);
    for (const name of names) { try { out.push(await readFile(resolve(folder, name), 'utf8')); } catch { /* Some crates use a license directory; their SPDX identifier and upstream URL remain recorded. */ } }
  }
  return out.join('\n');
}

async function findPackage(from, name) {
  for (let dir = from; ; dir = dirname(dir)) {
    const candidate = join(dir, 'node_modules', name);
    try { await stat(join(candidate, 'package.json')); return await realpath(candidate); } catch { /* keep walking */ }
    if (dirname(dir) === dir) throw Error(`Web notices: ${name} is not installed. Run pnpm install --frozen-lockfile.`);
  }
}

/**
 * Packages whose code or files are in apps/web/dist: the web app's production
 * dependency tree, plus Vite, which injects its module-preload helper. Workspace
 * packages are this repository's own code and are walked, not listed.
 */
export async function webPackages(repo) {
  const web = resolve(repo, 'apps/web');
  const found = new Map();
  const visit = async (dir, manifest, own) => {
    if (!own) {
      const key = `${manifest.name}@${manifest.version}`;
      if (found.has(key)) return;
      found.set(key, { name: manifest.name, version: manifest.version, license: manifest.license, repository: manifest.repository?.url ?? manifest.repository, dir });
    }
    for (const name of Object.keys(manifest.dependencies ?? {})) {
      const child = await findPackage(dir, name);
      const childManifest = JSON.parse(await readFile(join(child, 'package.json'), 'utf8'));
      await visit(child, childManifest, childManifest.private === true && childManifest.name?.startsWith('@metrix-writer/'));
    }
  };
  const manifest = JSON.parse(await readFile(join(web, 'package.json'), 'utf8'));
  await visit(web, manifest, true);
  const vite = await findPackage(web, 'vite');
  await visit(vite, { ...JSON.parse(await readFile(join(vite, 'package.json'), 'utf8')), dependencies: {} }, false);
  return [...found.values()].sort((a, b) => a.name.localeCompare(b.name));
}

/** Files copied into the web app rather than installed as a package, with their licence. */
export const VENDORED = [
  { name: 'lobe-icons', license: 'MIT', repository: 'https://github.com/lobehub/lobe-icons', what: 'provider and model-maker marks', file: 'apps/web/notices/lobe-icons-LICENSE.txt' },
];

export async function webNotices(repo) {
  const out = ['Third-party code and fonts in the desktop window (apps/web/dist).\n'];
  for (const pkg of await webPackages(repo)) {
    const names = (await readdir(pkg.dir)).filter(name => LICENSE_FILE.test(name)).sort();
    if (!pkg.license || !names.length) throw Error(`Web notices: ${pkg.name} ${pkg.version} has no license file. Add its license before shipping it.`);
    out.push(`\n=== ${pkg.name} ${pkg.version} (${pkg.license}) ===\n${pkg.repository ?? ''}\n`);
    for (const name of names) out.push(await readFile(join(pkg.dir, name), 'utf8'));
  }
  for (const item of VENDORED) {
    out.push(`\n=== ${item.name} (${item.license}), ${item.what} ===\n${item.repository}\n`);
    out.push(await readFile(resolve(repo, item.file), 'utf8'));
  }
  return out.join('\n');
}

/** Text models are Rust constants; read the same fields the API serves. */
export function textModels(source) {
  const blocks = source.split('ModelSpec {').slice(1);
  return blocks.map(block => {
    const field = name => block.match(new RegExp(`\\b${name}: "([^"]+)"`))?.[1];
    const model = { id: field('id'), display_name: field('display_name'), source_name: field('source_name'), source_url: field('source_url'), license: field('license')?.toLowerCase() };
    if (Object.values(model).some(value => !value)) throw Error('Model notices: a text model is missing its id, source or license.');
    return model;
  });
}

// The license text shipped for each downloadable model. MIT copyright lines
// differ by publisher, so MIT models are matched on their source.
function modelLicense(model) {
  if (model.license === 'apache-2.0') return 'Apache-2.0.txt';
  if (model.license === 'cc-by-4.0') {
    if (!model.attribution?.includes('creativecommons.org/licenses/by/4.0')) throw Error(`Model notices: ${model.id} is CC BY 4.0 but has no attribution.`);
    return 'CC-BY-4.0.txt';
  }
  if (model.license === 'mit' && model.source_name.startsWith('distil-whisper/')) return 'distil-whisper-MIT.txt';
  if (model.license === 'mit' && model.source_name === 'ggerganov/whisper.cpp') return 'openai-whisper-MIT.txt';
  throw Error(`Model notices: no license text for ${model.id} (${model.license} from ${model.source_name}).`);
}

export async function modelNotices(repo) {
  const speech = JSON.parse(await readFile(resolve(repo, 'apps/api/src/desktop_settings/model-catalog.json'), 'utf8'));
  const text = textModels(await readFile(resolve(repo, 'apps/api/src/cleanup/local_text/mod.rs'), 'utf8'));
  const models = [...speech, ...text].map(m => ({ ...m, license: m.license.toLowerCase() }));
  const texts = new Set();
  const out = ['Models you can download in the app. None ships in the installer; each is downloaded from its',
    'publisher at a pinned revision and checked against a SHA-256 hash.\n'];
  for (const model of models) {
    const file = modelLicense(model);
    texts.add(file);
    out.push(`${model.display_name} (${model.id})\n${model.source_url}\nLicense: ${{ mit: 'MIT', 'apache-2.0': 'Apache-2.0', 'cc-by-4.0': 'CC-BY-4.0' }[model.license]}, text below as ${file}${model.attribution ? `\n${model.attribution}` : ''}\n`);
  }
  for (const file of [...texts].sort()) out.push(`\n=== ${file} ===\n\n${await readFile(resolve(repo, NOTICE_TEXTS, file), 'utf8')}`);
  return out.join('\n');
}

const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

/**
 * FFmpeg is LGPL-2.1-or-later; its exact source ships beside the binary. The
 * archive comes from --ffmpeg-source, WRITER_FFMPEG_SOURCE or .local/sources.
 */
export async function ffmpegSource(repo, archive = process.env.WRITER_FFMPEG_SOURCE) {
  const pin = JSON.parse(await readFile(resolve(repo, 'scripts/inference/ffmpeg-source.json'), 'utf8'));
  const name = basename(new URL(pin.source_url).pathname);
  const pinned = bytes => bytes.length === pin.bytes && sha256(bytes) === pin.sha256;
  let bytes;
  if (archive) {
    bytes = await readFile(archive);
  } else {
    // No path given: reuse or fill a cache of the pinned archive. The hash pin
    // is the signature-verified release, so the download is checked, not trusted.
    const cache = resolve(repo, '.local/sources', name);
    bytes = await readFile(cache).catch(() => null);
    if (!bytes || !pinned(bytes)) {
      console.log(`Fetching the pinned FFmpeg source ${pin.source_url}`);
      const response = await fetch(pin.source_url).catch(() => null);
      if (!response?.ok) throw Error(`FFmpeg ships in this package, so its source must too, and ${pin.source_url} could not be downloaded. Pass --ffmpeg-source <path to ${name}>.`);
      bytes = Buffer.from(await response.arrayBuffer());
      if (pinned(bytes)) { await mkdir(dirname(cache), { recursive: true }); await writeFile(cache, bytes); }
    }
  }
  if (!pinned(bytes)) throw Error(`The FFmpeg source differs from the pin in scripts/inference/ffmpeg-source.json (expected ${name}, ${pin.bytes} bytes, SHA-256 ${pin.sha256}).`);
  const readme = [`FFmpeg ${pin.version} source code`, '',
    `The ffmpeg program in runtime/ffmpeg is built from ${name} in this folder, without changes.`,
    'It is licensed under the GNU Lesser General Public License, version 2.1 or later; see',
    'notices/ffmpeg-LICENSE.md and notices/ffmpeg-LICENSE.txt. The configure options and',
    'compiler are in notices/ffmpeg-config.txt and notices/ffmpeg-build.json. The build does',
    'not enable GPL, version 3 or non-free components.', '',
    `Upstream: ${pin.source_url}`, `SHA-256:  ${pin.sha256}`, `Signed by FFmpeg release key ${pin.signer_fingerprint}.`, ''].join('\n');
  return { name, bytes, readme };
}

// The runtime families in a package and the license each one must ship with.
const RUNTIME_LICENSES = [
  ['runtime/ffmpeg/', /^notices\/ffmpeg-LICENSE\.md$/],
  ['runtime/whisper/', /^notices\/whisper(\.cpp|-cpu)?-LICENSE\.txt$/],
  ['runtime/text/', /^notices\/llama-LICENSE\.txt$/],
  ['runtime/nemo/', /^notices\/nemo-[a-z]+\/nemo-speech\/LICENSE$/],
];
export function assertRuntimeNotices(paths) {
  for (const [prefix, notice] of RUNTIME_LICENSES) {
    if (paths.some(path => path.startsWith(prefix)) && !paths.some(path => notice.test(path))) throw Error(`The runtime package ships ${prefix} without its license notice.`);
  }
}

const title = path => ({
  'notices/MODELS-NOTICES.txt': 'Downloadable models',
  'notices/WEB-NOTICES.txt': 'Interface libraries and fonts',
  'notices/RUST-NOTICES.txt': 'Application libraries (Rust)',
  'notices/source/README.txt': 'FFmpeg source code',
}[path] ?? path.replace(/^notices\//, ''));
const group = path => /MODELS/.test(path) ? 'Models' : /(WEB|RUST)-NOTICES/.test(path) ? 'Application' : 'Speech and text runtime';

/**
 * Writes the generated notices into <staging>/notices and returns the bundle
 * resource map for them, including index.json, which lists every notice the
 * in-app licenses view may read. `runtime` is the lock paths already staged.
 */
export async function writeNotices({ repo, staging, target, runtime = [], ffmpegArchive }) {
  assertRuntimeNotices(runtime);
  const files = {}, index = [];
  const add = async (path, content, kind = 'text') => {
    const destination = resolve(staging, path);
    await mkdir(dirname(destination), { recursive: true });
    await writeFile(destination, content);
    files[destination] = path;
    if (kind) index.push({ path, title: title(path), group: group(path), kind });
  };
  await add('notices/MODELS-NOTICES.txt', await modelNotices(repo));
  await add('notices/WEB-NOTICES.txt', await webNotices(repo));
  await add('notices/RUST-NOTICES.txt', await rustNotices(repo, target));
  for (const path of runtime.filter(path => path.startsWith('notices/')).sort()) index.push({ path, title: title(path), group: group(path), kind: 'text' });
  if (runtime.some(path => path.startsWith('runtime/ffmpeg/'))) {
    const source = await ffmpegSource(repo, ffmpegArchive);
    await add('notices/source/README.txt', source.readme);
    await add(`notices/source/${source.name}`, source.bytes, 'source');
  }
  await add('notices/index.json', JSON.stringify({ version: 1, notices: index }, null, 2), null);
  return files;
}

/** `desktop:installer --check`: generate the notice set that would ship and list it. */
export async function previewNotices({ repo, target, lock, ffmpegArchive }) {
  const { mkdtemp } = await import('node:fs/promises');
  await mkdir(resolve(repo, '.local'), { recursive: true });
  const staging = await mkdtemp(resolve(repo, '.local/notices-check-'));
  const runtime = (lock?.files ?? []).filter(entry => ['runtime', 'notice'].includes(entry.role) || entry.path.startsWith('notices/')).map(entry => entry.path);
  const files = await writeNotices({ repo, staging, target, runtime, ffmpegArchive });
  const index = JSON.parse(await readFile(resolve(staging, 'notices/index.json'), 'utf8')).notices;
  const sizes = Object.fromEntries(await Promise.all(Object.entries(files).map(async ([file, path]) => [path, (await stat(file)).size])));
  const pinned = Object.fromEntries((lock?.files ?? []).map(entry => [entry.path, entry.bytes]));
  console.log(`Notices (${index.length} listed in the app, generated in ${staging}):`);
  for (const entry of index) console.log(`  ${entry.path}  ${sizes[entry.path] ?? pinned[entry.path]} bytes  [${entry.group}${entry.kind === 'source' ? ', source code' : ''}]`);
  return staging;
}
