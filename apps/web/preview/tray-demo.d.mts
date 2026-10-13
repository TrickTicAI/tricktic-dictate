// Types for tray-demo.mjs, which the tray page's browser tests import.
export type TrayTheme = Record<string, string>;
export type TrayRow = Record<string, unknown>;
export interface TrayOptions { platform?: 'windows' | 'macos'; now?: number }
export declare const TRAY_STATES: readonly string[];
export declare const TRAY_THEMES: { paper: TrayTheme; dracula: TrayTheme };
export declare function trayRows(state?: string, options?: TrayOptions): TrayRow[];
export declare function trayPayload(state?: string, theme?: TrayTheme, options?: TrayOptions): { rows: TrayRow[] } & Record<string, unknown>;
