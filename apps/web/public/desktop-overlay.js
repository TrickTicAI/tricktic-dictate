/* Shell presentation only; durable capture and transcription belong to the owner. */
/*
 * `?preview` is this same panel framed inside Settings, where it demonstrates
 * the settings being edited. No shell stands behind it: a stand-in with the
 * Tauri API's shape delivers what the Settings page posts, and every action is
 * inert.
 */
const preview = new URLSearchParams(location.search).has('preview');
const api = preview ? previewApi() : window.__TAURI__;
function previewApi() {
  const listeners = new Map();
  addEventListener('message', (event) => {
    const message = event.data;
    // Only the page hosting this frame, on this origin, can drive it.
    if (event.source !== window.parent || event.origin !== location.origin || message?.source !== 'writer-panel-preview') return;
    for (const listener of listeners.get(message.name) ?? []) listener({ payload: message.payload });
  });
  return {
    event: { listen: async (name, listener) => {
      if (!listeners.has(name)) listeners.set(name, new Set());
      listeners.get(name).add(listener);
      return () => listeners.get(name).delete(listener);
    } },
    core: { invoke: async () => {} },
  };
}
if (preview) {
  // A demonstration is not somewhere to tab into, and its scheme is fixed to
  // match the frame element holding it: Chromium paints an opaque backdrop
  // behind a frame whose scheme differs, which would square off the pill.
  document.querySelectorAll('button').forEach((button) => { button.tabIndex = -1; });
  document.documentElement.style.colorScheme = 'normal';
}
let preferences = { completion_hold_ms: 1200, auto_dismiss: true, reduced_motion: false, visibility: 'recording', scale: 1 };
const alwaysVisible = () => preferences.visibility === 'always';
// Install every listener before asking the shell to replay state. A reload
// must not lose its snapshot because another listener finished registering first.
const subscriptions = [];
const handlers = new Map();
function listen(name, handler) {
  handlers.set(name, handler);
  subscriptions.push(api.event.listen(name, handler));
}
let current, sequence = -1, lastLevel = 0, capturingSince = 0, dismiss;
/* The last few levels, newest first: the meter rolls outward from its middle. */
let history = [];
// A first launch has no retained snapshot to hand back on the ready handshake,
// so the panel starts from the state it is actually in rather than none.
document.body.dataset.state = 'idle';
const title = document.querySelector('#title');
const detail = document.querySelector('#detail');
const bars = Array.from({ length: 11 }, () => {
  const bar = document.createElement('i'); document.querySelector('#bars').append(bar); return bar;
});
const activity = document.querySelector('.activity');
const word = document.querySelector('#word');
const elapsed = document.querySelector('#elapsed');
const scale = () => Number(preferences.scale) > 0 ? Number(preferences.scale) : 1;
const titles = { idle: 'TrickTic Dictate', requesting: 'Opening microphone', capturing: 'Listening', saving: 'Saving', saved: 'Audio saved', queued: 'Getting ready', transcribing: 'Transcribing', success: 'Text ready', canceled: 'Discarded', error: 'Needs attention' };
const progressTitles = { waiting_for_capacity: 'Waiting to transcribe', retrying_1: 'Retrying 1 of 2', retrying_2: 'Retrying 2 of 2', processing_text: 'Processing text' };
const progressDetails = { waiting_for_capacity: 'Your recording is saved · retry resumes automatically', retrying_1: 'Retrying your saved recording automatically', retrying_2: 'Retrying your saved recording automatically', processing_text: 'Raw transcript is available in Writer' };
const details = { requesting: 'Starting recording…', capturing: 'Recording', saving: 'Finishing your recording…', saved: 'Ready to transcribe in TrickTic Dictate', queued: 'Your recording is saved', transcribing: 'Finishing your text…', success: 'Transcript saved in TrickTic Dictate', canceled: 'Recording discarded' };
const clearBars = () => { history = []; bars.forEach(bar => { bar.style.height = `${4 * scale()}px`; }); };
/*
 * Mirrored and rolling: the newest level is the middle bar and older ones move
 * outward, a little quieter each step. Heights are in pixels already scaled, so
 * the meter grows with the pill.
 */
