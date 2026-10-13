// The floating panel (every state) and the tray menu, over a fake shell.
// Usage: node shoot-panel-tray.mjs <base-url> <out-dir>
import { chromium } from '@playwright/test';
import { mkdirSync, writeFileSync } from 'node:fs';
import { TRAY_THEMES, trayPayload } from './tray-demo.mjs';

const [base = 'http://127.0.0.1:15301/', out = './shots'] = process.argv.slice(2);
mkdirSync(`${out}/parts`, { recursive: true });
const browser = await chromium.launch();
const THEMES = TRAY_THEMES;
const fakeShell = () => {
  const listeners = {};
  window.__TAURI__ = {
    event: { listen: async (name, cb) => { (listeners[name] ||= []).push(cb); return () => {}; } },
    core: { invoke: async (command) => (command === 'tray_menu_ready' ? window.__trayPayload ?? null : null) },
  };
  window.__emit = (name, payload) => (listeners[name] || []).forEach(cb => cb({ payload }));
};
const snapshot = (state, revision = 1) => ({ version: 1, kind: 'snapshot', epoch: 'e', session_id: 's', job_id: null, revision, state, audio_saved: true, error_code: state === 'error' ? 'capture_failed' : null });

const PANEL_STATES = [
  ['opening', [['writer-session', snapshot('requesting')]]],
  ['listening', [['writer-session', snapshot('capturing')], ['level', 0.03]]],
  ['listening-hover', [['writer-session', snapshot('capturing')], ['level', 0.02], ['hover']]],
  ['meter-unavailable', [['writer-session', snapshot('capturing')], ['wait', 1900]]],
  ['transcribing', [['writer-session', snapshot('transcribing')]]],
  ['saved', [['writer-session', snapshot('saved')]]],
  ['error', [['writer-session', snapshot('error')]]],
  ['pasted', [['writer-session', snapshot('transcribing')], ['writer-notice', { session_id: 's', text: 'Pasted', detail: 'Your text is in.', ms: 60000, tone: 'done' }]]],
  ['copied', [['writer-session', snapshot('transcribing')], ['writer-notice', { session_id: 's', text: 'Copied', detail: 'Copied to clipboard.', ms: 4000 }]]],
  ['waiting', [['writer-session', snapshot('capturing')], ['level', 0.02], ['writer-waiting', { count: 1 }]]],
  ['always-idle', [['writer-preferences', { auto_dismiss: true, completion_hold_ms: 1200, reduced_motion: false, visibility: 'always', scale: 1 }], ['writer-session', snapshot('idle')]]],
];

const parts = [];
for (const [themeName, theme] of Object.entries(THEMES)) {
  for (const [name, steps] of PANEL_STATES) {
    const page = await browser.newPage({ viewport: { width: 112, height: 44 }, deviceScaleFactor: 4 });
    await page.addInitScript(fakeShell);
    await page.goto(new URL('desktop-overlay.html', base).href, { waitUntil: 'networkidle' });
    await page.evaluate(t => window.__emit('writer-theme', t), theme);
    await page.evaluate(() => window.__emit('writer-preferences', { auto_dismiss: false, completion_hold_ms: 60000, reduced_motion: false, visibility: 'recording', scale: 1 }));
    let sequence = 0;
    for (const [kind, payload] of steps) {
      if (kind === 'level') {
        for (let i = 0; i < 6; i++) await page.evaluate(([rms, seq]) => window.__emit('writer-session', { version: 1, kind: 'level', epoch: 'e', session_id: 's', revision: 1, sequence: seq, rms: rms * (0.6 + Math.random()), peak: rms * 2 }), [payload, ++sequence]);
      } else if (kind === 'hover') await page.hover('section');
      else if (kind === 'wait') await page.waitForTimeout(payload);
      else await page.evaluate(([n, p]) => window.__emit(n, p), [kind, payload]);
    }
    await page.waitForTimeout(500);
    const file = `parts/panel-${themeName}-${name}.png`;
    await page.screenshot({ path: `${out}/${file}`, omitBackground: true });
    parts.push({ group: `panel-${themeName}`, name, file });
    await page.close();
  }
}

