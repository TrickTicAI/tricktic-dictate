import { PRODUCT_NAME } from './product';
/*
 * Ported from the TrickTic ADE web app's themes.
 *
 * The sixteen `ui` tokens keep the ADE's names and meanings exactly, so a theme
 * added to either application can be pasted into the other unchanged. The ADE's
 * `terminal` palette is deliberately dropped: Writer has no terminal. Writer's
 * extra surfaces derive rather than extend — the record dot is `--danger` and
 * the live waveform is `--accent`.
 */

export type ThemeKind = 'dark' | 'light';

export interface ThemeUi {
  /** Page background behind everything. */
  bg: string;
  /** Rail and dialogs. */
  bgRail: string;
  /** Pane headers, cards, inputs. */
  bgElevated: string;
  /** Hover states. */
  bgHover: string;
  border: string;
  borderStrong: string;
  text: string;
  textMuted: string;
  textFaint: string;
  accent: string;
  accentText: string;
  accentSoft: string;
  success: string;
  warn: string;
  danger: string;
  /** Optional gradient painted behind the rail and dialogs. */
  backdrop?: string;
}

export interface Theme {
  id: string;
  name: string;
  kind: ThemeKind;
  description: string;
  ui: ThemeUi;
}

type UiOverrides = Partial<ThemeUi> & Pick<ThemeUi, 'bg' | 'bgRail' | 'bgElevated' | 'accent'>;

function darkUi(overrides: UiOverrides): ThemeUi {
  return {
    bgHover: '#ffffff0f',
    border: '#ffffff14',
    borderStrong: '#ffffff26',
    text: '#e7eaf0',
    textMuted: '#9aa3b2',
    textFaint: '#5f6877',
    accentText: '#ffffff',
    accentSoft: `${overrides.accent}2b`,
    success: '#3fb950',
    warn: '#d7a13a',
    danger: '#f0596b',
    ...overrides,
  };
}

function lightUi(overrides: UiOverrides): ThemeUi {
  return {
    bgHover: '#00000010',
    border: '#00000016',
    borderStrong: '#00000030',
    text: '#1c2230',
    textMuted: '#5a6373',
    textFaint: '#98a0ad',
    accentText: '#ffffff',
    accentSoft: `${overrides.accent}26`,
    success: '#1f8a3b',
    warn: '#b7791f',
    danger: '#d1242f',
    ...overrides,
  };
}