function paintBars() {
  const middle = (bars.length - 1) / 2;
  bars.forEach((bar, index) => {
    const step = Math.abs(index - middle);
    const amplitude = history[step] ?? 0;
    const fade = 1 - step * 0.09;
    bar.style.height = `${(4 + amplitude * fade * 20) * scale()}px`;
  });
}

/*
 * Notices: a word or two the panel says that no dictation state covers -- a
 * shortcut press Writer could not act on, a paste it skipped -- and the hint
 * shown while insertion waits for the shortcut keys to be released. The shell
 * keeps the window on screen for as long as a timed notice lasts.
 */
const notice = Object.assign(document.createElement('span'), { id: 'notice' });
notice.setAttribute('aria-hidden', 'true');
document.querySelector('.activity').append(notice);
let noticeTimer = 0, noticeSession = null, noticeDetail = '';
function clearNotice() {
  clearTimeout(noticeTimer);
  delete document.body.dataset.notice;
  delete document.body.dataset.noticeTone;
  notice.textContent = ''; noticeSession = null; noticeDetail = '';
}
/** The tick a `done` notice ("Pasted") carries. */
function tick() {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
  path.setAttribute('d', 'M20 6 9 17l-5-5');
  svg.append(path);
  return svg;
}
/*
 * Earlier dictations whose text is still to come while this one owns the
 * panel. The pill counts them at its edge -- "+1" -- so an earlier dictation
 * does not simply vanish from sight; its text still lands, in order, as soon as
 * nothing holds it back.
 */
const waitingMark = Object.assign(document.createElement('span'), { id: 'waiting' });
waitingMark.setAttribute('role', 'status');
document.querySelector('section').append(waitingMark);

/*
 * Processing indicator. Three dots fill in turn for as long as the dictation is
 * being prepared, saved, queued or transcribed. It shows that work is still
 * happening; it is not a completion estimate, because nothing here knows how
 * long the model will take.
 *
 * Painted from here as an inline gradient rather than a CSS keyframe: Windows
 * animation effects are off on the target machine, WebView2 reports that as
 * `prefers-reduced-motion: reduce`, and the keyframe this replaces was being
 * silenced by it. Inline styles repaint regardless, which is why the level bars
 * beside these dots kept working.
 */
const dots = Array.from(document.querySelectorAll('#loading i'));
const processingStates = ['saving', 'queued', 'transcribing'];
const fillMs = 420;
const cycleMs = fillMs * (dots.length + 1); // A held beat with all three full before it repeats.
let processingSince = 0, processingTimer = 0;
function paintProcessing() {
  const position = ((Date.now() - processingSince) % cycleMs) / fillMs;
  dots.forEach((dot, index) => {
    const filled = Math.max(0, Math.min(1, position - index));
    // Reduced motion keeps the same meaning with whole steps instead of a sweep.
    const level = Math.round((preferences.reduced_motion ? Math.floor(filled) : filled) * 100);
    // MW-39 themed the panel's CSS; this dot fill is painted here, so it reads
    // the same `--text` token rather than staying white on a light theme.
    dot.style.backgroundImage = `linear-gradient(to top, var(--dot, var(--text, #f5f5f6)) ${level}%, transparent ${level}%)`;
  });
}
function scheduleProcessing() {
  clearInterval(processingTimer);
  processingTimer = setInterval(paintProcessing, preferences.reduced_motion ? 500 : 60);
}
function processing(active) {
  if (!active) {
    clearInterval(processingTimer);
    processingTimer = 0;
    dots.forEach(dot => { dot.style.backgroundImage = ''; });
    return;
  }
  if (processingTimer) return; // Already running; a new snapshot must not restart the sweep.
  processingSince = Date.now();
  paintProcessing();
  scheduleProcessing();
}
const action = value => api.core.invoke('overlay_action', { action: value, epoch: current?.epoch, revision: current?.revision }).catch(() => {
  detail.textContent = 'Open TrickTic Dictate from the tray to continue';
});
/*
 * Which controls the pill offers. Depends on both the session state and the
 * visibility preference -- an always-visible panel offers a record control
 * between dictations and has nothing to dismiss to -- so a preference change
 * re-runs it rather than waiting for the next session message.
 */
