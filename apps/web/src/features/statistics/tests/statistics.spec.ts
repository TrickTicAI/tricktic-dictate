import { test, expect, type Page } from '@playwright/test';
import type { InputLanguage, Job } from '@metrix-writer/contracts';
import type { Recording } from '../../../recorder';
import { edit, edits, recording, job, failed } from './fixtures';
import { change, duration, heatmap, jobTime, jobsFor, languageShare, niceMax, percentile, previousRecordings, selectedRecordings, speechModels, streaks, textModelUse, timeSaved, timeSpent } from '../metrics';
import { sessionStatistics } from '../../home/statistics';
import { textModelName } from '../../home/language';
const url = '/src/features/statistics/tests/statistics.html';
async function open(page: Page, query = '') {
 await page.route('**/api/v1/jobs/*/processing', route => {
 const id = new URL(route.request().url()).pathname.split('/').at(-2)!;
 return route.fulfill({json:{api_version:'1',data:edits[id as keyof typeof edits] ?? []}});
 });
 await page.goto(url + query);
}
const tiles = (page: Page) => page.locator('.tile .tile-figure');
const allMeasurements = (page: Page) => page.locator('.statistics-more > summary').click();

test('metrics deduplicate retries, preserve Hebrew words, and exclude unmeasured times', () => {
 const r=recording('a'),j=job('a','a','שלום hello'),attempts={job:failed,history:[j,j]};
 expect(jobsFor(attempts)).toHaveLength(2);
 expect(sessionStatistics([r,r],{a:attempts})).toMatchObject({words:2,transcribed:1,recordings:1});
 expect(jobTime({...j,updated_at_ms:j.created_at_ms-1})).toBeNull();
 if(j.status.state === 'succeeded') expect(jobTime({...j,status:{state:'succeeded',result:{...j.status.result,cleanup:{state:'pending'}}}})).toBeNull();
 expect(percentile([100,200,900],.5)).toBe(200);expect(percentile([],.95)).toBeNull();
 expect(duration(null)).toBe('—');expect(duration(0)).toBe('0 ms');
 expect(selectedRecordings([r,recording('older',40)],'7')).toHaveLength(1);
});

/* A recording at a given local day and hour, for the arithmetic below. */
function at(id: string, now: Date, daysAgo: number, hour = 10, language: InputLanguage = 'en'): Recording {
 const created = new Date(now); created.setDate(created.getDate() - daysAgo); created.setHours(hour, 15, 0, 0);
 return { ...recording(id), language, createdAt: created, metadata: { ...recording(id).metadata, created_at_ms: created.getTime(), input_language: language } };
}

test('periods, changes, streaks, the hour map, languages and time saved are plain arithmetic', () => {
 const now = new Date('2026-09-30T12:00:00');
 // Today, yesterday, the day before; then a gap; then two days in a row.
 const days = [0, 1, 2, 5, 6].map(d => at(`s${d}`, now, d));
 expect(streaks(days, now)).toEqual({ current: 3, longest: 3 });
 // Nothing yet today: the run that ended yesterday still counts.
 expect(streaks(days.filter(r => r.id !== 's0'), now)).toEqual({ current: 2, longest: 2 });
 expect(streaks([], now)).toEqual({ current: 0, longest: 0 });

 const previous = previousRecordings([at('p6', now, 6), at('p7', now, 7), at('p13', now, 13), at('p14', now, 14)], '7', now)!;
 expect(previous.map(r => r.id).sort()).toEqual(['p13', 'p7']);
 expect(previousRecordings(days, 'all', now)).toBeNull();
 expect(change(12, 10)).toBe(20); expect(change(5, 0)).toBeNull(); expect(change(null, 3)).toBeNull(); expect(change(8, 10)).toBe(-20);

 expect(niceMax(0)).toBe(1); expect(niceMax(173)).toBe(200); expect(niceMax(230)).toBe(250); expect(niceMax(1200)).toBe(2000);
 expect(timeSpent(45_000)).toBe('45 s'); expect(timeSpent(38 * 60_000)).toBe('38 min'); expect(timeSpent(252 * 60_000)).toBe('4 h 12 min'); expect(timeSpent(120 * 60_000)).toBe('2 h');
 // 400 words typed at 40 wpm is 10 minutes; one minute was spent speaking them.
 expect(timeSaved(400, 60_000, 40)).toBe(9 * 60_000);
 expect(timeSaved(10, 600_000, 40)).toBe(0);

 const monday = at('mon', new Date('2026-09-28T12:00:00'), 0, 9);
 const hebrew = at('he', new Date('2026-09-29T12:00:00'), 0, 21, 'he');
 const saved = { mon: { job: job('job-mon', 'mon', 'one two three four') }, he: { job: job('job-he', 'he', 'שלום עולם') } };
 const grid = heatmap([monday, hebrew], saved);
 expect(grid[0][9]).toBe(4); expect(grid[1][21]).toBe(2);
 expect(grid.flat().reduce((a, b) => a + b, 0)).toBe(6);
 expect(languageShare([monday, hebrew], saved)).toEqual([{ id: 'en', words: 4, share: 67 }, { id: 'he', words: 2, share: 33 }]);
});

