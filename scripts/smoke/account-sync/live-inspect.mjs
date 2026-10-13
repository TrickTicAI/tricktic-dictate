// Read-only owner inspection of the live Writer Firestore tree for MW-OUATH-11
// acceptance. It reports structure, counts, consent/fence state, portable
// setting values and SHA-256 prefixes of user text; it never prints text, UIDs,
// emails or tokens. It issues GET/list requests only.
//
// Auth: the owner's firebase-tools login from the pinned test tooling
// (`npm ci --prefix infra/firebase/emulator`, then `firebase login`). Owner
// credentials bypass Security Rules, so this proves what is stored, not what a
// client may read; rules isolation is proven by the emulator suites and clients.
//
//   node scripts/smoke/account-sync/live-inspect.mjs [--project metrix-writer] \
//     [--expect <utf8 file, one expected text per line>]
//
// --expect reports, per line, whether a live record holds exactly that text
// (hash equality after no normalization), to prove English/Hebrew/mixed round
// trips without echoing the text.
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';

const root = resolve(import.meta.dirname, '../../..');
const args = process.argv.slice(2);
const option = name => { const i = args.indexOf(name); return i < 0 ? undefined : args[i + 1]; };
const project = option('--project') ?? 'metrix-writer';
if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(project)) throw new Error('invalid --project');
const expectFile = option('--expect');

const lib = join(root, 'infra/firebase/emulator/node_modules/firebase-tools/lib');
const require = createRequire(import.meta.url);
const { configstore } = require(join(lib, 'configstore'));
const { requireAuth } = require(join(lib, 'requireAuth'));
const { Client } = require(join(lib, 'apiv2'));
if (!configstore.get('user')) throw new Error('Run the pinned `firebase login` first');
await requireAuth({ user: configstore.get('user'), tokens: configstore.get('tokens') });
const client = new Client({ urlPrefix: 'https://firestore.googleapis.com', auth: true });
const base = `/v1/projects/${project}/databases/(default)/documents`;

const hash = text => createHash('sha256').update(text, 'utf8').digest('hex').slice(0, 12);
function decode(value) {
  const [[type, raw]] = Object.entries(value);
  switch (type) {
    case 'nullValue': return null;
    case 'integerValue': return Number(raw);
    case 'arrayValue': return (raw.values ?? []).map(decode);
    case 'mapValue': return fields(raw.fields);
    default: return raw;
  }
}
const fields = (map = {}) => Object.fromEntries(Object.entries(map).map(([k, v]) => [k, decode(v)]));
const leaf = name => name.slice(name.lastIndexOf('/') + 1);

async function get(path) {
  const res = await client.request({ method: 'GET', path, responseType: 'json', resolveOnHTTPError: true });
  if (res.status === 404) return null;
  if (res.status !== 200) throw new Error(`Firestore ${res.status} for a ${path.split('/').length}-segment path`);
  return res.body;
}
async function list(path, extra = '') {
  const out = [];
  let token = '';
  do {
    const body = await get(`${path}?pageSize=300${extra}${token && `&pageToken=${encodeURIComponent(token)}`}`);
    out.push(...(body?.documents ?? []));
    token = body?.nextPageToken ?? '';
  } while (token);
  return out;
}

const expected = expectFile
  ? readFileSync(expectFile, 'utf8').split('\n').filter(line => line.length).map(line => ({ hash: hash(line) }))
  : [];
const seen = new Set();
const report = { project, at: new Date().toISOString(), users: [] };

// showMissing lists users whose account document is gone but whose
// subcollections (e.g. cleanup_jobs after account deletion) remain.
for (const doc of await list(`${base}/users`, '&showMissing=true&mask.fieldPaths=account_state')) {
  const uid = leaf(doc.name);
  const path = `${base}/users/${encodeURIComponent(uid)}`;
  const accountDoc = await get(path);
  const account = accountDoc && fields(accountDoc.fields);
  const consent = account?.consent;
  const user = {
    uid_hash: hash(uid),
    account: account ? {
      state: account.account_state, epoch: account.account_epoch,
      consent_revision: consent?.revision,
      settings: consent?.settings, dictionary: consent?.dictionary,
      transcripts: consent && { enabled: consent.transcripts.enabled,
        generation: consent.transcripts.generation, history: consent.transcripts.history },
      resets: Array.isArray(account.resets) ? account.resets.length : 0,
    } : null,
    records: {}, settings: {}, text_hashes: [],
  };
  for (const record of await list(`${path}/records`)) {
    const r = fields(record.fields);
    const kind = r.content?.data?.kind ?? r.category;
    const bucket = (user.records[r.category] ??= {});
    const key = `${r.content?.state}:${kind}`;
    bucket[key] = (bucket[key] ?? 0) + 1;
    if (r.content?.state !== 'live') continue;
    const data = r.content.data;
    if (r.category === 'settings') {
      user.settings[data.key] = typeof data.value === 'string' ? data.value : data.value && Object.keys(data.value).sort().join('+');
      continue;
    }
    for (const field of ['text', 'from', 'to', 'phrase']) {
      if (typeof data[field] !== 'string') continue;
      const h = hash(data[field]);
      user.text_hashes.push(`${r.category}.${kind}.${field}:${h}`);
      seen.add(h);
    }
  }
  for (const sub of ['receipts', 'families', 'attempts', 'devices']) {
    user[sub] = (await list(`${path}/${sub}`, '&mask.fieldPaths=__name__')).length;
  }
  user.cleanup_jobs = (await list(`${path}/cleanup_jobs`)).map(job => {
    const j = fields(job.fields);
    return { state: j.receipt?.state, scope: j.request?.scope, effect: j.request?.effect };
  });
  user.text_hashes.sort();
  report.users.push(user);
}
if (expected.length) report.expected = expected.map((e, i) => ({ line: i + 1, hash: e.hash, present: seen.has(e.hash) }));
process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