function renderControls() {
  const state = current?.state ?? 'idle';
  const at = value => document.querySelector(`[data-action=${value}]`);
  const between = ['idle', 'success', 'canceled'].includes(state);
  // Starting from the panel never activates this window, so the transcript
  // still lands in whatever had focus.
  at('start').hidden = !alwaysVisible() || !between;
  at('stop').hidden = !['requesting', 'capturing'].includes(state);
  at('cancel').hidden = !['requesting', 'capturing', 'saving', 'queued', 'transcribing'].includes(state);
  // A failure whose audio was kept offers Retry beside Open and Dismiss. A retry
  // transcribes the saved recording again and keeps it in history; it is never
  // pasted, because by then you are somewhere else.
  at('retry').hidden = !current?.audio_saved || !['saved', 'error', 'canceled'].includes(state);
  at('open').hidden = !['error', 'success'].includes(state);
  at('dismiss').hidden = alwaysVisible() || !['saved', 'error', 'success', 'canceled'].includes(state);
}
/*
 * Receipt stages for a dictation start as the panel saw it. The shell marks
 * when it forwards "Opening microphone" and "Listening"; this reports when each
 * reached this page, when a frame showing it had been produced -- the second
 * animation frame after the change -- and whether it came as the event itself
 * or from the shell's periodic resync. Wall-clock times, which the shell places
 * among its own stages. Once per dictation and state.
 */
