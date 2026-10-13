import { useCallback, useSyncExternalStore } from 'react';

/*
 * URL-backed view state. There is no router dependency in this workspace, so
 * this is the one place that reads and writes `location.search`. Tab, sub-tab,
 * selected session and the open settings section all go through it, which is
 * what makes refresh, back/forward and copied links behave.
 */

const CHANGED = 'writer:urlchange';

function notify() {
  window.dispatchEvent(new Event(CHANGED));
}

function subscribe(listener: () => void) {
  window.addEventListener('popstate', listener);
  window.addEventListener(CHANGED, listener);
  return () => {
    window.removeEventListener('popstate', listener);
    window.removeEventListener(CHANGED, listener);
  };
}

function search() {
  return window.location.search;
}

/**
 * Updates one parameter, preserving every unrelated one. A value equal to the
 * default drops the parameter so the common view has a clean URL.
 */
export function setParam(key: string, value: string | null, options: { push?: boolean } = {}) {
  const params = new URLSearchParams(window.location.search);
  if (value === null) params.delete(key);
  else params.set(key, value);
  const query = params.toString();
  const url = `${window.location.pathname}${query ? `?${query}` : ''}${window.location.hash}`;
  // Replace by default: switching tabs should not fill the back stack.
  window.history[options.push ? 'pushState' : 'replaceState'](null, '', url);
  notify();
}

/** Reads several parameters at once without subscribing; for event handlers. */
export function readParam(key: string): string | null {
  return new URLSearchParams(window.location.search).get(key);
}

/**
 * A parameter constrained to a known set. An unknown or missing value falls
 * back to the default rather than rendering an empty view, so a hand-edited or
 * stale link still opens something.
 */
export function useEnumParam<T extends string>(key: string, allowed: readonly T[], fallback: T) {
  const raw = useSyncExternalStore(subscribe, search, search);
  const current = new URLSearchParams(raw).get(key);
  const value = allowed.includes(current as T) ? (current as T) : fallback;
  const set = useCallback(
    (next: T, options?: { push?: boolean }) => setParam(key, next === fallback ? null : next, options),
    [key, fallback],
  );
  return [value, set] as const;
}

/**
 * A free-form parameter such as a selected session id or a search term. The
 * caller validates the value against real data; an id that no longer exists is
 * a normal outcome, not an error.
 */
export function useParam(key: string) {
  const raw = useSyncExternalStore(subscribe, search, search);
  const value = new URLSearchParams(raw).get(key);
  const set = useCallback((next: string | null, options?: { push?: boolean }) => setParam(key, next, options), [key]);
  return [value, set] as const;
}
