// Firebase ID token verification. The token only ever comes from the
// Authorization header; the caller's UID is the verified `sub`.
const bearer = /^Bearer ([A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]*)$/;
const invalid = /^auth\/(id-token-|argument-error|invalid-|user-disabled|user-not-found)/;
const kind = error => invalid.test(error?.code ?? '') ? 'unauthenticated' : 'unavailable';

export function createVerifier(auth) {
  /**
   * Returns { claims } for a valid, unrevoked token of an existing user. With
   * `deletedUser`, a token whose user no longer exists returns
   * { claims, deleted: true } when its signature and expiry still verify.
   */
  return async function verify(header, { deletedUser = false } = {}) {
    const token = typeof header === 'string' && header.length <= 8192 ? bearer.exec(header)?.[1] : null;
    if (!token) return { error: 'unauthenticated' };
    try {
      return { claims: await auth.verifyIdToken(token, true) };
    } catch (error) {
      if (!deletedUser || error?.code !== 'auth/user-not-found') return { error: kind(error) };
      try {
        return { claims: await auth.verifyIdToken(token, false), deleted: true };
      } catch (retry) {
        return { error: kind(retry) };
      }
    }
  };
}