let shownReported = [];
function reportShown(message, via) {
  const key = `${message.session_id}/${message.state}`;
  if (preview || !['requesting', 'capturing'].includes(message.state) || !message.session_id || shownReported.includes(key)) return;
  shownReported = [...shownReported.slice(-7), key];
  const received = Date.now();
  requestAnimationFrame(() => requestAnimationFrame(() => {
    const shown = { session_id: message.session_id, state: message.state, received_ms: received, painted_ms: Date.now(), via };
    void api.core.invoke('overlay_action', { action: 'shown', shown }).catch(() => {});
  }));
}
function scheduleDismiss() {
  clearTimeout(dismiss);
  if (['idle', 'success', 'canceled'].includes(current?.state)) return; // Native owner hides immediately.
  if (preferences.auto_dismiss && current?.state === 'saved') {
    dismiss = setTimeout(() => { void action('dismiss'); }, preferences.completion_hold_ms);
  }
}
listen('writer-session', ({ payload: message, resync }) => {
  if (message.kind === 'snapshot') {
    if (current && current.epoch === message.epoch && current.revision >= message.revision) return;
    const sameCapture = current?.state === 'capturing' && message.state === 'capturing' && current.session_id === message.session_id && current.epoch === message.epoch;
    current = message; sequence = -1;
    // A hint describes one moment of one dictation. A new dictation, or another
    // one taking the panel over, ends any notice.
    if (document.body.dataset.notice === 'hint' || ['requesting', 'capturing'].includes(message.state) || noticeSession && noticeSession !== message.session_id) clearNotice();
    if (!sameCapture) { lastLevel = 0; capturingSince = Date.now(); clearBars(); }
    const changed = document.body.dataset.state !== message.state;
    document.body.dataset.state = message.state;
    if (message.progress) document.body.dataset.progress = message.progress; else delete document.body.dataset.progress;
    if (message.state !== 'capturing') delete document.body.dataset.meter;
    word.replaceChildren(...(message.state === 'error'
      ? [Object.assign(document.createElement('i'), { className: 'err-dot' }), 'Failed']
      : message.state === 'saved' ? ['Saved'] : []));
    // A new state arrives with a small morph; the same state repeated does not.
    if (changed && !preferences.reduced_motion) { activity.classList.remove('morph'); void activity.offsetWidth; activity.classList.add('morph'); }
    processing(processingStates.includes(message.state));
    // A discard that could not remove the audio yet -- its transcription is
    // still running -- must not claim the recording is gone.
    const keptAfterCancel = message.state === 'canceled' && message.audio_saved;
    title.textContent = (keptAfterCancel ? 'Canceled' : progressTitles[message.progress] || titles[message.state]) || 'TrickTic Dictate';
    detail.textContent = message.state === 'error'
      ? `${message.error_code || 'Recording problem'} · ${message.audio_saved ? 'Audio saved' : 'Open TrickTic Dictate for details'}`
      : keptAfterCancel ? 'Saved audio is kept'
      : progressDetails[message.progress] || details[message.state] || '';
    detail.title = detail.textContent;
    if (document.body.dataset.notice) { detail.textContent = noticeDetail; detail.title = noticeDetail; }
    renderControls();
    scheduleDismiss();
    reportShown(message, resync ? 'sync' : 'event');
  } else if (message.kind === 'level' && current?.state === 'capturing' && message.epoch === current.epoch && message.session_id === current.session_id && message.revision === current.revision && message.sequence > sequence) {
    sequence = message.sequence; lastLevel = Date.now();
    delete document.body.dataset.meter;
    // A quiet microphone is still recording. Meter data is never a speech detector.
    const rms = Number.isFinite(message.rms) ? Math.max(0, Math.min(1, message.rms)) : 0;
    const amplitude = Math.min(1, Math.sqrt(rms) * 5);
    history = [amplitude, ...history].slice(0, Math.ceil(bars.length / 2));
    paintBars();
    detail.textContent = 'Recording';
  }
});
setInterval(() => {
  if (current?.state !== 'capturing') return;
  if (Date.now() - (lastLevel || capturingSince) > 1500) {
    clearBars(); detail.textContent = 'Recording · meter unavailable';
    document.body.dataset.meter = 'off';
  }
  // How long you have been talking, for the hover.
  const seconds = Math.max(0, Math.floor((Date.now() - capturingSince) / 1000));
  elapsed.textContent = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
}, 300);
document.querySelectorAll('button').forEach(button => button.addEventListener('click', () => { void action(button.dataset.action); }));
const themeTokens = ['bg', 'border', 'text', 'text_muted', 'accent', 'accent_text', 'surface', 'danger'];
/*
 * Colours come from two messages. `writer-theme` carries the app theme's
 * resolved values; the panel colour preference, when set, replaces the fill and
 * the voice animation, and the tones drawn from the animation colour. Either
 * message repaints from both, so a theme change under custom colours, and a
 * return to "Match theme", each land without waiting for the other.
 */