// Tray menu: the root while ready, recording and failed, and three submenus.
for (const [themeName, theme] of Object.entries(THEMES)) {
  for (const [name, state, drill] of [['idle', 'ready'], ['recording', 'recording'], ['failed', 'failed'], ['model', 'ready', 'Speech model'], ['mode', 'ready', 'Mode'], ['preferences', 'ready', 'Preferences']]) {
    const page = await browser.newPage({ viewport: { width: 420, height: 760 }, deviceScaleFactor: 2 });
    await page.addInitScript(fakeShell);
    await page.addInitScript(payload => { window.__trayPayload = payload; }, trayPayload(state, theme));
    await page.goto(new URL('desktop-tray.html?bare', base).href, { waitUntil: 'networkidle' });
    await page.waitForTimeout(300);
    if (drill) await page.locator('.row .label').getByText(drill, { exact: true }).click();
    await page.mouse.move(400, 740);
    await page.waitForTimeout(200);
    const box = await page.locator('#panel').boundingBox();
    const file = `parts/tray-${themeName}-${name}.png`;
    await page.screenshot({ path: `${out}/${file}`, clip: { x: Math.max(0, box.x - 14), y: Math.max(0, box.y - 14), width: box.width + 28, height: box.height + 28 }, omitBackground: true });
    parts.push({ group: `tray-${themeName}`, name, file });
    await page.close();
  }
}
writeFileSync(`${out}/parts.json`, JSON.stringify(parts, null, 2));

// Composite sheets: the panel over a light and a dark "desktop", the tray beside it.
const sheet = (group, background) => `<!doctype html><meta charset=utf-8><style>
body{margin:0;padding:28px;background:${background};font:600 13px/1.4 system-ui,sans-serif;color:${background === '#ffffff' ? '#333' : '#ddd'}}
.grid{display:grid;grid-template-columns:repeat(5,auto);gap:22px 28px;justify-content:start}
figure{margin:0}figcaption{margin-top:6px;font-weight:500;opacity:.8}img{width:224px;height:88px;display:block}
</style><div class=grid>${parts.filter(p => p.group === group).map(p => `<figure><img src="${p.file}"><figcaption>${p.name}</figcaption></figure>`).join('')}</div>`;
for (const [group, bg] of [['panel-paper', '#ffffff'], ['panel-dracula', '#1e1f29']]) {
  writeFileSync(`${out}/${group}.html`, sheet(group, bg));
  const page = await browser.newPage({ viewport: { width: 1400, height: 400 }, deviceScaleFactor: 1 });
  await page.goto(`file://${out}/${group}.html`);
  await page.screenshot({ path: `${out}/${group}.png`, fullPage: true });
  await page.close();
}
const traySheet = (theme, bg) => `<!doctype html><meta charset=utf-8><style>body{margin:0;padding:24px;background:${bg};display:flex;gap:24px;align-items:flex-start}img{height:auto;width:auto;max-height:720px}</style>${parts.filter(p => p.group === `tray-${theme}`).map(p => `<img src="${p.file}" style="zoom:.5">`).join('')}`;
for (const [theme, bg] of [['paper', '#cfd6df'], ['dracula', '#1e1f29']]) {
  writeFileSync(`${out}/tray-${theme}.html`, traySheet(theme, bg));
  const page = await browser.newPage({ viewport: { width: 1400, height: 760 }, deviceScaleFactor: 1 });
  await page.goto(`file://${out}/tray-${theme}.html`);
  await page.screenshot({ path: `${out}/tray-${theme}.png`, fullPage: true });
  await page.close();
}
console.log('done', parts.length);
await browser.close();