test('speech models report median, 90% and success, and text model names read as names', () => {
 const fast = job('fast', 'a', 'x', 400), slow = job('slow', 'b', 'x', 2000), broken: Job = { ...failed, id: 'broken' };
 const [model] = speechModels([fast, slow, broken]);
 expect(model).toMatchObject({ jobs: 3, succeeded: 2, ended: 3, median: 400, p90: 2000 });
 expect(textModelName('mistralai/mistral-medium-3-5')).toBe('Mistral Medium 3.5');
 expect(textModelName('gemini-3.8-flash')).toBe('Gemini 3.8 Flash');
 expect(textModelName('openai/gpt-5-mini')).toBe('GPT-5 mini');
});

test('tiles, the period switch and reliability reflect retained history', async ({page}) => {
 await open(page);
 // The page opens on the last seven days.
 const period = page.getByRole('group', { name: 'Time period' });
 await expect(period.getByRole('button', { name: '7 days' })).toHaveAttribute('aria-pressed', 'true');
 await expect(tiles(page)).toHaveText(['6', '3 min', '3wpm', '≈ 0 min']);
 const reliability = page.getByRole('region', { name: 'Reliability' });
 await expect(reliability).toContainText('50%');
 await expect(reliability).toContainText('2 / 3');
 await reliability.getByRole('button', { name: 'Open failed dictations' }).click();
 await expect(page.locator('body')).toHaveAttribute('data-history', 'attention');
 await expect(page.getByRole('region', { name: 'Speech models' }).locator('tbody tr')).toHaveCount(1);

 await period.getByRole('button', { name: 'All history' }).click();
 await expect(tiles(page)).toHaveText(['9', '4 min', '3wpm', '≈ 0 min']);
 await page.reload();
 await expect(period.getByRole('button', { name: 'All history' })).toHaveAttribute('aria-pressed', 'true');

 // The typing speed behind "time saved" is yours to set.
 await page.getByRole('button', { name: '40 wpm' }).click();
 await page.getByRole('dialog', { name: 'Typing speed' }).getByRole('spinbutton').fill('20');
 await page.keyboard.press('Escape');
 await expect(page.getByRole('button', { name: '20 wpm' })).toBeVisible();
});

test('the words chart has a scale, today, and a tooltip for every day', async ({page}) => {
 await open(page);
 const chart = page.getByRole('group', { name: /Words per day/ });
 await expect(chart.locator('.chart-bar')).toHaveCount(7);
 await expect(chart.locator('.chart-bar.today')).toHaveCount(1);
 await expect(chart.locator('.chart-y span').first()).toHaveText('5');
 await chart.locator('.chart-bar.today').focus();
 await expect(chart.getByRole('tooltip')).toContainText('4 words');
 const card = page.getByRole('region', { name: 'Words per day' });
 await card.getByText('Daily totals').click();
 await expect(card.locator('.statistics-daily-grid p')).toHaveCount(7);
});

test('session details open from All measurements and lead back to history', async ({page}) => {
 await open(page);
 await allMeasurements(page);
 await expect(page.getByRole('region', { name: 'Super mode statistics' })).toContainText('1 / 2');
 await expect(page.getByRole('region', { name: 'Super mode statistics' })).toContainText('200 / 40');
 await expect(page.locator('.statistics-sessions tbody tr')).toHaveCount(3);
 await page.getByRole('button',{name:'Details',exact:true}).first().click();
 const dialog=page.getByRole('dialog',{name:'Session timings'});
 await expect(dialog).toContainText('800 ms');await expect(dialog).toContainText('200 ms');
 await expect(dialog).toContainText('Silence shortened');await expect(dialog).toContainText('42.00 → 4.00 MB');
 await page.getByRole('button',{name:'Open in history'}).click();
 await expect(page.locator('body')).toHaveAttribute('data-opened','a');
 await page.getByRole('button',{name:'Details',exact:true}).nth(1).click();
 await expect(dialog).toContainText('Not recorded');await page.keyboard.press('Escape');
 await expect(page.getByRole('button',{name:'Details',exact:true}).nth(1)).toBeFocused();
});

