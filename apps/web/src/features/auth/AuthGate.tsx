import { useEffect, useRef, useState, type ReactNode } from 'react';
import { nativeSession, sessionLost, signIn, signOut } from './session';
import './auth.css';
import { PRODUCT_NAME } from '../../product';

export function AuthGate({ children }: { children: ReactNode }) {
  const native = nativeSession();
  const [authorized, setAuthorized] = useState(native);
  const [opened, setOpened] = useState(native);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const keyInput = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const expired = () => { setAuthorized(false); setError('Sign in again. Saved recordings remain available.'); };
    window.addEventListener(sessionLost, expired);
    return () => window.removeEventListener(sessionLost, expired);
  }, []);
  if (native) return children;
  async function login(event: React.FormEvent) {
    event.preventDefault();
    const key = keyInput.current?.value ?? '';
    // Clear the DOM immediately; never retain the owner key in React state.
    if (keyInput.current) keyInput.current.value = '';
    setBusy(true); setError('');
    try { await signIn(key); setAuthorized(true); setOpened(true); }
    catch (cause) { setError(cause instanceof Error ? cause.message : 'Sign-in failed.'); }
    finally { setBusy(false); }
  }
  return <>
    {/* Keep capture/recovery mounted when a session expires. No audio is erased. */}
    {opened && <div className="authenticated-app" hidden={!authorized}>{children}</div>}
    {authorized ? <div className="owner-session-controls">
      <button type="button" className="button ghost small" disabled={busy} onClick={() => {
        setBusy(true); setError('');
        void signOut().catch(() => setError('Could not sign out on the server. Try again.')).finally(() => setBusy(false));
      }}>Sign out</button>
      {error && <span role="alert">{error}</span>}
    </div> : <main className="owner-login">
      <form onSubmit={event => void login(event)}>
        <h1>Sign in to {PRODUCT_NAME}</h1>
        <p>Enter the owner key for this {PRODUCT_NAME} server.</p>
        <label htmlFor="owner-key">Owner key</label>
        <input ref={keyInput} id="owner-key" type="password" autoComplete="off" spellCheck={false} required maxLength={64} autoFocus />
        <button className="button primary" disabled={busy} type="submit">{busy ? 'Signing in…' : 'Sign in'}</button>
        {error && <p role="alert">{error}</p>}
        <p>Your sign-in lasts for this tab, up to 12 hours. Reloading requires signing in again.</p>
      </form>
    </main>}
  </>;
}
