// The real desktop UI in a browser, over a fake Tauri host and fake API with
// made-up data (see README.md). Nothing here reaches a real backend, microphone,
// keychain or account. Not part of any build.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { buildData } from './fake-data.mjs';
import { TRAY_STATES, TRAY_THEMES, trayPayload } from './tray-demo.mjs';

const web = fileURLToPath(new URL('../', import.meta.url));
// Read on every page load, so edits to the fake host apply without a restart
// and a long-running preview keeps "Today" on today.
const host = () => readFileSync(fileURLToPath(new URL('./fake-host.js', import.meta.url)), 'utf8');

/*
 * The tray menu page with the rows the shell would send, chosen in the query:
 * `?state=ready|recording|working|failed|starting`, `?theme=paper|dracula`,
 * `?platform=windows|macos`. A strip under the menu switches between them;
 * clicking a row only logs the id the shell would receive. `?bare` drops the
 * strip and the backdrop.
 */
function trayPage(url) {
  const pick = (name, allowed, fallback) => (allowed.includes(url.searchParams.get(name)) ? url.searchParams.get(name) : fallback);
  const state = pick('state', TRAY_STATES, 'ready');
  const theme = pick('theme', Object.keys(TRAY_THEMES), 'paper');
  const platform = pick('platform', ['windows', 'macos'], 'windows');
  const payload = trayPayload(state, TRAY_THEMES[theme], { platform });
  const link = (name, value, label) => {
    const next = new URLSearchParams({ state, theme, platform, [name]: value });
    const current = { state, theme, platform }[name] === value;
    return `<a href="?${next}"${current ? ' aria-current="true"' : ''}>${label}</a>`;
  };
  const strip = `<nav id="demo" aria-label="Preview states">${TRAY_STATES.map((value) => link('state', value, value)).join('')}<i></i>${link('theme', 'paper', 'Paper')}${link('theme', 'dracula', 'Dracula')}<i></i>${link('platform', 'windows', 'Windows')}${link('platform', 'macos', 'macOS')}</nav>`;
  const style = `<style>html,body{overflow:auto!important;background:${theme === 'dracula' ? '#1e1f29' : '#e9e4dc'}!important}#demo{position:fixed;inset:auto 12px 12px 12px;display:flex;flex-wrap:wrap;gap:6px;font:12px system-ui;z-index:9}#demo a{padding:4px 9px;border-radius:999px;background:#ffffffcc;color:#222;text-decoration:none}#demo a[aria-current]{background:#222;color:#fff}#demo i{width:10px}</style>`;
  // A page that already has a shell, such as a screenshot run's, keeps it.
  const shell = `<script>window.__TAURI__??={event:{listen:async()=>()=>{}},core:{invoke:async(command,args)=>{if(command==='tray_menu_ready')return ${JSON.stringify(payload)};if(command==='tray_menu_command')console.info('tray command',args.id);return null;}}};</script>`;
  const page = readFileSync(fileURLToPath(new URL('../public/desktop-tray.html', import.meta.url)), 'utf8');
  // `?bare` leaves the page as the shell shows it, for screenshots.
  const chrome = url.searchParams.has('bare') ? '' : `${style}${strip}`;
  return page.replace('<body>', `<body>${shell}${chrome}`).replace(/<title>[^<]*<\/title>/, '<title>Tray menu preview</title>');
}

export default defineConfig({
  root: web,
  base: process.env.DEVGW_BASE ?? '/',
  plugins: [react(), {
    name: 'fake-desktop-host',
    enforce: 'pre',
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        const url = new URL(request.url ?? '/', 'http://preview');
        if (!url.pathname.endsWith('/desktop-tray.html')) return next();
        response.setHeader('Content-Type', 'text/html; charset=utf-8');
        response.end(trayPage(url));
      });
    },
    transform(code, id) {
      // The real check also requires a tauri.localhost origin, which a browser never has.
      if (id.endsWith('/desktop-session/runtime/native.ts')) {
        return code.replace(/export const isTauri = \(\) => [^;]+;/, 'export const isTauri = () => !!window.__TAURI__;');
      }
    },
    transformIndexHtml: {
      order: 'pre',
      // `?history=N` (up to 2000) makes the history that long, for measuring long lists.
      handler: (html, context) => {
        const asked = Number(new URL(context.originalUrl ?? '/', 'http://preview').searchParams.get('history'));
        const history = Number.isFinite(asked) ? Math.min(2000, Math.max(28, Math.round(asked))) : 28;
        return html.replace('<head>', `<head><script>window.__FAKE_DATA__=${JSON.stringify(buildData(Date.now(), { history }))};${host()}</script>`);
      },
    },
  }],
  server: { hmr: false, allowedHosts: true, fs: { allow: [fileURLToPath(new URL('../../../', import.meta.url))] } },
});
