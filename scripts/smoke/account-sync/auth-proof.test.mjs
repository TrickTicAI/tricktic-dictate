import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { beginAttempt, consumeCallback, exchangeCode, exchangeGoogle,
  refreshSession, requestJson, validateConfig, ProofError } from './auth-proof.mjs';
import { browserCode } from './desktop.mjs';

const config = { projectId: 'writer-fixture', apiKey: 'fixture-only',
  webClientId: 'fixture-web.apps.googleusercontent.com',
  desktopClientId: 'fixture-desktop.apps.googleusercontent.com' };
const redirect = 'http://127.0.0.1:45678/callback';
const start = () => beginAttempt(config.desktopClientId, redirect);
const callback = (attempt, params = {}) => '/callback?' + new URLSearchParams({
  state: attempt.state, code: 'synthetic-code', ...params });
const consume = (attempt, params) => consumeCallback(attempt, callback(attempt, params), '127.0.0.1:45678');
// Deliberately unsigned fixture JWTs. Only injected fake HTTPS responses accept
// these; no emulator or live Google/Firebase acceptance is claimed by this suite.
const jwt = (value) => ['fixture', Buffer.from(JSON.stringify(value)).toString('base64url'), 'fixture'].join('.');
const google = (changes = {}) => jwt({ iss: 'https://accounts.google.com',
  sub: 'google-fixture-user', aud: config.desktopClientId, exp: Date.now() / 1000 + 3600, ...changes });
const firebase = (changes = {}) => jwt({ iss: `https://securetoken.google.com/${config.projectId}`,
  aud: config.projectId, sub: 'firebase-fixture-user', exp: Date.now() / 1000 + 3600,
  firebase: { sign_in_provider: 'google.com' }, ...changes });
const response = (changes = {}) => ({ idToken: firebase(), refreshToken: 'synthetic-refresh',
  localId: 'firebase-fixture-user', providerId: 'google.com', ...changes });

test('missing configuration fails before any external request', () => {
  for (const value of [null, {}, { ...config, apiKey: 'REPLACE_ME' }]) {
    assert.throws(() => validateConfig(value), /configuration_required/);
  }
  assert.equal(validateConfig(config), config);
});

test('fresh state, nonce, PKCE S256; verifier never enters authorization URL', () => {
  const { attempt, url } = start();
  const params = new URL(url).searchParams;
  assert.equal(params.get('code_challenge'), createHash('sha256').update(attempt.verifier).digest('base64url'));
  assert.equal(params.get('code_challenge_method'), 'S256');
  assert.equal(params.get('scope'), 'openid email profile');
  assert.equal(params.get('nonce'), attempt.nonce);
  assert.ok(!url.includes(attempt.verifier));
  assert.notEqual(attempt.state, start().attempt.state);
});

test('callback binds path, host, state and rejects duplicate/ambiguous/absolute targets', () => {
  const { attempt } = start();
  for (const [target, host] of [
    [callback(attempt), 'attacker.invalid'],
    [callback(attempt, { state: 'wrong' }), '127.0.0.1:45678'],
    [callback(attempt) + '&state=second', '127.0.0.1:45678'],
    [callback(attempt) + '&code=second', '127.0.0.1:45678'],
    [callback(attempt, { error: 'access_denied' }), '127.0.0.1:45678'],
    [callback(attempt).replace('/callback', '/elsewhere'), '127.0.0.1:45678'],
    ['http://attacker.invalid' + callback(attempt), '127.0.0.1:45678'],
    ['//attacker.invalid' + callback(attempt), '127.0.0.1:45678'],
  ]) assert.throws(() => consumeCallback(attempt, target, host), ProofError);
  assert.equal(attempt.consumed, false);
  assert.equal(consume(attempt), 'synthetic-code');
  assert.throws(() => consume(attempt), /expired_or_replayed/);
});

test('expiry and cancellation consume no exchange and cannot replay', () => {
  const { attempt } = start();
  assert.throws(() => consumeCallback(attempt, callback(attempt), '127.0.0.1:45678', attempt.expiresAt), /expired_or_replayed/);
  const cancelled = start().attempt;
  assert.throws(() => consume(cancelled, { code: '', error: 'access_denied' }), /cancelled/);
  assert.equal(cancelled.consumed, true);
  assert.throws(() => consume(cancelled), /expired_or_replayed/);
});

test('redirect must be an exact IPv4 loopback callback with ephemeral port', () => {
  for (const uri of ['https://example.com/callback', 'http://localhost:1/callback',
    'http://0.0.0.0:1/callback', redirect + '?extra=1', 'http://127.0.0.1/callback']) {
    assert.throws(() => beginAttempt('fixture', uri), /invalid_redirect/);
  }
});

test('code exchange sends verifier once; Firebase owns resulting UID', async () => {
  const { attempt } = start();
  const calls = [];
  const post = async (url, body, form) => {
    calls.push({ url, body, form });
    return calls.length === 1 ? { id_token: google({ nonce: attempt.nonce }) } : response();
  };
  const code = consume(attempt);
  const session = await exchangeCode(config, attempt, code, post);
  assert.equal(session.uid, 'firebase-fixture-user');
  assert.equal(calls[0].body.code_verifier, attempt.verifier);
  assert.equal(calls[0].body.redirect_uri, redirect);
  assert.equal(calls[1].body.requestUri, redirect);
  assert.equal(calls[1].body.returnSecureToken, true);
  assert.equal(calls[1].body.returnIdpCredential, false);
  assert.equal(new URLSearchParams(calls[1].body.postBody).get('providerId'), 'google.com');
  await assert.rejects(exchangeCode(config, attempt, code, post), /invalid_attempt/);
});

