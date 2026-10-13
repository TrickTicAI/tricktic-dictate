// Qualification helpers only: no persistence, sync, Admin SDK or app integration.
import { createHash, randomBytes, timingSafeEqual } from 'node:crypto';

export class ProofError extends Error {
  constructor(code) { super(code); this.code = code; }
}
const fail = (code) => { throw new ProofError(code); };
const text = (value) => typeof value === 'string' && value.length > 0;
const opaque = () => randomBytes(32).toString('base64url');
const endpoints = {
  google: 'https://oauth2.googleapis.com/token',
  signIn: 'https://identitytoolkit.googleapis.com/v1/accounts:signInWithIdp',
  refresh: 'https://securetoken.googleapis.com/v1/token',
};
const knownErrors = new Set([
  'INVALID_IDP_RESPONSE', 'OPERATION_NOT_ALLOWED', 'INVALID_REFRESH_TOKEN',
  'TOKEN_EXPIRED', 'USER_DISABLED', 'USER_NOT_FOUND', 'API_KEY_INVALID',
  'PROJECT_NUMBER_MISMATCH', 'INVALID_API_KEY', 'invalid_grant', 'invalid_client',
]);

export function validateConfig(config) {
  if (!config || !/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(config.projectId)
      || !text(config.apiKey) || config.apiKey.includes('REPLACE')
      || !/^[A-Za-z0-9_-]+\.apps\.googleusercontent\.com$/.test(config.webClientId)
      || !/^[A-Za-z0-9_-]+\.apps\.googleusercontent\.com$/.test(config.desktopClientId)) {
    fail('configuration_required');
  }
  if (config.desktopClientSecret !== undefined && !text(config.desktopClientSecret)) {
    fail('configuration_required');
  }
  return config;
}

export function beginAttempt(clientId, redirectUri, now = Date.now()) {
  const callback = new URL(redirectUri);
  if (callback.protocol !== 'http:' || callback.hostname !== '127.0.0.1'
      || !callback.port || callback.pathname !== '/callback' || callback.search
      || callback.hash || callback.username || callback.password) fail('invalid_redirect');
  const attempt = { state: opaque(), verifier: opaque(), nonce: opaque(),
    redirectUri, expiresAt: now + 180_000, consumed: false };
  const url = new URL('https://accounts.google.com/o/oauth2/v2/auth');
  url.search = new URLSearchParams({ client_id: clientId, redirect_uri: redirectUri,
    response_type: 'code', scope: 'openid email profile', state: attempt.state,
    nonce: attempt.nonce, prompt: 'select_account', code_challenge_method: 'S256',
    code_challenge: createHash('sha256').update(attempt.verifier).digest('base64url') });
  return { attempt, url: url.toString() };
}

export function consumeCallback(attempt, requestTarget, host, now = Date.now()) {
  if (attempt.consumed || now >= attempt.expiresAt) fail('expired_or_replayed');
  const expected = new URL(attempt.redirectUri);
  if (!requestTarget.startsWith('/') || requestTarget.startsWith('//')
      || requestTarget.length > 8192 || host !== expected.host) fail('invalid_callback');
  const url = new URL(requestTarget, expected.origin);
  if (url.origin !== expected.origin || url.pathname !== expected.pathname || url.hash) {
    fail('invalid_callback');
  }
  for (const key of ['state', 'code', 'error']) {
    if (url.searchParams.getAll(key).length > 1) fail('invalid_callback');
  }
  const state = Buffer.from(url.searchParams.get('state') ?? '');
  const wanted = Buffer.from(attempt.state);
  if (state.length !== wanted.length || !timingSafeEqual(state, wanted)) fail('invalid_state');
  const code = url.searchParams.get('code');
  const error = url.searchParams.get('error');
  if ((code && error) || (!code && !error) || (code && code.length > 4096)) {
    fail('invalid_callback');
  }
  // Consume before any exchange, including a matching cancellation callback.
  attempt.consumed = true;
  if (error) fail(error === 'access_denied' ? 'cancelled' : 'provider_rejected');
  return code;
}

