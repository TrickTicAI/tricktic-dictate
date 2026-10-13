import { useCallback, useEffect, useState } from 'react';
import { DEFAULT_THEME_ID, THEMES, themeById, themeCssVariables, type Theme } from '../../themes';

const STORAGE_KEY = 'writer-theme';

/**
 * The theme is a local presentation preference, so it lives in `localStorage`
 * rather than in `Settings`, which the API owns. A blocked or cleared store
 * degrades to the default instead of failing to paint.
 */
export function storedThemeId(): string {
  try {
    return localStorage.getItem(STORAGE_KEY) ?? DEFAULT_THEME_ID;
  } catch {
    return DEFAULT_THEME_ID;
  }
}

/**
 * Writes the sixteen tokens onto the root element, sets `data-theme-kind` so
 * `color-scheme` follows, and updates the `theme-color` meta. One call repaints
 * the whole application, including the title bar, which is ordinary page
 * content in the frameless window.
 */
export function applyTheme(theme: Theme): void {
  const root = document.documentElement;
  for (const [token, value] of Object.entries(themeCssVariables(theme))) root.style.setProperty(token, value);
  root.dataset.theme = theme.id;
  root.dataset.themeKind = theme.kind;
  document.querySelector('meta[name="theme-color"]')?.setAttribute('content', theme.ui.bg);
}

/** Colours the shell paints with: the panel's and the tray menu's, and the page background behind the window. Resolved here so the shell never re-derives a palette. */
export function overlayTheme(theme: Theme) {
  return {
    kind: theme.kind,
    bg: theme.ui.bgElevated,
    border: theme.ui.borderStrong,
    text: theme.ui.text,
    text_muted: theme.ui.textMuted,
    accent: theme.ui.accent,
    accent_text: theme.ui.accentText,
    surface: theme.ui.bgHover,
    danger: theme.ui.danger,
    window: theme.ui.bg,
  };
}

export type OverlayTheme = ReturnType<typeof overlayTheme>;

export function useTheme(onChange?: (theme: Theme) => void) {
  const [id, setId] = useState(storedThemeId);
  const theme = themeById(id);
  useEffect(() => {
    applyTheme(theme);
    onChange?.(theme);
    // A subscriber added after the first paint still needs the current theme,
    // so this runs on mount as well as on change.
  }, [theme, onChange]);
  const select = useCallback((next: string) => {
    setId(themeById(next).id);
    try {
      localStorage.setItem(STORAGE_KEY, themeById(next).id);
    } catch {
      /* A blocked store only costs persistence; the live theme still applies. */
    }
  }, []);
  return { theme, themes: THEMES, select };
}