test('text models compare use over time, success and the time of their own requests', async ({page}) => {
 const luna = {provider:'openai',model:'gpt-5.6-luna'} as const, mistral = {provider:'openrouter',model:'mistralai/mistral-medium-3-5'} as const;
 const byJob: Record<string, unknown[]> = {
  // 200 ms is the fixture's reused early draft: counted as use, not as request time.
  'job-a': [edit('luna-a','job-a',1500,luna), edit('mistral-a','job-a',600,mistral), edit('reused-a','job-a',200,mistral)],
  'job-b': [edit('mistral-b','job-b',800,mistral), {...edit('failed-b','job-b',undefined,mistral), state:{state:'failed',error:{message:'Synthetic',retryable:true}}}],
  // Forty days old: in the model totals for all history, outside the 30-day chart.
  'job-d': [edit('luna-d','job-d',1300,luna)],
 };
 await page.route('**/api/v1/jobs/*/processing', route => route.fulfill({json:{api_version:'1',data:byJob[new URL(route.request().url()).pathname.split('/').at(-2)!] ?? []}}));
 await page.goto(url + '?period=all');
 const card = page.getByRole('region',{name:'Text model statistics'});
 const rows = card.locator('.statistics-text-models li');
 await expect(rows).toHaveCount(2);
 await expect(rows.nth(0)).toContainText('Mistral Medium 3.5');
 await expect(rows.nth(0)).toContainText('OpenRouter · 3 successful / 4 edits · 75% succeeded');
 await expect(rows.nth(0).locator('dd')).toHaveText(['600 ms','800 ms']);
 await expect(rows.nth(1)).toContainText('OpenAI · 2 successful / 2 edits · 100% succeeded');
 await expect(rows.nth(1).locator('dd')).toHaveText(['1.3 s','1.5 s']);
 await expect(rows.nth(0).locator('.latency-median')).toHaveCount(1);
 await expect(card).toContainText('Times cover 4 successful model requests');
 await allMeasurements(page);
 const daily = page.getByRole('region', { name: 'Daily model use' });
 await expect(daily.getByRole('img',{name:/Polishing attempts per day/})).toHaveAttribute('aria-label',/5 attempts over 30 days/);
 await daily.getByText('Daily model use').click();
 const days = daily.locator('.statistics-daily-grid p');
 await expect(days).toHaveCount(2);
 await expect(days.nth(0)).toContainText('Mistral Medium 3.5 2');
 await expect(days.nth(1)).toContainText('Mistral Medium 3.5 2 · GPT-5.6 luna 1');
 await page.getByRole('group', { name: 'Time period' }).getByRole('button', { name: '7 days' }).click();
 await expect(rows).toHaveCount(2);
 await expect(rows.nth(1)).toContainText('1 successful / 1 edit ·');
 await expect(daily.getByRole('img',{name:/Polishing attempts per day/})).toHaveAttribute('aria-label',/5 attempts over 7 days/);
 await expect(daily.getByRole('img',{name:/Polishing attempts per day/}).locator('.model-activity-stack i')).toHaveCount(3);
});

test('a slow host is named beside its model, with what to change about it', async ({page}) => {
 const deepseek = {provider:'openrouter',model:'deepseek-v4.1-flash'} as const;
 const byJob: Record<string, unknown[]> = {
  'job-a': [edit('fast-1','job-a',500,deepseek,'DeepInfra'), edit('fast-2','job-a',600,deepseek,'DeepInfra'), edit('fast-3','job-a',550,deepseek,'DeepInfra')],
  'job-b': [edit('slow-1','job-b',6000,deepseek,'Relace'), edit('slow-2','job-b',6500,deepseek,'Relace'), edit('slow-3','job-b',7000,deepseek,'Relace')],
  'job-d': [edit('stood-in','job-d',700,deepseek,'DeepInfra',true)],
 };
 await page.route('**/api/v1/jobs/*/processing', route => route.fulfill({json:{api_version:'1',data:byJob[new URL(route.request().url()).pathname.split('/').at(-2)!] ?? []}}));
 await page.goto(url + '?period=all');
 const card = page.getByRole('region',{name:'Text model statistics'});
 await expect(card.locator('.statistics-hosts')).toHaveText('DeepInfra: 550 ms over 4 · Relace: 6.5 s over 3');
 const advice = page.getByRole('list',{name:'Text model suggestions'});
 await expect(advice.getByRole('listitem').first()).toHaveText('deepseek-v4.1-flash took 6.5 s on Relace and 550 ms on DeepInfra. In Manage custom models you can pin DeepInfra or avoid Relace.');
 // A model that never needed its fallback is not told to change anything about it.
 await expect(advice.getByRole('listitem')).toHaveCount(1);
 await page.getByRole('region', { name: 'Suggestions' }).getByRole('button', { name: 'Open Modes' }).click();
 await expect(page.locator('body')).toHaveAttribute('data-settings', 'modes');
});