// Claims are only consistency checks. This is NOT a general JWT verifier.
// Google signatures/audience acceptance are enforced by Firebase signInWithIdp.
// Firebase claims below must come directly from its authenticated HTTPS response.
function claims(token) {
  try {
    if (!text(token) || token.length > 32768) fail('invalid_token');
    const parts = token.split('.');
    if (parts.length !== 3 || parts.some((part) => !/^[A-Za-z0-9_-]+$/.test(part))) {
      fail('invalid_token');
    }
    const value = JSON.parse(Buffer.from(parts[1], 'base64url'));
    if (!value || typeof value !== 'object' || Array.isArray(value)) fail('invalid_token');
    return value;
  } catch { fail('invalid_token'); }
}

function checkGoogle(token, audience, nonce) {
  const value = claims(token);
  if (value.aud !== audience
      || !['accounts.google.com', 'https://accounts.google.com'].includes(value.iss)
      || !text(value.sub) || !Number.isFinite(value.exp) || value.exp <= Date.now() / 1000
      || (nonce !== undefined && value.nonce !== nonce)) fail('google_claims_mismatch');
}

// Never forward redirects or print upstream bodies, URLs, tokens, or exceptions.
export async function requestJson(url, body, form = false, fetcher = fetch) {
  try {
    const response = await fetcher(url, { method: 'POST', redirect: 'error',
      signal: AbortSignal.timeout(15_000), headers: {
        'Content-Type': form ? 'application/x-www-form-urlencoded' : 'application/json',
      }, body: form ? new URLSearchParams(body).toString() : JSON.stringify(body) });
    let bytes = 0;
    const chunks = [];
    for await (const chunk of response.body) {
      bytes += chunk.length;
      if (bytes > 65536) fail('response_too_large');
      chunks.push(Buffer.from(chunk));
    }
    const value = JSON.parse(Buffer.concat(chunks).toString());
    if (!response.ok) {
      const code = value?.error?.message ?? value?.error;
      fail(knownErrors.has(code) ? code : 'upstream_rejected');
    }
    if (!value || typeof value !== 'object' || Array.isArray(value)) fail('invalid_response');
    return value;
  } catch (error) {
    if (error instanceof ProofError) throw error;
    fail('network_or_response_error');
  }
}

export async function exchangeCode(config, attempt, code, post = requestJson) {
  if (!attempt.consumed || attempt.exchanged || Date.now() >= attempt.expiresAt) fail('invalid_attempt');
  attempt.exchanged = true;
  const body = { client_id: config.desktopClientId, code, code_verifier: attempt.verifier,
    redirect_uri: attempt.redirectUri, grant_type: 'authorization_code' };
  if (config.desktopClientSecret) body.client_secret = config.desktopClientSecret;
  const response = await post(endpoints.google, body, true);
  checkGoogle(response.id_token, config.desktopClientId, attempt.nonce);
  return exchangeGoogle(config, response.id_token, config.desktopClientId,
    attempt.redirectUri, post);
}

function firebaseSession(config, response, expectedUid) {
  if (response.needConfirmation || response.mfaPendingCredential) fail('additional_auth_required');
  const value = claims(response.idToken);
  if (value.aud !== config.projectId || value.iss !== `https://securetoken.google.com/${config.projectId}`
      || !text(value.sub) || value.sub.length > 128 || value.sub !== response.localId
      || (expectedUid !== undefined && value.sub !== expectedUid)
      || !Number.isFinite(value.exp) || value.exp <= Date.now() / 1000
      || value.firebase?.sign_in_provider !== 'google.com'
      || !text(response.refreshToken)) fail('firebase_claims_mismatch');
  return { uid: value.sub, idToken: response.idToken, refreshToken: response.refreshToken };
}

export async function exchangeGoogle(config, googleToken, audience,
  requestUri = 'http://localhost', post = requestJson) {
  checkGoogle(googleToken, audience);
  const url = `${endpoints.signIn}?${new URLSearchParams({ key: config.apiKey })}`;
  const response = await post(url, { requestUri, returnSecureToken: true,
    returnIdpCredential: false,
    postBody: new URLSearchParams({ id_token: googleToken, providerId: 'google.com' }).toString() });
  if (response.providerId !== 'google.com') fail('unexpected_provider');
  return firebaseSession(config, response);
}

export async function refreshSession(config, session, post = requestJson) {
  const url = `${endpoints.refresh}?${new URLSearchParams({ key: config.apiKey })}`;
  const response = await post(url, { grant_type: 'refresh_token', refresh_token: session.refreshToken }, true);
  return firebaseSession(config, { idToken: response.id_token,
    refreshToken: response.refresh_token, localId: response.user_id }, session.uid);
}
