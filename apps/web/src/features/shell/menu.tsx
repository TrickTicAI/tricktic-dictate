import { useCallback, useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type MouseEvent as ReactMouseEvent } from 'react';
import { ClipboardPaste, Copy, Scissors, TextSelect, type LucideIcon } from 'lucide-react';

/*
 * Writer's own context menu.
 *
 * The WebView draws a menu of its own — Back, Reload, Save as, Print — which is
 * about the browser rather than about the thing under the pointer, and which no
 * theme can repaint. It is suppressed everywhere. A surface that has real
 * actions opens this menu instead by calling `openMenu`; a surface that does
 * not gets no menu at all, which is the honest answer for a background.
 *
 * One menu is open at a time and it lives in a module store rather than in
 * React context, so a row deep in a list can open one without every ancestor
 * having to thread a callback down to it.
 */

export type MenuEntry =
  | { kind?: 'command'; label: string; icon?: LucideIcon; hint?: string; disabled?: boolean; danger?: boolean; run: () => void }
  | { kind: 'separator' };

/** `false` and `undefined` are allowed so callers can write conditional rows inline. */
export type MenuInput = (MenuEntry | false | null | undefined)[];

type OpenMenu = {
  entries: MenuEntry[];
  x: number;
  y: number;
  direction: 'ltr' | 'rtl';
  restore: HTMLElement | null;
};

const isCommand = (entry: MenuEntry): entry is Extract<MenuEntry, { kind?: 'command' }> =>
  entry.kind === undefined || entry.kind === 'command';

let current: OpenMenu | null = null;
const listeners = new Set<() => void>();
const read = () => current;
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
function publish(next: OpenMenu | null) {
  current = next;
  listeners.forEach((listener) => listener());
}

/* Events a component has already answered. The document-level fallback below
   only has to decide what to do with the ones nothing claimed. */
const claimed = new WeakSet<Event>();

export function closeMenu(restoreFocus = false) {
  const open = current;
  publish(null);
  if (restoreFocus && open?.restore?.isConnected) open.restore.focus();
}

/**
 * Opens the menu for one right-click. Always suppresses the WebView's menu,
 * including when `entries` turns out to hold no command: "nothing to do here"
 * is shown as no menu rather than as a list of dead rows.
 */
export function openMenu(event: MouseEvent | ReactMouseEvent, entries: MenuInput) {
  const native = 'nativeEvent' in event ? event.nativeEvent : event;
  claimed.add(native);
  event.preventDefault();
  const list = entries.filter((entry): entry is MenuEntry => !!entry);
  if (!list.some(isCommand)) { closeMenu(); return; }
  const target = event.target instanceof HTMLElement ? event.target : null;
  // A menu raised from the keyboard (Shift+F10, the Menu key) reports no
  // pointer position in some builds; it is anchored to the element instead.
  const corner = target?.getBoundingClientRect();
  const point = native.clientX || native.clientY
    ? { x: native.clientX, y: native.clientY }
    : { x: corner ? corner.left + 8 : 0, y: corner ? corner.bottom : 0 };
  publish({
    ...point,
    entries: list,
    // The menu is interface, so it reads the way the interface does. A Hebrew
    // transcript mirrors its own row; it does not mirror the commands acting
    // on it.
    direction: document.documentElement.dir === 'rtl' ? 'rtl' : 'ltr',
    restore: document.activeElement instanceof HTMLElement ? document.activeElement : null,
  });
}

type Field = HTMLInputElement | HTMLTextAreaElement;
const FIELDS = 'input:not([type=range]):not([type=checkbox]):not([type=radio]), textarea';

async function paste(field: Field) {
  field.focus();
  try {
    const text = await navigator.clipboard.readText();
    // `insertText` respects the current selection and raises the input event a
    // controlled React field listens for, so the value the component holds and
    // the value on screen stay the same thing.
    if (text) document.execCommand('insertText', false, text);
  } catch { /* Ctrl+V still reaches the field directly. */ }
}

/**
 * Cut, copy, paste and select all for a text field. This is the one place the
 * platform menu was actually relevant, so it is the one place the replacement
 * has to carry the same four commands.
 */
function editEntries(field: Field): MenuEntry[] {
  const selected = field.selectionStart !== field.selectionEnd;
  const writable = !field.readOnly && !field.disabled;
  const act = (command: string) => () => { field.focus(); document.execCommand(command); };
  return [
    { label: 'Cut', icon: Scissors, hint: 'Ctrl+X', disabled: !selected || !writable, run: act('cut') },
    { label: 'Copy', icon: Copy, hint: 'Ctrl+C', disabled: !selected, run: act('copy') },
    { label: 'Paste', icon: ClipboardPaste, hint: 'Ctrl+V', disabled: !writable, run: () => void paste(field) },
    { kind: 'separator' },
    { label: 'Select all', icon: TextSelect, hint: 'Ctrl+A', disabled: !field.value, run: () => { field.focus(); field.select(); } },
  ];
}

