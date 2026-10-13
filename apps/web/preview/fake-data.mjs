// Made-up data for the fake desktop host. Nothing here is a real recording,
// transcript, key or account. Built in Node and inlined into the page.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const catalog = JSON.parse(readFileSync(`${root}apps/api/src/desktop_settings/model-catalog.json`, 'utf8'));

const DAY = 86_400_000;
// Anchor "now" to the real clock so "Today" groups line up in the preview.
const uuid = (prefix, n) => `${prefix}-0000-4000-8000-${n.toString(16).padStart(12, '0')}`;
/** `history` repeats the sample dictations further back, for measuring long lists. */
export function buildData(now = Date.now(), { history = 28 } = {}) {
  const MODEL = 'whisper-large-v3-turbo-q5-0';
  const selection = { provider: 'local', model: MODEL };
  const english = [
    'Hey Maya, thanks for sending the deck. I went through the pricing slides and I think we should lead with the annual plan, then show the monthly option as the fallback. Can we sync tomorrow at ten?',
    'Note to self: the onboarding flow drops people at the microphone step. Try asking for permission only after they press record for the first time.',
    'Remind me to call the accountant about the quarterly VAT report before Thursday.',
    'The build is green on main. I merged the reliability card and kicked off the Windows installer. Release notes are in the changelog.',
    'Let’s move the stand-up to 9:30 so the Tel Aviv team can join before their lunch.',
    'Draft reply: Thanks for the quick turnaround. The second version reads much better. One small thing, the chart on page four is missing its axis labels.',
    'Shopping list: oat milk, two avocados, coffee beans, and the good olive oil.',
    'In the settings page, the search should find a row even when I misspell clipboard. Make sure typos within one edit still match.',
    'Can you check whether the dictionary correction for Tailscale applies when it is the first word of the sentence?',
    'Meeting summary: we agreed to ship the Mac build to testers on Monday, keep the panel at the bottom by default, and revisit the colours after feedback.',
    'Quick thought for the blog post: speaking is three times faster than typing, but only if the text lands where your cursor is without any extra steps.',
    'Please book a table for four at eight, somewhere quiet near Rothschild.',
    'Function name should be resolveDeliveryTarget, and it returns the window that was in front when the dictation stopped.',
    'I tried the new local model on the laptop. Hebrew is noticeably better, English is about the same, and it is roughly twice as fast on the GPU.',
  ];
  const hebrew = [
    'היי נועה, אפשר להזיז את הפגישה של מחר לשעה ארבע? יש לי שיחה עם לקוח בשלוש וחצי.',
    'תזכורת: לשלם את חשבון החשמל עד סוף השבוע ולבדוק למה החיוב קפץ החודש.',
    'סיכום שיחה עם דני: מתחילים את הגרסה החדשה ביום ראשון, והוא שולח את העיצובים עד יום חמישי.',
    'לקנות מתנה ליום ההולדת של סבתא, אולי ספר בישול או צעיף חם לחורף.',
    'רעיון לפוסט: איך הכתבה קולית חוסכת לי שעה ביום, עם שלוש דוגמאות מהעבודה.',
    'תודה רבה על העזרה אתמול, זה ממש הציל אותי לפני ההגשה.',
  ];
  const mixed = [
    'צריך לבדוק את ה-pull request של ה-dashboard לפני ה-merge, במיוחד את ה-loading states.',
    'שלחתי ל-Maya את ה-invoice, תבדוק שה-VAT נכון לפני שהיא מעבירה ל-finance.',
  ];

  // A spread of sessions over the last 15 days, newest first.
  const plan = [];
  const offsets = [0.02, 0.05, 0.11, 0.2, 0.31, 0.45, 0.9, 1.05, 1.2, 1.4, 1.75, 2.1, 2.6, 3.05, 3.4, 4.2, 4.6, 5.1, 6.3, 6.8, 7.2, 8.4, 9.1, 10.5, 11.2, 12.6, 13.3, 14.1];
  for (let extra = 1; offsets.length < history; extra++) offsets.push(14.1 + extra * 0.37);
  let e = 0, h = 0, m = 0;
  offsets.forEach((days, index) => {
    const kind = index % 5 === 1 ? 'he' : index % 7 === 3 ? 'mixed' : 'en';
    const text = kind === 'he' ? hebrew[h++ % hebrew.length] : kind === 'mixed' ? mixed[m++ % mixed.length] : english[e++ % english.length];
    const words = text.split(/\s+/).length;
    const duration = Math.round((words / 2.4) * 1000 + 1200 + (index % 3) * 900);
    plan.push({ id: uuid('5e7c0d1a', index + 1), created: Math.round(now - days * DAY), language: kind, text, duration });
  });
  // Make a few interesting states.
  const failedIndex = 4;      // transcription failed, audio kept
  const interruptedIndex = 9; // capture interrupted (app closed), audio kept
  const untranscribedIndex = 15; // saved, never transcribed (no model at the time)

  const recordings = plan.map((item, index) => ({
    id: item.id,
    byte_length: Math.round(item.duration * 32),
    capture_state: index === interruptedIndex ? 'interrupted' : 'complete',
    created_at_ms: item.created,
    duration_ms: item.duration,
    input_language: item.language,
    mime_type: 'audio/wav',
    chunk_count: 1,
  }));

  const superProfile = profiles(selection).profiles.find(p => p.kind === 'super');
  const attempts = plan.flatMap((item, index) => {
    if (index === untranscribedIndex) return [];
    const options = {
      cleanup: { mode: 'off' }, input_language: item.language, output_language: { mode: 'preserve' }, selection,
      vocabulary: { hints: ['Tailscale', 'TrickTic', 'Kowalczyk'], corrections: [{ from: 'tail scale', to: 'Tailscale' }], snippets: [] },
    };
    const request = { idempotency_key: `key-${item.id}`, recording_id: item.id, options };
    const created = item.created + item.duration + 400;
    const status = index === failedIndex
      ? { state: 'failed', error: { code: 'provider_unavailable', message: 'The local model worker did not answer.', retryable: true } }
      : { state: 'succeeded', result: { cleanup: { state: 'off' }, transcript: { execution_location: 'self_hosted_server', input_language: item.language, output_language: { mode: 'preserve' }, raw_text: rawFrom(item.text), selection } } };
    const job = { id: uuid('9b2f4c6e', index + 1), created_at_ms: created, updated_at_ms: created + 900 + (index % 4) * 350, request, status };
    return [{ id: item.id, request, job, history: [], capturedMode: index % 3 === 0 ? undefined : superProfile }];
  });

  // Super-processed text for sessions that captured the Super profile.
  const processing = {};
  for (const attempt of attempts) {
    if (!attempt.capturedMode || attempt.job.status.state !== 'succeeded') continue;
    const raw = attempt.job.status.result.transcript.raw_text;
    processing[attempt.job.id] = [{
      id: `proc-${attempt.id}`, job_id: attempt.job.id, created_at_ms: attempt.job.updated_at_ms + 50,
      profile: attempt.capturedMode,
      request: { operation_id: `op-${attempt.id}`, profile_id: attempt.capturedMode.id, profile_revision: 3, context: { application: '', selected_text: '', clipboard: '' } },
      diagnostics: { processing_ms: 640 + (raw.length % 9) * 70, first_response_ms: 310, response_complete_ms: 610, input_tokens: 220, output_tokens: 64, reused_draft: raw.length % 2 === 0, served_by: 'Groq', fallback_used: false, selected_model: { provider: 'openrouter', model: 'google/gemini-3.8-flash' }, delivery: { copied: true, pasted: true, at_ms: attempt.job.updated_at_ms + 700, stop_to_delivery_ms: 1180, app: ['Slack', 'Notes', 'Microsoft Outlook', 'Visual Studio Code'][raw.length % 4] } },
      raw_text: raw, input_text: raw, prompt_version: 1,
      state: { state: 'succeeded', text: polish(raw, plan.find(item => item.id === attempt.id)?.text) },
    }];
  }

  const models = catalog
    .filter(model => ['whisper-large-v3-turbo-q5-0', 'whisper-large-v3-turbo', 'ivrit-turbo', 'whisper-small', 'distil-large-v3-5', 'parakeet-tdt-0-6b-v3', 'whisper-tiny', 'whisper-medium-q5-0'].includes(model.id))
    .map(model => ({
      id: model.id, display_name: model.display_name, file_name: model.file_name ?? `${model.id}.bin`, bytes: model.bytes,
      sha256: model.sha256 ?? '0'.repeat(64), source_url: model.source_url ?? 'https://huggingface.co/', source_name: model.source_name ?? 'Hugging Face',
      license: model.license, notes: model.notes, input_languages: model.input_languages,
      metadata: model.metadata ?? null, smoke_tests: null, compatibility_issue: null,
      weights: model.id === MODEL ? { state: 'installed', bytes: model.bytes, sha256: '0'.repeat(64), verified_at_ms: now - 9 * DAY, origin: 'download' }
        : model.id === 'ivrit-turbo' ? { state: 'installed', bytes: model.bytes, sha256: '0'.repeat(64), verified_at_ms: now - 6 * DAY, origin: 'download' }
        : model.id === 'whisper-small' ? { state: 'downloading', downloaded_bytes: Math.round(model.bytes * 0.42), total_bytes: model.bytes }
        : { state: 'not_installed', resumable_bytes: null },
      selected: model.id === MODEL, loaded: model.id === MODEL,
    }));

  const settings = {
    ui_language: 'en',
    transcription: { selection, input_language: 'en', output_language: { mode: 'preserve' }, vocabulary: { hints: [], corrections: [], snippets: [] }, cleanup: { mode: 'off' } },
  };
  const setup = {
    mode: 'desktop', first_run: false, settings,
    runtime: {
      hardware: { architecture: 'x86_64', cpu_threads: 16, total_ram_bytes: 34_359_738_368, available_ram_bytes: 18_253_611_008, parakeet_cpu_supported: true },
      execution: { requested: 'automatic', cpu_available: true, gpu_available: true, fallback_reason: null },
      worker: { state: 'ready', model: MODEL, device: 'NVIDIA GeForce RTX 4070', backend: 'vulkan', started_at_ms: now - 3_600_000, restarts: 0 },
      problem: null, packaged: true, worker_executable_present: true, vulkan_library_present: true, ffmpeg_present: true, external_server: false,
      models_dir: 'C:\\Users\\Alex\\AppData\\Local\\TrickTic Dictate\\Models',
    },
    models,
    cloud: [
      { provider: 'openai', models: ['gpt-transcribe', 'gpt-4o-transcribe', 'gpt-4o-mini-transcribe', 'whisper-1'], credential: 'protected_store', check: { outcome: 'accepted', checked_at_ms: now - 2 * DAY }, verified: true, selected: false },
      { provider: 'gemini', models: ['gemini-3.8-flash'], credential: 'absent', check: null, verified: false, selected: false },
      { provider: 'grok', models: ['grok-speech-to-text'], credential: 'absent', check: null, verified: false, selected: false },
      { provider: 'openrouter', models: [], credential: 'protected_store', check: { outcome: 'accepted', checked_at_ms: now - DAY }, verified: false, selected: false },
    ],
  };
  const providers = [
    { id: 'local', name: 'On this PC', configured: true, available: true, status: 'ready', models: [MODEL, 'ivrit-turbo'] },
    { id: 'openai', name: 'OpenAI', configured: true, available: true, status: 'ready', models: ['gpt-transcribe', 'gpt-4o-transcribe', 'gpt-4o-mini-transcribe', 'whisper-1'] },
    { id: 'gemini', name: 'Gemini', configured: false, available: false, status: 'missing_credentials' },
    { id: 'grok', name: 'Grok', configured: false, available: false, status: 'missing_credentials' },
  ];
  const vocabulary = {
    hints: [
      { text: 'Tailscale', enabled: true }, { text: 'TrickTic', enabled: true }, { text: 'Kowalczyk', enabled: true },
      { text: 'Kubernetes', enabled: true }, { text: 'Vite', enabled: true }, { text: 'ivrit.ai', enabled: true },
      { text: 'Rothschild', enabled: true }, { text: 'שדרינסקי', enabled: true }, { text: 'OpenRouter', enabled: false },
    ],
    corrections: [
      { from: 'tail scale', to: 'Tailscale', enabled: true }, { from: 'trick tick', to: 'TrickTic', enabled: true },
      { from: 'free', to: 'three', enabled: false }, { from: 'trick tick dictate', to: 'TrickTic Dictate', enabled: true },
      { from: 'גיט האב', to: 'GitHub', enabled: true },
    ],
    snippets: [
      { phrase: 'my email', text: 'alex@example.com', enabled: true },
      { phrase: 'my address', text: '12 Rothschild Blvd, Tel Aviv-Yafo', enabled: true },
      { phrase: 'sign off', text: 'Best,\nAlex', enabled: true },
      { phrase: 'meeting link', text: 'https://meet.example.com/alex-weekly', enabled: false },
    ],
  };
  const preferences = {
    recording: 'Ctrl+Shift+F9', cancel: 'Ctrl+Shift+F10', toggle: false, paste_into: 'stop', preserve_clipboard: true, private_clipboard: false,
    visibility: 'recording', placement: 'bottom', position: null, scale: 1, reduced_motion: false, completion_hold_ms: 1200, auto_dismiss: true, panel_colors: null,
  };
  return { now, recordings, attempts, processing, setup, providers, vocabulary, modes: profiles(selection), preferences };
}

