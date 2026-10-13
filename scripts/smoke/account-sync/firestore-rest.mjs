// Minimal Firestore emulator REST client for the rules tests. It speaks the same
// REST protocol the desktop adapter will use; it never contacts a real project.
const host = process.env.FIRESTORE_EMULATOR_HOST;
export const projectId = process.env.GCLOUD_PROJECT ?? 'demo-metrix-writer';
if (!host || !projectId.startsWith('demo-')) {
  throw new Error('Run through scripts/smoke/account-sync/firestore-rules.mjs (emulator, demo- project only)');
}
const root = `projects/${projectId}/databases/(default)/documents`;
const api = `http://${host}/v1/${root}`;

export function encode(value) {
  if (value === null) return { nullValue: null };
  if (typeof value === 'boolean') return { booleanValue: value };
  if (typeof value === 'number') {
    return Number.isInteger(value) ? { integerValue: String(value) } : { doubleValue: value };
  }
  if (typeof value === 'string') return { stringValue: value };
  if (Array.isArray(value)) return { arrayValue: value.length ? { values: value.map(encode) } : {} };
  const fields = Object.fromEntries(Object.entries(value).map(([key, item]) => [key, encode(item)]));
  return { mapValue: Object.keys(fields).length ? { fields } : {} };
}
export function decode(value) {
  const [[type, raw]] = Object.entries(value);
  switch (type) {
    case 'nullValue': return null;
    case 'integerValue': return Number(raw);
    case 'arrayValue': return (raw.values ?? []).map(decode);
    case 'mapValue': return decodeFields(raw.fields);
    default: return raw;
  }
}
const decodeFields = (fields = {}) =>
  Object.fromEntries(Object.entries(fields).map(([key, item]) => [key, decode(item)]));

// Unsigned emulator ID token for a Google-provider Firebase user; 'owner' bypasses rules.
export function token(uid) {
  if (uid === 'owner') return 'owner';
  const part = object => Buffer.from(JSON.stringify(object)).toString('base64url');
  const now = Math.floor(Date.now() / 1000);
  return `${part({ alg: 'none', typ: 'JWT' })}.${part({ sub: uid, user_id: uid, iat: now, exp: now + 3600,
    auth_time: now, aud: projectId, iss: `https://securetoken.google.com/${projectId}`,
    firebase: { sign_in_provider: 'google.com', identities: {} } })}.`;
}

async function call(method, url, auth, body) {
  const headers = { 'content-type': 'application/json' };
  if (auth) headers.authorization = `Bearer ${token(auth)}`;
  const response = await fetch(url, { method, headers, body: body && JSON.stringify(body),
    signal: AbortSignal.timeout(30_000) });
  const text = await response.text();
  return { status: response.status, body: text ? JSON.parse(text) : {} };
}

export class Denied extends Error {
  constructor(status, body) { super(`Firestore ${status}: ${body.error?.status} ${body.error?.message ?? ''}`.trim()); this.status = status; this.body = body; }
}

// batchGet, because the emulator cannot parse a transaction in a GET query string.
export async function getDocument(path, auth, transaction) {
  const { status, body } = await call('POST', `${api}:batchGet`, auth,
    { documents: [`${root}/${path}`], ...(transaction && { transaction }) });
  if (status !== 200) throw new Denied(status, body);
  const [entry] = body;
  return entry.found ? decodeFields(entry.found.fields) : null;
}

export async function commit(writes, auth, transaction) {
  const { status, body } = await call('POST', `${api}:commit`, auth, { ...(transaction && { transaction }),
    writes: writes.map(({ path, data, remove }) => remove ? { delete: `${root}/${path}` }
      : { update: { name: `${root}/${path}`, fields: encode(data).mapValue.fields ?? {} } }) });
  if (status !== 200) throw new Denied(status, body);
  return body;
}

// Run `plan(read)` in a read-write transaction and commit its writes.
export async function transact(auth, plan) {
  const begin = await call('POST', `${api}:beginTransaction`, auth, {});
  if (begin.status !== 200) throw new Denied(begin.status, begin.body);
  const transaction = begin.body.transaction;
  const outcome = await plan(path => getDocument(path, auth, transaction));
  if (outcome.writes.length) await commit(outcome.writes, auth, transaction);
  else await call('POST', `${api}:rollback`, auth, { transaction });
  return outcome;
}

export async function list(parent, collection, auth, limit) {
  const { status, body } = await call('POST', `${api}${parent ? `/${parent}` : ''}:runQuery`, auth,
    { structuredQuery: { from: [{ collectionId: collection }], ...(limit !== undefined && { limit }) } });
  if (status !== 200) throw new Denied(status, body);
  return body.filter(entry => entry.document).map(entry => decodeFields(entry.document.fields));
}

export async function clear() {
  const response = await fetch(`http://${host}/emulator/v1/${root}`, { method: 'DELETE' });
  if (!response.ok) throw new Error(`emulator reset failed: ${response.status}`);
}

export const seed = documents => commit([...documents].map(([path, data]) => ({ path, data })), 'owner');