export const THEMES: Theme[] = [
  {
    // Writer's own addition and its default. Terracotta rather than pure black,
    // so an active sidebar item and the record button stay distinguishable from
    // ordinary dark text on warm paper.
    id: 'paper',
    name: 'Paper',
    kind: 'light',
    description: `Warm paper and ink. ${PRODUCT_NAME}’s default.`,
    ui: lightUi({
      bg: '#EBE5DC',
      bgRail: '#E3DCD1',
      bgElevated: '#FDFBF7',
      accent: '#B4541E',
      accentText: '#FFFDF9',
      text: '#1F1B16',
      textMuted: '#6B6155',
      textFaint: '#9C9084',
      success: '#2F7D3A',
      warn: '#A5761B',
      danger: '#B3261E',
    }),
  },
  {
    id: 'dark',
    name: 'Dark',
    kind: 'dark',
    description: 'Cool graphite with a blue accent.',
    ui: darkUi({ bg: '#0e1116', bgRail: '#12161d', bgElevated: '#181d26', accent: '#4c8dff' }),
  },
  {
    id: 'black',
    name: 'Black',
    kind: 'dark',
    description: 'Pure OLED black. Minimal, zero distraction.',
    ui: darkUi({ bg: '#000000', bgRail: '#050505', bgElevated: '#0d0d0d', accent: '#ffffff', accentText: '#000000', border: '#ffffff1a' }),
  },
  {
    id: 'light',
    name: 'Light',
    kind: 'light',
    description: 'Clean and bright. Crisp white with sharp contrast.',
    ui: lightUi({ bg: '#f5f6f8', bgRail: '#ffffff', bgElevated: '#ffffff', accent: '#2f6fed' }),
  },
  {
    id: 'dracula',
    name: 'Dracula',
    kind: 'dark',
    description: 'Slate surfaces with a magenta accent.',
    ui: darkUi({
      bg: '#282a36',
      bgRail: '#2c2e3a',
      bgElevated: '#333540',
      bgHover: '#ffffff12',
      border: '#ffffff18',
      borderStrong: '#ffffff2e',
      accent: '#ec4899',
      accentText: '#ffffff',
      accentSoft: '#ec489933',
      text: '#f8f8f2',
      textMuted: '#a9abbb',
      textFaint: '#6b6f82',
      success: '#50fa7b',
      warn: '#f1fa8c',
      danger: '#ff5555',
    }),
  },
  {
    id: 'solarized-dark',
    name: 'Solarized Dark',
    kind: 'dark',
    description: 'Deep teal with the classic Solarized palette.',
    ui: darkUi({ bg: '#00212b', bgRail: '#002b36', bgElevated: '#073642', accent: '#2aa198', text: '#eee8d5', textMuted: '#93a1a1', textFaint: '#586e75' }),
  },
  {
    id: 'solarized-light',
    name: 'Solarized Light',
    kind: 'light',
    description: 'Warm paper with the Solarized palette.',
    ui: lightUi({ bg: '#eee8d5', bgRail: '#fdf6e3', bgElevated: '#fdf6e3', accent: '#cb4b16', text: '#073642', textMuted: '#586e75' }),
  },
  {
    id: 'gruvbox-dark',
    name: 'Gruvbox Dark',
    kind: 'dark',
    description: 'Retro groove, warm and low contrast.',
    ui: darkUi({ bg: '#1d2021', bgRail: '#242627', bgElevated: '#282828', accent: '#fe8019', accentText: '#1d2021', text: '#ebdbb2', textMuted: '#a89984', textFaint: '#665c54' }),
  },
  {
    id: 'gruvbox-light',
    name: 'Gruvbox Light',
    kind: 'light',
    description: 'Retro groove on cream paper.',
    ui: lightUi({ bg: '#f2e5bc', bgRail: '#fbf1c7', bgElevated: '#fbf1c7', accent: '#d65d0e', text: '#3c3836', textMuted: '#665c54' }),
  },
  {
    id: 'nord',
    name: 'Nord',
    kind: 'dark',
    description: 'Arctic, north-bluish and calm.',
    ui: darkUi({ bg: '#242933', bgRail: '#2e3440', bgElevated: '#3b4252', accent: '#88c0d0', accentText: '#2e3440', text: '#eceff4', textMuted: '#a3adc2', textFaint: '#616e88' }),
  },
  {
    id: 'tokyo-night',
    name: 'Tokyo Night',
    kind: 'dark',
    description: 'Neon city lights on indigo.',
    ui: darkUi({ bg: '#16161e', bgRail: '#1a1b26', bgElevated: '#1f2335', accent: '#7aa2f7', accentText: '#16161e', text: '#c0caf5', textMuted: '#8b93b8', textFaint: '#565f89' }),
  },
  {
    id: 'catppuccin-mocha',
    name: 'Catppuccin Mocha',
    kind: 'dark',
    description: 'Soothing pastels on espresso.',
    ui: darkUi({ bg: '#181825', bgRail: '#1e1e2e', bgElevated: '#24243a', accent: '#cba6f7', accentText: '#181825', text: '#cdd6f4', textMuted: '#a6adc8', textFaint: '#6c7086' }),
  },
  {
    id: 'one-dark',
    name: 'One Dark',
    kind: 'dark',
    description: 'The Atom classic.',
    ui: darkUi({ bg: '#21252b', bgRail: '#282c34', bgElevated: '#2c313a', accent: '#61afef', accentText: '#21252b', text: '#abb2bf', textMuted: '#8b93a1', textFaint: '#5c6370' }),
  },
  {
    id: 'cyber-wave',
    name: 'Cyber Wave',
    kind: 'dark',
    description: 'Gradient backdrop with a violet-to-cyan accent.',
    ui: darkUi({
      bg: '#0b1020',
      bgRail: '#0f1630',
      bgElevated: '#141c3a',
      accent: '#7c5cff',
      backdrop: 'linear-gradient(160deg, #0b1020 0%, #121a3d 55%, #0a2a3a 100%)',
    }),
  },
  {
    id: 'willow-dream',
    name: 'Willow Dream',
    kind: 'dark',
    description: 'Forest gradient with a soft coral accent.',
    ui: darkUi({
      bg: '#0f1a16',
      bgRail: '#13231d',
      bgElevated: '#1a2f27',
      accent: '#ff8f7a',
      accentText: '#0f1a16',
      backdrop: 'linear-gradient(160deg, #0f1a16 0%, #143529 60%, #0f2a2a 100%)',
    }),
  },
];

export const DEFAULT_THEME_ID = 'paper';

export function themeById(id: string | undefined | null): Theme {
  return THEMES.find((theme) => theme.id === id) ?? THEMES[0];
}

export function themeCssVariables(theme: Theme): Record<string, string> {
  const { ui } = theme;
  return {
    '--bg': ui.bg,
    '--bg-rail': ui.bgRail,
    '--bg-elevated': ui.bgElevated,
    '--bg-hover': ui.bgHover,
    '--border': ui.border,
    '--border-strong': ui.borderStrong,
    '--text': ui.text,
    '--text-muted': ui.textMuted,
    '--text-faint': ui.textFaint,
    '--accent': ui.accent,
    '--accent-text': ui.accentText,
    '--accent-soft': ui.accentSoft,
    '--success': ui.success,
    '--warn': ui.warn,
    '--danger': ui.danger,
    '--backdrop': ui.backdrop ?? ui.bgRail,
  };
}
