import { useEffect, useState } from 'react';
import { invoke, isTauri } from '../desktop-session/runtime/native';
import { STORAGE_PLACE } from '../desktop-session/runtime/storage-full';

/** Saved dictation data against its limit, from the shell's last measurement. */
export interface StorageBudget { used_bytes: number; limit_bytes: number }
export type StorageLevel = 'ok' | 'almost_full' | 'full';
/** Settings dispatches this on the window after the limit changes. */
export const STORAGE_LIMIT_CHANGED = 'writer-storage-limit-changed';
/** Warn once saved data passes this share of its limit. */
export const STORAGE_WARN_RATIO = 0.9;

/** Sizes the way Windows Explorer and Finder say them: one decimal, never "0.0". */
export function size(value: number): string {
  const units: [number, string][] = [[1024 ** 3, 'GB'], [1024 ** 2, 'MB'], [1024, 'KB']];
  for (const [unit, label] of units) {
    if (value >= unit) return `${(value / unit).toFixed(value >= 100 * unit ? 0 : 1)} ${label}`;
  }
  return value > 0 ? 'Under 1 KB' : '0 KB';
}

export function storageLevel(used: number, limit: number): StorageLevel {
  if (!(limit > 0)) return 'ok';
  if (used >= limit) return 'full';
  return used >= limit * STORAGE_WARN_RATIO ? 'almost_full' : 'ok';
}

/** One plain sentence: how full, what happens at the limit, and where to free space. */
export function storageWarning(level: Exclude<StorageLevel, 'ok'>, used: number, limit: number, where = `in ${STORAGE_PLACE}`): string {
  const amount = `${size(Math.min(used, limit))} of ${size(limit)} used`;
  return level === 'full'
    ? `Storage is full: ${amount}. New dictations can’t be transcribed until you free space ${where}.`
    : `Storage is almost full: ${amount}. At the limit, new dictations can’t be transcribed. Free space ${where}.`;
}

/** What a dictation refused for storage says in its history entry. */
export const STORAGE_FULL_FAILURE = `Storage is full, so this dictation was not transcribed. Your audio is saved. Free space in ${STORAGE_PLACE}, then retry.`;

/**
 * The desktop's storage budget, read again whenever `refresh` changes to
 * anything but null. The shell answers from its last measurement and never
 * walks the history, so this is cheap after every dictation. Undefined in the
 * browser build and before the first measurement.
 */
export function useStorageBudget(refresh: string | null): StorageBudget | undefined {
  const [budget, setBudget] = useState<StorageBudget>();
  // A new limit in Settings is read at once, even between dictations.
  const [changed, setChanged] = useState(0);
  useEffect(() => {
    const bump = () => setChanged((value) => value + 1);
    window.addEventListener(STORAGE_LIMIT_CHANGED, bump);
    return () => window.removeEventListener(STORAGE_LIMIT_CHANGED, bump);
  }, []);
  useEffect(() => {
    if (!isTauri() || refresh === null) return;
    let current = true;
    invoke<StorageBudget | null>('storage_budget')
      .then((value) => { if (current) setBudget(value ?? undefined); })
      .catch(() => { /* An older shell has no budget reading; nothing to warn about. */ });
    return () => { current = false; };
  }, [refresh, changed]);
  return budget;
}
