import { isTauri } from '../desktop-session/runtime/native';
import { PRODUCT_NAME } from '../../product';

// Deliberately memory-only: no cookies, URL parameters, localStorage or IndexedDB.
let session: string | null = null;
export const sessionLost = 'writer-session-lost';
export const sessionStarted = 'writer-session-started';
export const nativeSession = () => isTauri() || location.origin === 'https://writer.metrix.invalid';

export async function apiFetch(path: string, init: RequestInit = {}): Promise<Response> {
  if (nativeSession()) return fetch(path, init);
  const url = new URL(path, location.origin);
  if (url.origin !== location.origin || !url.pathname.startsWith('/api/') || url.search) {
    throw new Error(`${PRODUCT_NAME} API requests must use this application’s origin.`);
  }
  const headers = new Headers(init.headers);
  headers.set('X-Writer-Request', '1');
  const sentSession = session;
  if (sentSession) headers.set('Authorization', `Bearer ${sentSession}`);
  const response = await fetch(path, { ...init, headers, credentials: 'omit', redirect: 'error', cache: 'no-store' });
  // A late rejection from an old session must not revoke a fresh login.
  if (response.status === 401 && sentSession && session === sentSession) {
    session = null;
    window.dispatchEvent(new Event(sessionLost));
  }
  return response;
}
export async function signIn(ownerKey: string): Promise<void> {
  const response = await fetch('/api/v1/session', {
    method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Writer-Request': '1' },
    body: JSON.stringify({ owner_key: ownerKey }), credentials: 'omit', redirect: 'error', cache: 'no-store',
  });
  if (!response.ok) {
    const failure = await response.json().catch(() => null);
    throw new Error(failure?.error?.code === 'session_limit'
      ? `Too many active tabs. Sign out of another tab or restart the ${PRODUCT_NAME} server.`
      : response.status === 429 ? 'Wait a minute before signing in again.'
      : 'Sign-in failed. Check the owner key and server connection.');
  }
  const body = await response.json();
  if (body.api_version !== '1' || typeof body.data?.token !== 'string' || !/^[a-f0-9]{64}$/.test(body.data.token)) {
    throw new Error(`${PRODUCT_NAME} returned an invalid session.`);
  }
  session = body.data.token;
  window.dispatchEvent(new Event(sessionStarted));
}
export async function signOut(): Promise<void> {
  const response = await apiFetch('/api/v1/session', { method: 'DELETE' });
  if (!response.ok && response.status !== 401) throw new Error('Could not sign out on the server. Try again.');
  session = null;
  window.dispatchEvent(new Event(sessionLost));
}
