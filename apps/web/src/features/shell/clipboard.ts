import { useCallback, useEffect, useRef, useState } from 'react';

/**
 * One clipboard write for the whole application.
 *
 * It reports whether the text actually reached the clipboard rather than
 * assuming it did: a surface that says "Copied" when the write was refused is
 * the interface making a claim it cannot support.
 */
export async function copyText(text: string): Promise<boolean> {
  if (!text) return false;
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/**
 * Shared copy state for a surface with several ways to copy the same thing —
 * a control on the row, a right-click entry, a keyboard shortcut — so all of
 * them confirm in the same place and only one of them can be lit at a time.
 */
export function useCopyFeedback(holdMs = 1600) {
  const [copied, setCopied] = useState('');
  const [message, setMessage] = useState('');
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);

  const copy = useCallback(async (text: string, id = 'text') => {
    const done = await copyText(text);
    clearTimeout(timer.current);
    setCopied(done ? id : '');
    setMessage(done ? 'Copied' : 'Clipboard unavailable. Select the text and copy it manually.');
    timer.current = setTimeout(() => { setCopied(''); setMessage(''); }, holdMs);
    return done;
  }, [holdMs]);

  return { copied, message, copy };
}
