import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { textModels, modelNotices, webNotices, webPackages, assertRuntimeNotices, ffmpegSource } from './notices.mjs';

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const lock = async platform => JSON.parse(await readFile(join(repo, `scripts/desktop/${platform}-runtime.lock.json`), 'utf8'));

async function fixture(files, run) {
  const root = await mkdtemp(join(tmpdir(), 'writer-notices-test-'));
  try {
    for (const [path, content] of Object.entries(files)) {
      await mkdir(dirname(join(root, path)), { recursive: true });
      await writeFile(join(root, path), typeof content === 'string' ? content : JSON.stringify(content));
    }
    await run(root);
  } finally { await rm(root, { recursive: true, force: true }); }
}

test('every downloadable model is listed with its license, and Parakeet with its CC BY attribution', async () => {
  const source = await readFile(join(repo, 'apps/api/src/cleanup/local_text/mod.rs'), 'utf8');
  const text = textModels(source);
  assert.equal(text.length, source.split('ModelSpec {').length - 1);
  const speech = JSON.parse(await readFile(join(repo, 'apps/api/src/desktop_settings/model-catalog.json'), 'utf8'));
  const notices = await modelNotices(repo);
  for (const model of [...speech, ...text]) assert.ok(notices.includes(`(${model.id})\n${model.source_url}\nLicense: `), model.id);
  assert.match(notices, /Parakeet TDT 0\.6B v3 by NVIDIA Corporation, licensed under CC BY 4\.0/);
  assert.match(notices, /=== CC-BY-4\.0\.txt ===\n\nAttribution 4\.0 International/);
  assert.match(notices, /Copyright \(c\) 2022 OpenAI/);
});

test('a model without a bundled license text or a CC BY model without attribution fails the build', async () => {
  const model = { id: 'm', display_name: 'M', source_name: 'someone/m', source_url: 'https://example.invalid/m', license: 'mit' };
  const rust = 'pub const MODELS: &[ModelSpec] = &[];';
  await fixture({ 'apps/api/src/desktop_settings/model-catalog.json': [model], 'apps/api/src/cleanup/local_text/mod.rs': rust }, async root => {
    await assert.rejects(modelNotices(root), /no license text for m/);
  });
  await fixture({ 'apps/api/src/desktop_settings/model-catalog.json': [{ ...model, license: 'CC-BY-4.0' }], 'apps/api/src/cleanup/local_text/mod.rs': rust }, async root => {
    await assert.rejects(modelNotices(root), /CC BY 4\.0 but has no attribution/);
  });
});

test('web notices cover the bundled dependency tree and fail on a package without a license file', async () => {
  const names = (await webPackages(repo)).map(pkg => pkg.name);
  for (const name of ['react', 'react-dom', 'scheduler', 'lucide-react', '@fontsource/inter', '@fontsource/assistant', 'vite']) assert.ok(names.includes(name), name);
  assert.ok(!names.some(name => name.startsWith('@metrix-writer/')));
  assert.match(await webNotices(repo), /=== @fontsource\/inter [^ ]+ \(OFL-1\.1\) ===/);
  // Vendored files carry their licence too: the provider marks are lobe-icons.
  assert.match(await webNotices(repo), /=== lobe-icons \(MIT\), provider and model-maker marks ===[\s\S]*Copyright \(c\) 2023 LobeHub/);
  const manifest = (name, extra = {}) => ({ name, version: '1.0.0', license: 'MIT', ...extra });
  await fixture({
    'apps/web/package.json': manifest('@metrix-writer/web', { private: true, dependencies: { a: '1.0.0' } }),
    'apps/web/node_modules/a/package.json': manifest('a'),
    'apps/web/node_modules/vite/package.json': manifest('vite'),
    'apps/web/node_modules/vite/LICENSE.md': 'MIT',
  }, async root => {
    await assert.rejects(webNotices(root), /a 1\.0\.0 has no license file/);
  });
});

test('each runtime family must ship with its license notice', async () => {
  for (const platform of ['windows', 'macos']) assertRuntimeNotices((await lock(platform)).files.map(entry => entry.path));
  assert.throws(() => assertRuntimeNotices(['runtime/ffmpeg/ffmpeg.exe', 'notices/ffmpeg-LICENSE.txt']), /runtime\/ffmpeg\/ without its license/);
  assert.throws(() => assertRuntimeNotices(['runtime/nemo/cpu/bin/nemo-speech.exe']), /runtime\/nemo\//);
  assertRuntimeNotices([]);
});

test('the FFmpeg source must be the pinned archive', async () => {
  await fixture({ 'ffmpeg-8.0.3.tar.xz': 'not the release' }, async root => {
    await assert.rejects(ffmpegSource(repo, join(root, 'ffmpeg-8.0.3.tar.xz')), /differs from the pin/);
  });
});
