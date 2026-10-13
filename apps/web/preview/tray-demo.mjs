// The rows the shell would send the tray menu, over made-up data, in the same
// shape `tray.rs` publishes them (see `rows()` there). Used by the preview
// server for /desktop-tray.html and by shoot-panel-tray.mjs.

export const TRAY_STATES = ['ready', 'recording', 'working', 'failed', 'starting'];

export const TRAY_THEMES = {
  paper: { kind: 'light', bg: '#FDFBF7', border: '#00000030', text: '#1F1B16', text_muted: '#6B6155', accent: '#B4541E', accent_text: '#FFFDF9', surface: '#00000010', danger: '#B3261E' },
  dracula: { kind: 'dark', bg: '#343746', border: '#ffffff30', text: '#F8F8F2', text_muted: '#BFBFBF', accent: '#FF79C6', accent_text: '#282A36', surface: '#ffffff12', danger: '#FF5555' },
};

const action = (id, label, icon, extra = {}) => ({ kind: 'action', id, label, detail: '', keys: false, icon, enabled: true, ...extra });
const check = (id, label, checked, extra = {}) => ({ kind: 'check', id, label, detail: '', keys: false, icon: '', enabled: true, checked, ...extra });
const toggle = (id, label, checked) => ({ kind: 'switch', id, label, detail: '', keys: false, icon: '', enabled: true, checked });
const divider = { kind: 'divider' };
const heading = (label, at_ms = null) => ({ kind: 'heading', label, at_ms });
const note = (label) => ({ kind: 'note', label, detail: '' });
const submenu = (label, icon, detail, rows) => ({ kind: 'submenu', label, detail, icon, enabled: true, rows });

const STATUS = {
  ready: ['Ready to dictate', 'Whisper large-v3-turbo · English · Super', 'ready'],
  recording: ['Recording', 'Shure MV7+ · Super', 'recording'],
  working: ['Transcribing', 'Whisper large-v3-turbo · English · Super', 'working'],
  failed: ['The last dictation failed', 'Your audio is saved. Open to retry.', 'failed'],
  starting: ['Getting ready', 'TrickTic Dictate is starting', 'starting'],
};

export function trayRows(state = 'ready', { platform = 'windows', now = Date.now() } = {}) {
  const recording = state === 'recording';
  const [title, detail, tone] = STATUS[state] ?? STATUS.ready;
  const record = platform === 'macos' ? 'Ctrl+Shift+Space' : 'Ctrl+Shift+F9';
  const discard = platform === 'macos' ? 'Ctrl+Shift+F10' : 'Ctrl+Shift+F10';
  const microphones = [
    check('mic:', 'System default', false, { enabled: !recording }),
    divider,
    check('mic:shure', 'Shure MV7+', true, { enabled: !recording }),
    check('mic:array', 'Microphone Array (Realtek)', false, { enabled: !recording }),
    check('mic:airpods', 'AirPods Pro', false, { enabled: !recording }),
    ...(recording ? [divider, note('Stop recording to change the microphone')] : []),
    divider,
    action('settings:microphone', 'Microphone settings…', 'settings'),
  ];
  const languages = [
    check('lang:en', 'English', true),
    check('lang:he', 'Hebrew', false),
    check('lang:mixed', 'Hebrew and English', false),
    divider,
    note('Whisper large-v3-turbo transcribes English, Hebrew and mixed speech.'),
  ];
  const models = [
    heading('On this computer'),
    check('model:local:whisper-large-v3-turbo-q5-0', 'Whisper large-v3-turbo', true),
    check('model:local:ivrit-turbo', 'ivrit.ai Whisper large-v3-turbo', false, { detail: 'Hebrew only' }),
    check('model:local:whisper-small', 'Whisper small', false, { enabled: false, detail: 'Downloading' }),
    heading('Cloud'),
    check('model:openai:gpt-4o-transcribe', 'OpenAI · gpt-4o-transcribe', false),
    check('model:gemini:gemini-2.5-flash', 'Gemini · gemini-2.5-flash', false, { enabled: false, detail: 'Needs a key' }),
    divider,
    submenu('API keys', 'key', '', [action('key-set:openai', 'Set OpenAI key…', ''), action('key-set:gemini', 'Set Gemini key…', ''), action('key-clear:openai', 'Remove OpenAI key', '')]),
    action('settings:engine', 'Speech engine settings…', 'settings'),
  ];
  const modes = [
    check('mode:regular', 'Regular', false),
    check('mode:super', 'Super', true, { detail: 'Ctrl+Alt+S', keys: true }),
    check('mode:command', 'Command', false, { enabled: false, detail: 'Needs a text model' }),
    divider,
    action('settings:modes', 'Mode settings…', 'settings'),
  ];
  const preferences = [
    toggle('toggle:overlay', 'Floating panel', true),
    toggle('toggle:sounds', 'Start and stop sounds', true),
    ...(platform === 'macos' ? [] : [toggle('toggle:duck', 'Quiet other audio while dictating', false)]),
    toggle('toggle:startup', platform === 'macos' ? 'Launch at login' : 'Start with Windows', true),
    divider,
    action('restore-panel', 'Restore floating panel', 'window'),
  ];
  const settings = [
    ['engine', 'Speech engine', 'engine'], ['shortcuts', 'Shortcuts & pasting', 'keyboard'], ['microphone', 'Microphone & sound', 'mic'],
    ['panel', 'Floating panel', 'panel'], ['modes', 'Modes', 'wand'], ['text', 'Text models', 'sparkles'], ['keys', 'API keys', 'key'],
    ['general', 'General', 'general'], ['appearance', 'Appearance', 'palette'], ['account', 'Account & sync', 'user'],
    ['data', 'Privacy & data', 'shield'], ['about', 'About & updates', 'info'],
  ].map(([id, label, icon]) => action(`settings:${id}`, label, icon));
  settings.push(divider, action('data-folder', 'Open data folder', 'folder'));

  return [
    { kind: 'status', id: 'open', label: title, detail, tone, since_ms: recording ? now - 7_000 : null, enabled: true },
    action('dictate', recording ? 'Stop and paste' : 'Start dictation', recording ? 'stop' : 'mic', { detail: record, keys: true, enabled: state !== 'starting' }),
    ...(recording || state === 'working' ? [action('cancel', 'Discard dictation', 'trash', { detail: discard, keys: true })] : []),
    divider,
    heading('Last dictation', now - 4 * 60_000),
    note('“Hey Maya, thanks for sending the deck. I went through the pricing…”'),
    action('copy-pasted', 'Copy it again', 'copy'),
    action('copy-raw', 'Copy as spoken', 'quote'),
    action('show-last', 'Open in TrickTic Dictate', 'text'),
    divider,
    submenu('Microphone', 'mic', recording ? 'In use' : 'Shure MV7+', microphones),
    submenu('Language', 'globe', 'English', languages),
    submenu('Speech model', 'engine', 'Whisper large-v3-turbo', models),
    submenu('Mode', 'wand', 'Super', modes),
    divider,
    submenu('Preferences', 'preferences', '', preferences),
    submenu('Settings', 'settings', '', settings),
    divider,
    action('quit', 'Quit TrickTic Dictate', 'power'),
  ];
}

export function trayPayload(state, theme = TRAY_THEMES.paper, options = {}) {
  return {
    rows: trayRows(state, options),
    scale: 1,
    max_width: 1920,
    max_height: 1040,
    theme,
    direction: 'ltr',
    platform: options.platform ?? 'windows',
    reduced_motion: false,
  };
}