let theme = {};
const literalHex = /^#[0-9a-f]{6}$/i;
function luminance(hex) {
  const channel = (at) => {
    const value = parseInt(hex.slice(at, at + 2), 16) / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
}
function paint() {
  const tokens = { ...theme };
  let kind = theme.kind;
  const own = preferences.panel_colors;
  // The shell refuses anything else; checked again because it lands in CSS.
  if (own && literalHex.test(own.background) && literalHex.test(own.wave)) {
    Object.assign(tokens, {
      bg: own.background, text: own.wave, accent: own.wave,
      border: `${own.wave}33`, surface: `${own.wave}29`, text_muted: `${own.wave}b3`,
    });
    kind = luminance(own.background) > 0.179 ? 'light' : 'dark';
  }
  const style = document.documentElement.style;
  for (const token of themeTokens) {
    const property = `--${token.replace('_', '-')}`;
    if (typeof tokens[token] === 'string') style.setProperty(property, tokens[token]);
    else style.removeProperty(property);
  }
  if (!preview && kind) style.colorScheme = kind === 'dark' ? 'dark' : 'light';
}
listen('writer-theme', ({ payload }) => { theme = payload ?? {}; paint(); });
listen('writer-preferences', ({ payload: value }) => {
  preferences = value;
  document.body.dataset.reducedMotion = String(value.reduced_motion);
  document.body.dataset.visibility = value.visibility ?? 'recording';
  document.documentElement.style.setProperty('--scale', String(value.scale ?? 1));
  if (current?.state === 'capturing') paintBars();
  paint();
  scheduleDismiss();
  if (processingTimer) { paintProcessing(); scheduleProcessing(); }
  renderControls();
});
document.querySelector('.activity').addEventListener('pointerdown', event => { if (event.button === 0) void action('drag'); });
listen('writer-model', ({ payload: model }) => {
  if (current?.state !== 'queued') return;
  if (model.weights?.state === 'downloading') {
    title.textContent = 'Preparing model'; detail.textContent = `${Math.floor(model.weights.downloaded_bytes / 1e6)} MB downloaded · audio saved`;
  } else if (model.weights?.state === 'verifying' || model.worker?.state === 'starting') {
    title.textContent = 'Preparing model'; detail.textContent = 'Loading model · audio saved';
  } else { title.textContent = titles.queued; detail.textContent = details.queued; }
});

listen('writer-notice', ({ payload: message }) => {
  // Meant for a dictation this panel is no longer showing.
  if (message.session_id && message.session_id !== current?.session_id) return;
  clearTimeout(noticeTimer);
  noticeSession = message.session_id ?? null;
  noticeDetail = message.detail;
  notice.replaceChildren(...(message.tone === 'done' ? [tick()] : []), message.text);
  if (message.tone === 'done') document.body.dataset.noticeTone = 'done'; else delete document.body.dataset.noticeTone;
  detail.textContent = message.detail; detail.title = message.detail;
  document.body.dataset.notice = message.ms ? 'timed' : 'hint';
  if (message.ms) noticeTimer = setTimeout(clearNotice, message.ms);
});
listen('writer-waiting', ({ payload }) => {
  const count = Math.max(0, Math.floor(Number(payload?.count) || 0));
  if (!count) { delete document.body.dataset.waiting; waitingMark.replaceChildren(); return; }
  document.body.dataset.waiting = String(count);
  const shown = Object.assign(document.createElement('span'), { textContent: String(count) });
  shown.setAttribute('aria-hidden', 'true');
  const words = Object.assign(document.createElement('span'), { className: 'unseen',
    textContent: count === 1 ? '1 earlier dictation still to be inserted' : `${count} earlier dictations still to be inserted` });
  waitingMark.replaceChildren(shown, words);
});

// Writer draws its own menus. The WebView's own list — Back, Reload, Save
// as — is about the browser rather than about anything in this window, and
// no theme can repaint it, so it never appears.
addEventListener('contextmenu', (event) => event.preventDefault());

// A native challenge exercises the event path and repairs a missed snapshot.
// There is no page timer: hidden WebViews may throttle timers for minutes.
listen('writer-panel-sync', ({ payload }) => {
  for (const [name, value] of [
    ['writer-preferences', payload.preferences], ['writer-theme', payload.theme],
    ['writer-session', payload.snapshot], ['writer-waiting', { count: payload.waiting }],
  ]) {
    if (name === 'writer-preferences' && JSON.stringify(value) === JSON.stringify(preferences)) continue;
    if (name === 'writer-theme' && JSON.stringify(value) === JSON.stringify(theme)) continue;
    if (value != null) handlers.get(name)?.({ payload: value, resync: true });
  }
  // Event delivery alone can stay alive while a hidden browser surface stops
  // producing frames. Answer only after rendering has had a chance to advance.
  requestAnimationFrame(() => requestAnimationFrame(() => {
    void api.core.invoke('overlay_action', { action: 'alive', nonce: payload.nonce }).catch(() => {});
  }));
});
void Promise.all(subscriptions).then(() => action('ready'));
