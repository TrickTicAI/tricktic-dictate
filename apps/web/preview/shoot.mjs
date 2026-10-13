// Screenshots of the desktop UI over the fake host: every page and every Settings
// section in two themes at two sizes (SIZES=1440x900,1920x1080), small windows
// and the first-run guide.
// Usage: node preview/shoot.mjs <base-url> <out-dir>   (ONLY=1 shoots just the guide)
import { chromium } from '@playwright/test';
import { mkdirSync } from 'node:fs';

const [base = 'http://127.0.0.1:15301/', out = './shots'] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
const browser = await chromium.launch();
const problems = [];

async function open({ theme = 'paper', width = 1440, height = 900, onboarding = false, rtl = false } = {}) {
  const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 1 });
  page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));
  await page.addInitScript(([theme, onboarding]) => {
    try { localStorage.setItem('writer-theme', theme); } catch { /* preview */ }
    window.__FAKE_LOG = false;
    if (onboarding) window.__FAKE_ONBOARDING = true;
  }, [theme, onboarding]);
  page.rtl = rtl;
  return page;
}
async function shot(page, url, name, prepare) {
  await page.goto(new URL(url, base).href, { waitUntil: 'networkidle' });
  await page.waitForTimeout(700);
  if (prepare) await prepare(page);
  await page.waitForTimeout(400);
  await page.screenshot({ path: `${out}/${name}.png` });
  console.log('saved', name);
}

// Every screen in Paper and Dracula, at 1440 × 900 and 1920 × 1080.
const SETTINGS = ['engine', 'shortcuts', 'microphone', 'panel', 'modes', 'text', 'keys', 'general', 'appearance', 'account', 'data', 'about'];
const SIZES = (process.env.SIZES ?? '1440x900,1920x1080').split(',').map((size) => size.split('x').map(Number));
for (const theme of (process.env.ONLY ? [] : ['paper', 'dracula'])) {
  for (const [width, height] of SIZES) {
    const page = await open({ theme, width, height });
    const tag = `${theme}-${width}`;
    await shot(page, '?tab=home', `home-${tag}`);
    await shot(page, '?tab=home', `home-reader-${tag}`, p => p.locator('.session-row').nth(1).click().catch(() => {}));
    await shot(page, '?tab=statistics', `statistics-${tag}`);
    for (const sub of ['words', 'corrections', 'snippets']) {
      await shot(page, `?tab=dictionary&sub=${sub}`, `dictionary-${sub}-${tag}`);
    }
    for (const section of SETTINGS) {
      await shot(page, `?settings=${section}`, `settings-${section}-${tag}`);
    }
    await page.close();
  }
}

// States worth seeing once: the long statistics page, recording, a right-click menu.
if (!process.env.ONLY) {
  const page = await open();
  await shot(page, '?tab=statistics', 'statistics-scrolled', async p => { await p.mouse.move(700, 600); await p.mouse.wheel(0, 900); });
  await shot(page, '?tab=home', 'home-recording', async p => {
    await p.getByRole('button', { name: /^(Record|Start)/ }).first().click().catch(() => {});
    await p.waitForTimeout(2600);
  });
  await shot(page, '?tab=home', 'home-context-menu', async p => {
    const row = p.locator('.session-row').first();
    const box = await row.boundingBox().catch(() => null);
    if (box) await p.mouse.click(box.x + 60, box.y + 20, { button: 'right' });
  });
  await page.close();
}

// Smaller window and Hebrew interface direction.
if (!process.env.ONLY) {
  const page = await open({ width: 1024, height: 700 });
  await shot(page, '?tab=home', 'home-1024');
  await page.close();
  const narrow = await open({ width: 700, height: 640 });
  await shot(narrow, '?tab=home', 'home-narrow');
  await narrow.close();
}

// First-run guide: walk it with the footer's primary button.
{
  const page = await open({ onboarding: true });
  await page.goto(new URL('?tab=home', base).href, { waitUntil: 'networkidle' });
  await page.waitForTimeout(900);
  for (let step = 1; step <= 6; step++) {
    await page.screenshot({ path: `${out}/onboarding-${step}.png` });
    console.log('saved', `onboarding-${step}`);
    const next = page.locator('.ob-actions button.primary, footer button.primary, button.button.primary').last();
    if (!(await next.isEnabled().catch(() => false))) { console.log('blocked at step', step); break; }
    await next.click().catch(() => {});
    await page.waitForTimeout(900);
  }
  await page.close();
}

console.log(JSON.stringify({ problems: problems.slice(0, 30) }, null, 2));
await browser.close();