function rawFrom(text) {
  // Raw speech keeps fillers, loses most punctuation and starts lower-case;
  // polishing puts the written form back.
  return text.replace(/^I tried/, 'um I tried').replace(/^Quick thought/, 'so quick thought').replace(/\bTailscale\b/, 'tail scale')
    .replace(/[,:]/g, '').replace(/\.$/, '').replace(/^[A-Z]/, c => c.toLowerCase());
}
function polish(raw, written) {
  return written ?? raw.replace(/^um /, '').replace(/^so /, '').replace(/\btail scale\b/, 'Tailscale').replace(/^./, c => c.toUpperCase());
}
function profiles(selection) {
  const context = { application: true, selected_text: false, clipboard: false };
  const formatting = { rewrite: 'polished', layout: 'automatic', lists: true, remove_fillers: true, self_corrections: true, spoken_punctuation: true };
  return {
    revision: 7, active: 'super',
    profiles: [
      { id: 'regular', revision: 1, name: 'Regular', kind: 'regular', instructions: '', text_model: null, context: { application: false, selected_text: false, clipboard: false }, shortcut: null },
      { id: 'super', revision: 3, name: 'Super', kind: 'super', instructions: 'Fix punctuation, remove filler words and keep my wording.', text_model: { provider: 'openrouter', model: 'google/gemini-3.8-flash' }, fallback_model: { provider: 'openai', model: 'gpt-5.4-mini' }, fallback_after_ms: 1000, polish_timeout_ms: 3000, context, formatting, process_while_speaking: true, fast_processing: true, shortcut: null },
      { id: 'command', revision: 2, name: 'Command', kind: 'command', instructions: 'Apply my spoken instruction to the selected text.', text_model: { provider: 'openrouter', model: 'anthropic/claude-haiku-4.5' }, context: { application: true, selected_text: true, clipboard: false }, shortcut: 'Ctrl+Shift+F8' },
    ],
  };
}
