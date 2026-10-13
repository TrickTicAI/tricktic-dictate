import { useEffect, type RefObject } from 'react';

/**
 * Closes a popover the way people expect: Escape, or pressing anywhere outside
 * it and the control that opened it. Focus moves into the popover when it opens.
 */
export function usePopover(open: boolean, panel: RefObject<HTMLElement | null>, anchor: RefObject<HTMLElement | null>, onClose: () => void) {
  useEffect(() => {
    if (!open) return;
    panel.current?.querySelector<HTMLElement>('[aria-checked="true"], button:not(:disabled)')?.focus();
    const away = (event: MouseEvent) => {
      const target = event.target as Node;
      if (!panel.current?.contains(target) && !anchor.current?.contains(target)) onClose();
    };
    const key = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.stopPropagation();
      onClose();
      anchor.current?.focus();
    };
    window.addEventListener('mousedown', away);
    window.addEventListener('keydown', key, true);
    return () => { window.removeEventListener('mousedown', away); window.removeEventListener('keydown', key, true); };
  }, [open, panel, anchor, onClose]);
}