test('empty and unavailable processing history are explicit and retryable', async ({page}) => {
 await open(page,'?empty');
 await expect(page.getByText('Your first dictation starts the picture')).toBeVisible();
 await expect(tiles(page)).toHaveText(['0','0 min','—','≈ 0 min']);
 await page.route('**/api/v1/jobs/*/processing',route=>route.fulfill({status:503,json:{}}));
 await page.goto(url);await expect(page.getByText(/Processing history unavailable/)).toBeVisible();
 await page.unroute('**/api/v1/jobs/*/processing');
 await page.route('**/api/v1/jobs/*/processing',route=>route.fulfill({json:{api_version:'1',data:[]}}));
 await page.getByRole('button',{name:'Retry',exact:true}).click();
 await expect(page.getByText(/Processing history unavailable/)).toHaveCount(0);
});

test('layout fits light, dark, narrow and RTL layouts', async ({page}) => {
 for (const query of ['', '?theme=dracula', '?rtl']) {
 await open(page,query);
 for(const size of [{width:600,height:700},{width:1120,height:800},{width:1440,height:900},{width:1920,height:1080}]) {
 await page.setViewportSize(size);
 await expect(page.getByRole('heading',{name:'Statistics',exact:true})).toBeVisible();
 expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth&&document.documentElement.scrollHeight<=innerHeight)).toBe(true);
 expect(await page.locator('.statistics-content').evaluate(el=>el.scrollWidth<=el.clientWidth)).toBe(true);
 }
 await allMeasurements(page);
 await page.getByRole('button',{name:'Details',exact:true}).first().click();
 await expect(page.getByRole('dialog')).toBeVisible();await page.keyboard.press('Escape');
 }
});

test('model statistics credit the backup winner and count original recovery as failed polishing', () => {
 const a = edit('fallback-winner', 'job-a', 800);
 const primary = { provider: 'openai' as const, model: 'gpt-4.1-mini' }, backup = { provider: 'gemini' as const, model: 'flash-test' };
 a.profile.text_model = primary; a.profile.fallback_model = backup;
 a.diagnostics = { processing_ms:800,reused_draft:false,fallback_used:true,selected_model:backup,model_attempts:[
  {selection:primary,role:'primary',state:'failed',error:'rate_limited',elapsed_ms:10,served_by:null},
  {selection:backup,role:'fallback',state:'succeeded',error:null,elapsed_ms:790,served_by:null},
 ]};
 const original = {...a,id:'original',diagnostics:{...a.diagnostics,original_used:true,fallback_used:false,selected_model:null,model_attempts:a.diagnostics.model_attempts!.map(c=>({...c,state:'failed',error:'provider_failed'}))}};
 const models = textModelUse([a,original]);
 expect(models.find(m=>m.key==='openai/gpt-4.1-mini')).toMatchObject({attempts:2,succeeded:0,failed:2,times:[]});
 expect(models.find(m=>m.key==='gemini/flash-test')).toMatchObject({attempts:2,succeeded:1,failed:1,times:[790]});
});

test('stage export downloads the bounded timing projection without history text', async ({ page }) => {
 const timings = { records: [{ attempt_id: 'timing-only', stages_ms: { asr: 23 } }] };
 await page.route('**/api/v1/statistics/latency', route => route.fulfill({ json: { api_version: '1', data: timings } }));
 await open(page);
 const download = page.waitForEvent('download');
 await page.getByRole('button', { name: 'Export stage timings' }).click();
 const file = await download;
 expect(file.suggestedFilename()).toBe('dictation-stage-timings.json');
 const stream = await file.createReadStream();
 const chunks: Buffer[] = []; for await (const chunk of stream!) chunks.push(chunk);
 expect(JSON.parse(Buffer.concat(chunks).toString())).toEqual(timings);
});