test('wrong nonce fails before Firebase; wrong Google audience/issuer/expiry fail closed', async () => {
  const { attempt } = start();
  const code = consume(attempt);
  await assert.rejects(exchangeCode(config, attempt, code, async () => ({ id_token: google({ nonce: 'wrong' }) })), /google_claims_mismatch/);
  for (const changes of [{ aud: 'other-client' }, { iss: 'other-issuer' }, { exp: 0 }]) {
    let called = false;
    await assert.rejects(exchangeGoogle(config, google(changes), config.desktopClientId,
      redirect, async () => { called = true; }), /google_claims_mismatch/);
    assert.equal(called, false);
  }
});

test('Android exchange requires the web audience', async () => {
  // Cross-client Android credentials can have an Android authorized party and
  // a web audience; Firebase decides whether that registration is accepted.
  const token = google({ aud: config.webClientId, azp: 'fixture-android.apps.googleusercontent.com' });
  const session = await exchangeGoogle(config, token, config.webClientId, undefined, async () => response());
  assert.equal(session.uid, 'firebase-fixture-user');
  await assert.rejects(exchangeGoogle(config, token, config.desktopClientId), /google_claims_mismatch/);
});

test('Firebase wrong project, issuer, provider, UID and expired response rejected', async () => {
  for (const changes of [{ aud: 'other-project' }, { iss: 'other-issuer' }, { sub: 'other-user' },
    { exp: 0 }, { firebase: { sign_in_provider: 'password' } }]) {
    await assert.rejects(exchangeGoogle(config, google(), config.desktopClientId, redirect,
      async () => response({ idToken: firebase(changes) })), /firebase_claims_mismatch/);
  }
  await assert.rejects(exchangeGoogle(config, google(), config.desktopClientId, redirect,
    async () => response({ providerId: 'password' })), /unexpected_provider/);
  await assert.rejects(exchangeGoogle(config, google(), config.desktopClientId, redirect,
    async () => response({ needConfirmation: true })), /additional_auth_required/);
});

test('refresh rotates token and refuses a changed account', async () => {
  const session = { uid: 'firebase-fixture-user', refreshToken: 'old-synthetic-refresh' };
  const post = async (_url, body, form) => {
    assert.equal(form, true);
    assert.equal(body.grant_type, 'refresh_token');
    assert.equal(body.refresh_token, session.refreshToken);
    return { id_token: firebase(), refresh_token: 'new-synthetic-refresh', user_id: session.uid };
  };
  assert.equal((await refreshSession(config, session, post)).refreshToken, 'new-synthetic-refresh');
  await assert.rejects(refreshSession(config, session, async () => ({
    id_token: firebase({ sub: 'other' }), refresh_token: 'fixture', user_id: 'other',
  })), /firebase_claims_mismatch/);
});

test('HTTP transport does not follow redirects and sanitizes raw failures', async () => {
  const secret = 'SENSITIVE_FIXTURE_MUST_NOT_ESCAPE';
  for (const fetcher of [
    async (_url, options) => {
      assert.equal(options.redirect, 'error');
      return new Response(JSON.stringify({ error: { message: secret } }), { status: 400 });
    },
    async () => { throw new Error(secret); },
    async () => new Response(secret),
  ]) {
    await assert.rejects(requestJson('https://example.invalid', {}, false, fetcher), (error) => {
      assert.ok(error instanceof ProofError);
      assert.ok(!String(error).includes(secret));
      return true;
    });
  }
  await assert.rejects(requestJson('https://example.invalid', {}, false,
    async () => new Response('x'.repeat(65537))), /response_too_large/);
  await assert.rejects(requestJson('https://example.invalid', {}, false,
    async () => new Response(JSON.stringify({ error: { message: 'INVALID_IDP_RESPONSE' } }), { status: 400 })), /INVALID_IDP_RESPONSE/);
});

test('real loopback listener ignores forged return then accepts correct one without reflecting code', async () => {
  let responseRead;
  const result = await browserCode(config.desktopClientId, { launch: async (authUrl) => {
    const params = new URL(authUrl).searchParams;
    const returnUrl = new URL(params.get('redirect_uri'));
    returnUrl.search = new URLSearchParams({ state: 'forged', code: 'fixture' });
    const forged = await fetch(returnUrl);
    assert.equal(forged.status, 400);
    await forged.text();
    returnUrl.search = new URLSearchParams({ state: params.get('state'), code: 'private-fixture-code' });
    responseRead = fetch(returnUrl).then(async (response) => {
      assert.equal(response.headers.get('cache-control'), 'no-store');
      const html = await response.text();
      assert.ok(!html.includes('private-fixture-code'));
      assert.ok(html.includes('history.replaceState'));
    });
    await responseRead;
  } });
  await responseRead;
  assert.equal(result.code, 'private-fixture-code');
});

test('real loopback cancellation and no-return timeout', async () => {
  await assert.rejects(browserCode(config.desktopClientId, { launch: async (authUrl) => {
    const params = new URL(authUrl).searchParams;
    const url = new URL(params.get('redirect_uri'));
    url.search = new URLSearchParams({ state: params.get('state'), error: 'access_denied' });
    await fetch(url).then((response) => response.text());
  } }), /cancelled/);
  await assert.rejects(browserCode(config.desktopClientId, { launch: async () => {}, timeoutMs: 20 }), /timed_out/);
  await assert.rejects(browserCode(config.desktopClientId, { launch: async () => { throw new ProofError('browser_unavailable'); } }), /browser_unavailable/);
});

test('CLI missing config fails closed without a stack trace', () => {
  const result = spawnSync(process.execPath, ['scripts/smoke/account-sync/desktop.mjs', '/missing/config.json'], { encoding: 'utf8' });
  assert.equal(result.status, 1);
  assert.equal(result.stderr.trim(), 'proof_failed');
  assert.equal(result.stdout, '');
});
