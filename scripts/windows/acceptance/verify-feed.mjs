#!/usr/bin/env node
// MW-SEC-21: verify a published Windows update feed the way an installed client
// would: signed metadata, exact size/hash, minisign signature and freshness.
// Read-only. Usage: node verify-feed.mjs <feed-url> <download-dir> <receipt.json>
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { verifySignature } from '../../desktop/signature.mjs';
import { verifyMetadata } from '../../desktop/update-metadata.mjs';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const [feedUrl, downloads, out] = process.argv.slice(2);
if (!feedUrl || !downloads || !out) throw Error('usage: verify-feed.mjs <feed-url> <download-dir> <receipt.json>');
const pubkey = JSON.parse(await readFile(join(ROOT, 'apps/desktop/tauri/tauri.conf.json'), 'utf8')).plugins.updater.pubkey;
const get = async url => {
  const response = await fetch(url, { redirect: 'error', signal: AbortSignal.timeout(15 * 60_000) });
  if (response.status !== 200) throw Error(`${url}: HTTP ${response.status}`);
  return { bytes: Buffer.from(await response.arrayBuffer()), cacheControl: response.headers.get('cache-control') };
};
const feed = await get(feedUrl);
const doc = JSON.parse(feed.bytes.toString('utf8'));
const entry = doc.platforms?.['windows-x86_64'];
if (!entry) throw Error('feed has no windows-x86_64 entry');
const url = new URL(entry.url);
const artifact = await get(url.href);
const sha256 = createHash('sha256').update(artifact.bytes).digest('hex');
const checks = {};
const run = (name, fn) => { try { checks[name] = fn() ?? true; } catch (error) { checks[name] = `FAILED: ${error.message}`; } };
run('feed read under the 64 KiB client ceiling', () => { if (feed.bytes.length > 65536) throw Error(`${feed.bytes.length} bytes`); });
run('artifact URL is the fixed content-addressed HTTPS path', () => {
  if (url.protocol !== 'https:' || url.host !== 'updates.tricktic.com' || url.search || url.hash
      || url.pathname !== `/writer/releases/${doc.version}/${sha256}/setup.exe`) throw Error(url.href);
});
run('minisign signature over the artifact verifies', () => verifySignature(artifact.bytes, entry.signature, pubkey));
run('signed release metadata binds version, hash, size and is fresh', () => verifyMetadata(entry.writer_release, artifact.bytes, doc.version, 'windows-x86_64', pubkey));
const name = `setup-${doc.version}-${sha256.slice(0, 12)}.exe`;
await writeFile(join(downloads, name), artifact.bytes, { flag: 'wx' }).catch(error => { if (error.code !== 'EEXIST') throw error; });
const passed = Object.values(checks).every(value => typeof value !== 'string' || !value.startsWith('FAILED'));
const result = { schema: 1, kind: 'mw-sec-21-feed-verification', checked_at: new Date().toISOString(), feed: feedUrl,
  feed_sha256: createHash('sha256').update(feed.bytes).digest('hex'), feed_cache_control: feed.cacheControl,
  version: doc.version, pub_date: doc.pub_date, artifact: { url: url.href, bytes: artifact.bytes.length, sha256, saved_as: name },
  passed, checks };
await writeFile(out, `${JSON.stringify(result, null, 2)}\n`, { flag: 'wx' });
console.log(`${feedUrl}: ${doc.version} ${passed ? 'verified' : 'FAILED'}`);
process.exitCode = passed ? 0 : 1;
