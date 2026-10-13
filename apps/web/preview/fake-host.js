// Fake Tauri host + fake API for browser previews. Runs before the app module.
// Every answer comes from window.__FAKE_DATA__; nothing reaches a real backend.
(() => {
  const data = window.__FAKE_DATA__;
  const log = (...args) => { if (window.__FAKE_LOG !== false) console.debug('[fake-host]', ...args); };
  const unknown = new Set();
  window.__fakeUnknown = unknown;
  const listeners = {};
  const emit = (name, payload) => (listeners[name] || []).forEach(cb => { try { cb({ payload }); } catch (e) { console.error(e); } });
  const clone = v => structuredClone(v);
  const state = {
    recordings: data.recordings, attempts: data.attempts, preferences: data.preferences,
    system: { launch_at_login: true, sounds: true, duck_audio: false },
    maximized: false,
    capture: null, // { id, started, language }
  };
  // What a made-up dictation "says", by language. Raw keeps the filler; Super removes it.
  const SCRIPT = {
    en: { raw: 'okay so for the launch post lets lead with the speed numbers then the Hebrew support and close with the download link', polished: 'For the launch post, let’s lead with the speed numbers, then Hebrew support, and close with the download link.' },
    he: { raw: 'תזכורת לעצמי לשלוח לנועה את המצגת עד מחר בבוקר ולבקש ממנה הערות', polished: 'תזכורת לעצמי: לשלוח לנועה את המצגת עד מחר בבוקר, ולבקש ממנה הערות.' },
    mixed: { raw: 'צריך לעדכן את ה-README לפני ה-release של מחר', polished: 'צריך לעדכן את ה-README לפני ה-release של מחר.' },
  };
  const scriptFor = language => SCRIPT[language] ?? SCRIPT.en;
  state.jobs = {};
  state.live = {};
  let previewTimer = null;
  const update = { installed: '0.27.0', available: null, notes: null, stage: 'current', downloaded: 0, total: null, checked_at: data.now - 3_600_000, error: null };
  const nativeStatus = () => {
    const c = state.capture;
    if (!c) return { active: false, id: null, elapsed: 0, saved_bytes: 0, error: null, rms: 0, peak: 0, level_sequence: 0, listening: false };
    const elapsed = Date.now() - c.started;
    const t = elapsed / 1000;
    const rms = Math.max(0, 0.05 + 0.04 * Math.sin(t * 7.1) + 0.03 * Math.sin(t * 13.7) + (Math.random() * 0.03));
    return { active: true, id: c.id, elapsed, saved_bytes: Math.round(elapsed * 32), error: null, rms, peak: Math.min(1, rms * 2.2), level_sequence: ++c.seq, listening: true };
  };
  const handlers = {
    // window + shell
    window_state: () => ({ maximized: state.maximized }),
    window_command: ({ command }) => { if (command === 'toggle-maximize') state.maximized = !state.maximized; },
    request_quit_command: () => null,
    updates_status: () => update,
    updates_check: () => update,
    // capture + sessions
    capture_list: () => clone(state.recordings),
    capture_get: ({ id }) => clone(state.recordings.find(r => r.id === id) ?? null),
    capture_delete: ({ id }) => { state.recordings = state.recordings.filter(r => r.id !== id); },
    capture_start: ({ id, language }) => { state.capture = { id, started: Date.now(), language, seq: 0 }; return nativeStatus(); },
    capture_status: () => nativeStatus(),
    capture_stop: () => {
      const c = state.capture; const status = nativeStatus(); state.capture = null;
      if (c) state.recordings = [{ id: c.id, byte_length: status.saved_bytes, capture_state: 'complete', created_at_ms: c.started, duration_ms: status.elapsed, input_language: c.language, mime_type: 'audio/wav', chunk_count: 1 }, ...state.recordings];
      return { ...status, active: false };
    },
    capture_import: () => null,
    // Words arrive while "speaking", as a local model's live draft does.
    capture_preview_start: ({ id }) => {
      clearInterval(previewTimer);
      const words = scriptFor(state.capture?.language).raw.split(' ');
      let shown = 0;
      previewTimer = setInterval(() => {
        if (!state.capture || state.capture.id !== id) { clearInterval(previewTimer); return; }
        shown = Math.min(words.length, shown + 1 + (shown % 2));
        emit('writer-preview', { id, text: words.slice(0, shown).join(' ') });
      }, 420);
      return null;
    },
    capture_preview: () => '',
    saved_attempts: () => clone(state.attempts),
    session_event: () => null, session_waiting: () => null, session_refused: () => null, quit_ready: () => null,
    deliver_transcript: () => ({ copied: true, pasted: true, message: 'Pasted', app: 'Notes' }),
    prepare_delivery: () => undefined,
    history_resume: () => null,
    // Storage (Privacy & data): sizes like a long-used install; nothing is ever deleted here.
    storage_status: () => ({ data_bytes: 2.8 * 1024 ** 3 + 3.6 * 1024 ** 2, data_limit_bytes: 20 * 1024 ** 3, audio_bytes: 2.8 * 1024 ** 3, models_bytes: 2.2 * 1024 ** 3, state_bytes: 0.3 * 1024 ** 2 }),
    storage_preview: ({ beforeMs, history }) => {
      const kept = state.recordings.filter((recording) => beforeMs === null || recording.created_at_ms < beforeMs);
      return { id: 'preview', recordings: kept.length, jobs: history ? state.attempts.filter((attempt) => kept.some((recording) => recording.id === attempt.id)).length : 0, history, before_ms: beforeMs };
    },
    storage_clear: () => { throw 'This preview never deletes anything.'; },
    history_delete: () => ({ deleted_recordings: state.recordings.length, deleted_jobs: state.attempts.length }),
    // preferences
    preferences_get: () => clone(state.preferences),
    preferences_save: ({ preferences }) => { state.preferences = clone(preferences); },
    preferences_theme: () => null,
    shortcut_capture: () => null,
    system_settings_get: () => clone(state.system),
    system_settings_save: ({ settings }) => { state.system = clone(settings); },
    list_microphones: () => ({ devices: [
      { id: 'default', label: 'Default — Microphone (Shure MV7+)' },
      { id: 'mv7', label: 'Microphone (Shure MV7+)' },
      { id: 'realtek', label: 'Microphone Array (Realtek(R) Audio)' },
      { id: 'airpods', label: 'Headset (Alex’s AirPods Pro)' },
    ], selected_id: 'mv7', prefer_built_in: null }),
    select_microphone: () => null,
    microphone_prefer_built_in: () => null,
    accessibility_status: () => ({ trusted: true }),
    accessibility_open: () => null,
    // onboarding
    onboarding_get: () => ({ completed_version: window.__FAKE_ONBOARDING ? 0 : 99, current_version: 3 }),
    onboarding_save: ({ completed }) => ({ completed_version: completed ? 3 : 0, current_version: 3 }),
    // models + keys
    modes_save: ({ settings }) => { data.modes = clone({ ...settings, revision: settings.revision + 1 }); return clone(data.modes); },
    import_model: () => null,
    credential_prompt: () => 'saved',
    credential_remove: () => null,
    // data + about
    data_location: () => ({ root: 'C:\\Users\\Alex\\AppData\\Local\\TrickTic Dictate' }),
    open_data_folder: () => null,
    notices_index: () => [{ path: 'THIRD-PARTY-NOTICES.txt', name: 'Third-party notices', license: 'Various' }],
    notice_text: () => 'Example license text.',
    notices_folder: () => null,
    legal_open: () => null,
    // account + sync
    account_snapshot: () => ({ state: 'guest', availability: 'ready', generation: 1, reauthenticating: false }),
    account_tasks: () => ({ configured: true, unfinished: [] }),
    account_local_copy: () => ({ items: 0, unsynced: 0 }),
    portable_status: () => null,
    transcript_status: () => null,
    portable_theme: () => null,
  };
  async function invoke(command, args = {}) {
    const handler = handlers[command];
    if (!handler) { unknown.add(command); log('unknown invoke', command, args); return null; }
    await new Promise(r => setTimeout(r, 4));
    return handler(args);
  }
  async function listen(name, cb) {
    (listeners[name] ||= []).push(cb);
    return () => { listeners[name] = (listeners[name] || []).filter(x => x !== cb); };
  }
  window.__TAURI__ = { core: { invoke }, event: { listen } };
  window.__fake = { emit, state, handlers, listeners };

  // A quiet synthetic WAV (8 kHz, 8-bit mono) so the audio player has something real to load.
  function wav(ms) {
    const rate = 8000, n = Math.max(1, Math.round(rate * ms / 1000));
    const buf = new ArrayBuffer(44 + n), v = new DataView(buf);
    const str = (o, t) => [...t].forEach((c, i) => v.setUint8(o + i, c.charCodeAt(0)));
    str(0, 'RIFF'); v.setUint32(4, 36 + n, true); str(8, 'WAVE'); str(12, 'fmt '); v.setUint32(16, 16, true);
    v.setUint16(20, 1, true); v.setUint16(22, 1, true); v.setUint32(24, rate, true); v.setUint32(28, rate, true);
    v.setUint16(32, 1, true); v.setUint16(34, 8, true); str(36, 'data'); v.setUint32(40, n, true);
    for (let i = 0; i < n; i++) v.setUint8(44 + i, 128 + Math.round(6 * Math.sin(i / 9) * Math.sin(i / 2400)));
    return buf;
  }
  // ---------- fake HTTP API ----------
  const ok = (body, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
  const env = data => ok({ api_version: '1', data });
  const realFetch = window.fetch.bind(window);
  const routes = [
    ['GET', /^\/api\/v1\/health$/, () => ok({ status: 'ok' })],
    ['GET', /^\/api\/v1\/providers$/, () => ok({ providers: data.providers })],
    ['GET', /^\/api\/v1\/setup$/, () => env(data.setup)],
    ['PUT', /^\/api\/v1\/settings$/, (_m, body) => { data.setup.settings = body.data; return env(body.data); }],
    ['GET', /^\/api\/v1\/settings$/, () => env(data.setup.settings)],
    ['GET', /^\/api\/v1\/vocabulary$/, () => env(data.vocabulary)],
    ['PUT', /^\/api\/v1\/vocabulary$/, (_m, body) => { Object.assign(data.vocabulary, body.data); return env(data.vocabulary); }],
    ['GET', /^\/api\/v1\/modes$/, () => env(data.modes)],
    ['GET', /^\/api\/v1\/modes\/providers$/, () => env([
      { selection: { provider: 'openrouter', model: 'google/gemini-3.8-flash' }, configured: true, execution_location: 'cloud' },
      { selection: { provider: 'openrouter', model: 'anthropic/claude-haiku-4.5' }, configured: true, execution_location: 'cloud' },
      { selection: { provider: 'openai', model: 'gpt-5-mini' }, configured: true, execution_location: 'cloud' },
    ])],
    ['POST', /^\/api\/v1\/transcriptions$/, (_m, body) => {
      const request = body.data; const now = Date.now();
      const job = { id: crypto.randomUUID(), created_at_ms: now, updated_at_ms: now, request, status: { state: 'queued' } };
      const profile = data.modes.profiles.find(p => p.id === data.modes.active);
      state.jobs[job.id] = { job, started: now, profile };
      return env(clone(job));
    }],
    ['GET', /^\/api\/v1\/jobs\/([^/]+)$/, m => {
      const entry = state.jobs[decodeURIComponent(m[1])];
      if (!entry) return ok({ api_version: '1', error: { code: 'not_found', message: 'No such job.' } }, 404);
      const age = Date.now() - entry.started;
      const job = entry.job;
      const language = job.request.options.input_language;
      if (age > 1700 && job.status.state !== 'succeeded') {
        job.status = { state: 'succeeded', result: { cleanup: { state: 'off' }, transcript: { execution_location: 'self_hosted_server', input_language: language, output_language: { mode: 'preserve' }, raw_text: scriptFor(language).raw, selection: job.request.options.selection } } };
        job.updated_at_ms = Date.now();
        if (entry.profile && entry.profile.kind !== 'regular') state.live[job.id] = { readyAt: Date.now() + 1400, profile: entry.profile, language };
      } else if (age > 500 && job.status.state === 'queued') { job.status = { state: 'transcribing' }; job.updated_at_ms = Date.now(); }
      return env(clone(job));
    }],
    ['GET', /^\/api\/v1\/modes\/defaults$/, () => env(data.modes)],
    ['GET', /^\/api\/v1\/jobs\/([^/]+)\/processing$/, m => env(data.processing[decodeURIComponent(m[1])] ?? [])],
    ['GET', /^\/api\/v1\/jobs\/([^/]+)\/live-processing$/, m => {
      const live = state.live[decodeURIComponent(m[1])];
      if (live) {
        const done = Date.now() >= live.readyAt;
        const raw = scriptFor(live.language).raw;
        const attempt = {
          id: `proc-${m[1]}`, job_id: decodeURIComponent(m[1]), created_at_ms: live.readyAt - 1400, profile: live.profile,
          request: { operation_id: `op-${m[1]}`, profile_id: live.profile.id, profile_revision: live.profile.revision, context: { application: '', selected_text: '', clipboard: '' } },
          diagnostics: { processing_ms: 610, reused_draft: true, prepared_at_release: true, served_by: 'Groq', selected_model: live.profile.text_model, delivery: done ? { copied: true, pasted: true, at_ms: live.readyAt + 200, stop_to_delivery_ms: 1240 } : null },
          raw_text: raw, input_text: raw, prompt_version: 1,
          state: done ? { state: 'succeeded', text: scriptFor(live.language).polished } : { state: 'pending' },
        };
        return env({ bound: true, attempt, selected_attempt: attempt, delivery: done ? { attempt_id: attempt.id, copied: true, pasted: true, at_ms: live.readyAt + 200, stop_to_delivery_ms: 1240, original_used: false, fallback_used: false, app: 'Notes' } : null });
      }
      const attempt = data.processing[decodeURIComponent(m[1])]?.[0] ?? null;
      return env({ bound: !!attempt, attempt, selected_attempt: attempt, delivery: attempt ? { copied: true, pasted: true, at_ms: attempt.created_at_ms + 600, stop_to_delivery_ms: 1180, original_used: false, app: attempt.diagnostics?.delivery?.app ?? null } : null });
    }],
    ['GET', /^\/api\/v1\/jobs\/([^/]+)\/processed$/, m => {
      const attempt = data.attempts.find(a => a.job?.id === decodeURIComponent(m[1]));
      const raw = attempt?.job?.status?.result?.transcript?.raw_text ?? '';
      return env({ text: raw.replace(/\btail scale\b/g, 'Tailscale') });
    }],
    ['GET', /^\/native\/audio\/([^/]+)$/, m => {
      const rec = state.recordings.find(r => r.id === decodeURIComponent(m[1]));
      return new Response(wav(Math.min(rec?.duration_ms ?? 2000, 60000)), { status: 200, headers: { 'Content-Type': 'audio/wav' } });
    }],
    ['GET', /^\/api\/v1\/cleanup\/providers$/, () => env([])],
  ];
  window.fetch = async (input, init = {}) => {
    const url = new URL(typeof input === 'string' ? input : input.url, location.href);
    const method = (init.method || (typeof input !== 'string' && input.method) || 'GET').toUpperCase();
    if (url.pathname.startsWith('/api/') || url.pathname.startsWith('/native/')) {
      let body = null;
      try { body = init.body ? JSON.parse(init.body) : null; } catch { /* not json */ }
      for (const [verb, pattern, handler] of routes) {
        const match = url.pathname.match(pattern);
        if (match && verb === method) { await new Promise(r => setTimeout(r, 8)); return handler(match, body); }
      }
      unknown.add(`${method} ${url.pathname}`);
      log('unknown fetch', method, url.pathname);
      return ok({ error: { code: 'not_found' } }, 404);
    }
    return realFetch(input, init);
  };
})();