/**
 * The single menu surface. Mounted once per window; it also installs the
 * document-level suppression, so a page that renders it never shows the
 * WebView's own menu.
 */
export function MenuHost() {
  const menu = useSyncExternalStore(subscribe, read, read);
  const panel = useRef<HTMLDivElement>(null);
  const [placed, setPlaced] = useState<{ x: number; y: number } | null>(null);

  useEffect(() => {
    const onContextMenu = (event: MouseEvent) => {
      if (claimed.has(event)) return;
      event.preventDefault();
      const target = event.target instanceof HTMLElement ? event.target : null;
      const field = target?.closest<Field>(FIELDS);
      if (field) openMenu(event, editEntries(field));
      else closeMenu();
    };
    document.addEventListener('contextmenu', onContextMenu);
    return () => document.removeEventListener('contextmenu', onContextMenu);
  }, []);

  // Measured, then flipped at whichever edge it would have run past. Menus are
  // placed before they are shown, so one never appears half off the window.
  useLayoutEffect(() => {
    setPlaced(null);
    if (!menu) return;
    const node = panel.current;
    if (!node) return;
    const margin = 8;
    const { width, height } = node.getBoundingClientRect();
    let x = menu.direction === 'rtl' ? menu.x - width : menu.x;
    if (x + width > window.innerWidth - margin) x = menu.x - width;
    if (x < margin) x = margin;
    let y = menu.y;
    if (y + height > window.innerHeight - margin) y = menu.y - height;
    if (y < margin) y = margin;
    setPlaced({ x: Math.min(x, Math.max(margin, window.innerWidth - width - margin)), y });
  }, [menu]);

  // Focus waits for the placement to be painted: a menu is hidden until it
  // knows where it goes, and a hidden element cannot take focus, which would
  // leave Escape and the arrow keys with nothing to act on.
  useEffect(() => {
    if (!menu || !placed) return;
    panel.current?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
  }, [menu, placed]);

  useEffect(() => {
    if (!menu) return;
    const dismiss = () => closeMenu();
    // Capture, because a scroller between the pointer and the document would
    // otherwise move the page out from under an open menu.
    window.addEventListener('scroll', dismiss, true);
    window.addEventListener('resize', dismiss);
    window.addEventListener('blur', dismiss);
    return () => {
      window.removeEventListener('scroll', dismiss, true);
      window.removeEventListener('resize', dismiss);
      window.removeEventListener('blur', dismiss);
    };
  }, [menu]);

  const move = useCallback((event: React.KeyboardEvent<HTMLDivElement>) => {
    const node = panel.current;
    if (!node) return;
    const items = [...node.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
    if (!items.length) return;
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    const go = (index: number) => { event.preventDefault(); items[(index + items.length) % items.length].focus(); };
    if (event.key === 'ArrowDown') go(at + 1);
    else if (event.key === 'ArrowUp') go(at < 0 ? items.length - 1 : at - 1);
    else if (event.key === 'Home') go(0);
    else if (event.key === 'End') go(items.length - 1);
    else if (event.key === 'Escape') { event.preventDefault(); closeMenu(true); }
    else if (event.key === 'Tab') closeMenu(true);
  }, []);

  if (!menu) return null;

  return (
    <div
      className="menu-layer"
      // A click outside dismisses the menu without also pressing whatever is
      // under it, which is how a menu behaves everywhere else on this desktop.
      onPointerDown={() => closeMenu(true)}
      onContextMenu={(event) => { event.preventDefault(); closeMenu(true); }}
    >
      <div
        ref={panel}
        className="menu"
        role="menu"
        aria-orientation="vertical"
        dir={menu.direction}
        onKeyDown={move}
        onPointerDown={(event) => event.stopPropagation()}
        style={{ transform: `translate(${placed?.x ?? menu.x}px, ${placed?.y ?? menu.y}px)`, visibility: placed ? 'visible' : 'hidden' }}
      >
        {menu.entries.map((entry, index) => {
          if (entry.kind === 'separator') return <hr key={`separator-${index}`} />;
          const Icon = entry.icon;
          return (
            <button
              key={`${entry.label}-${index}`}
              type="button"
              role="menuitem"
              className={entry.danger ? 'menu-item danger' : 'menu-item'}
              disabled={entry.disabled}
              onClick={() => { closeMenu(true); entry.run(); }}
            >
              {Icon ? <Icon size={14} aria-hidden /> : <span className="menu-icon-gap" aria-hidden />}
              <span className="menu-label">{entry.label}</span>
              {entry.hint && <span className="menu-hint">{entry.hint}</span>}
            </button>
          );
        })}
      </div>
    </div>
  );
}
