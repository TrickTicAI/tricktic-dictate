/*
 * The tray menu, drawn by Writer.
 *
 * The shell decides what the menu contains; this file only draws it and reports
 * back which row was activated. Every row carries the id the shell issued, and
 * nothing here invents one — an id this page made up would be refused by
 * `command()` on the other side anyway, which is the point of keeping the plan
 * and the drawing apart.
 *
 * Submenus drill down in place instead of flying out sideways. A flyout has to
 * fit inside a window whose size is fixed before it is known, and the version
 * that guessed at the widest submenu left an invisible dead zone next to the
 * menu that swallowed clicks meant for whatever was underneath.
 */

// Outside the shell there is nothing to talk to. The page still renders, so it
// can be opened and inspected, but every call is a no-op rather than a throw.
const api = window.__TAURI__ ?? { core: { invoke: async () => null }, event: { listen: async () => () => {} } };
const panel = document.querySelector('#panel');
const list = document.querySelector('#rows');
const backBar = document.querySelector('#back');
const backButton = backBar.querySelector('button');

/** The window's own icon set (lucide 0.441), drawn at 16px. Named by the shell. */
const ICONS = {
  mic: [["path",{"d":"M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z"}],["path",{"d":"M19 10v2a7 7 0 0 1-14 0v-2"}],["line",{"x1":"12","x2":"12","y1":"19","y2":"22"}]],
  stop: [["rect",{"width":"18","height":"18","x":"3","y":"3","rx":"2"}]],
  trash: [["path",{"d":"M3 6h18"}],["path",{"d":"M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"}],["path",{"d":"M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"}],["line",{"x1":"10","x2":"10","y1":"11","y2":"17"}],["line",{"x1":"14","x2":"14","y1":"11","y2":"17"}]],
  copy: [["rect",{"width":"14","height":"14","x":"8","y":"8","rx":"2","ry":"2"}],["path",{"d":"M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"}]],
  quote: [["path",{"d":"M17 6H3"}],["path",{"d":"M21 12H8"}],["path",{"d":"M21 18H8"}],["path",{"d":"M3 12v6"}]],
  text: [["path",{"d":"M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"}],["path",{"d":"M14 2v4a2 2 0 0 0 2 2h4"}],["path",{"d":"M10 9H8"}],["path",{"d":"M16 13H8"}],["path",{"d":"M16 17H8"}]],
  globe: [["circle",{"cx":"12","cy":"12","r":"10"}],["path",{"d":"M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"}],["path",{"d":"M2 12h20"}]],
  wand: [["path",{"d":"m21.64 3.64-1.28-1.28a1.21 1.21 0 0 0-1.72 0L2.36 18.64a1.21 1.21 0 0 0 0 1.72l1.28 1.28a1.2 1.2 0 0 0 1.72 0L21.64 5.36a1.2 1.2 0 0 0 0-1.72"}],["path",{"d":"m14 7 3 3"}],["path",{"d":"M5 6v4"}],["path",{"d":"M19 14v4"}],["path",{"d":"M10 2v2"}],["path",{"d":"M7 8H3"}],["path",{"d":"M21 16h-4"}],["path",{"d":"M11 3H9"}]],
  sparkles: [["path",{"d":"M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z"}],["path",{"d":"M20 3v4"}],["path",{"d":"M22 5h-4"}],["path",{"d":"M4 17v2"}],["path",{"d":"M5 18H3"}]],
  preferences: [["line",{"x1":"21","x2":"14","y1":"4","y2":"4"}],["line",{"x1":"10","x2":"3","y1":"4","y2":"4"}],["line",{"x1":"21","x2":"12","y1":"12","y2":"12"}],["line",{"x1":"8","x2":"3","y1":"12","y2":"12"}],["line",{"x1":"21","x2":"16","y1":"20","y2":"20"}],["line",{"x1":"12","x2":"3","y1":"20","y2":"20"}],["line",{"x1":"14","x2":"14","y1":"2","y2":"6"}],["line",{"x1":"8","x2":"8","y1":"10","y2":"14"}],["line",{"x1":"16","x2":"16","y1":"18","y2":"22"}]],
  general: [["line",{"x1":"4","x2":"4","y1":"21","y2":"14"}],["line",{"x1":"4","x2":"4","y1":"10","y2":"3"}],["line",{"x1":"12","x2":"12","y1":"21","y2":"12"}],["line",{"x1":"12","x2":"12","y1":"8","y2":"3"}],["line",{"x1":"20","x2":"20","y1":"21","y2":"16"}],["line",{"x1":"20","x2":"20","y1":"12","y2":"3"}],["line",{"x1":"2","x2":"6","y1":"14","y2":"14"}],["line",{"x1":"10","x2":"14","y1":"8","y2":"8"}],["line",{"x1":"18","x2":"22","y1":"16","y2":"16"}]],
  settings: [["path",{"d":"M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"}],["circle",{"cx":"12","cy":"12","r":"3"}]],
  power: [["path",{"d":"M12 2v10"}],["path",{"d":"M18.4 6.6a9 9 0 1 1-12.77.04"}]],
  check: [["path",{"d":"M20 6 9 17l-5-5"}]],
  chevron: [["path",{"d":"m9 18 6-6-6-6"}]],
  back: [["path",{"d":"m15 18-6-6 6-6"}]],
  key: [["path",{"d":"M2.586 17.414A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172a2 2 0 0 0 1.414-.586l.814-.814a6.5 6.5 0 1 0-4-4z"}],["circle",{"cx":"16.5","cy":"7.5","r":".5","fill":"currentColor"}]],
  folder: [["path",{"d":"m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"}]],
  window: [["rect",{"x":"2","y":"4","width":"20","height":"16","rx":"2"}],["path",{"d":"M10 4v4"}],["path",{"d":"M2 8h20"}],["path",{"d":"M6 4v4"}]],
  engine: [["path",{"d":"M2 10v3"}],["path",{"d":"M6 6v11"}],["path",{"d":"M10 3v18"}],["path",{"d":"M14 8v7"}],["path",{"d":"M18 5v13"}],["path",{"d":"M22 10v3"}]],
  keyboard: [["path",{"d":"M10 8h.01"}],["path",{"d":"M12 12h.01"}],["path",{"d":"M14 8h.01"}],["path",{"d":"M16 12h.01"}],["path",{"d":"M18 8h.01"}],["path",{"d":"M6 8h.01"}],["path",{"d":"M7 16h10"}],["path",{"d":"M8 12h.01"}],["rect",{"width":"20","height":"16","x":"2","y":"4","rx":"2"}]],
  panel: [["path",{"d":"M21 9V6a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2v10c0 1.1.9 2 2 2h4"}],["rect",{"width":"10","height":"7","x":"12","y":"13","rx":"2"}]],
  palette: [["circle",{"cx":"13.5","cy":"6.5","r":".5","fill":"currentColor"}],["circle",{"cx":"17.5","cy":"10.5","r":".5","fill":"currentColor"}],["circle",{"cx":"8.5","cy":"7.5","r":".5","fill":"currentColor"}],["circle",{"cx":"6.5","cy":"12.5","r":".5","fill":"currentColor"}],["path",{"d":"M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z"}]],
  user: [["circle",{"cx":"12","cy":"8","r":"5"}],["path",{"d":"M20 21a8 8 0 0 0-16 0"}]],
  shield: [["path",{"d":"M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"}],["path",{"d":"m9 12 2 2 4-4"}]],
  info: [["circle",{"cx":"12","cy":"12","r":"10"}],["path",{"d":"M12 16v-4"}],["path",{"d":"M12 8h.01"}]],
  alert: [["circle",{"cx":"12","cy":"12","r":"10"}],["line",{"x1":"12","x2":"12","y1":"8","y2":"12"}],["line",{"x1":"12","x2":"12.01","y1":"16","y2":"16"}]],
  loader: [["path",{"d":"M21 12a9 9 0 1 1-6.219-8.56"}]],
  hourglass: [["path",{"d":"M5 22h14"}],["path",{"d":"M5 2h14"}],["path",{"d":"M17 22v-4.172a2 2 0 0 0-.586-1.414L12 12l-4.414 4.414A2 2 0 0 0 7 17.828V22"}],["path",{"d":"M7 2v4.172a2 2 0 0 0 .586 1.414L12 12l4.414-4.414A2 2 0 0 0 17 6.172V2"}]],
};
/** The application's seven-bar waveform, filled rather than stroked. */
const MARK = [[0, 6.6, 6.8], [4, 3.7, 12.6], [8, 1, 18], [12, 4.4, 11.2], [16, 2.3, 15.5], [20, 5.5, 9], [24, 7.3, 5.4]];

const SVG = 'http://www.w3.org/2000/svg';
function icon(name, className, size = 16) {
  const nodes = ICONS[name];
  if (!nodes) return null;
  const svg = document.createElementNS(SVG, 'svg');
  for (const [key, value] of Object.entries({ viewBox: '0 0 24 24', width: size, height: size, fill: 'none', stroke: 'currentColor', 'stroke-width': 2, 'stroke-linecap': 'round', 'stroke-linejoin': 'round', 'aria-hidden': 'true' })) {
    svg.setAttribute(key, String(value));
  }
  if (className) svg.setAttribute('class', className);
  for (const [tag, attributes] of nodes) {
    const shape = document.createElementNS(SVG, tag);
    for (const [key, value] of Object.entries(attributes)) shape.setAttribute(key, value);
    svg.append(shape);
  }
  return svg;
}
function mark(size) {
  const svg = document.createElementNS(SVG, 'svg');
  svg.setAttribute('viewBox', '0 0 26.5 20');
  svg.setAttribute('width', String(size));
  svg.setAttribute('height', String(Math.round(size * 20 / 26.5)));
  svg.setAttribute('aria-hidden', 'true');
  for (const [x, y, height] of MARK) {
    const bar = document.createElementNS(SVG, 'rect');
    for (const [key, value] of Object.entries({ x, y, width: 2.5, height, rx: 1.25, fill: 'currentColor' })) bar.setAttribute(key, String(value));
    svg.append(bar);
  }
  return svg;
}

/** The stack of open panels: the root menu, then whatever was drilled into. */
let stack = [];
let bounds = { width: Infinity, height: Infinity };
let platform = 'windows';
let focused = -1;
let ticking = 0;

const dismiss = () => { void api.core.invoke('tray_menu_dismiss').catch(() => {}); };

function paint(theme, direction) {
  document.documentElement.dir = direction === 'rtl' ? 'rtl' : 'ltr';
  if (!theme) return;
  for (const token of ['bg', 'border', 'text', 'text_muted', 'accent', 'accent_text', 'surface', 'danger']) {
    if (typeof theme[token] === 'string') {
      document.documentElement.style.setProperty(`--${token.replace('_', '-')}`, theme[token]);
    }
  }
  document.documentElement.style.colorScheme = theme.kind === 'dark' ? 'dark' : 'light';
}

function show(payload) {
  if (!payload || !Array.isArray(payload.rows)) return;
  paint(payload.theme, payload.direction);
  platform = payload.platform === 'macos' ? 'macos' : 'windows';
  document.documentElement.dataset.motion = payload.reduced_motion ? 'reduced' : 'full';
  bounds = {
    width: Number(payload.max_width) || Infinity,
    height: Number(payload.max_height) || Infinity,
  };
  stack = [{ title: null, rows: payload.rows }];
  render();
}

function render() {
  const level = stack[stack.length - 1];
  const margin = 14;
  panel.style.maxWidth = `${Math.min(360, Math.max(220, bounds.width - 2 * margin))}px`;
  // The row list, not the panel, is what scrolls: the submenu header stays put.
  list.style.maxHeight = `${Math.max(120, bounds.height - 2 * margin - (level.title ? 44 : 0) - 14)}px`;

  backBar.hidden = !level.title;
  if (level.title) {
    backButton.replaceChildren();
    const glyph = document.createElement('span');
    glyph.className = 'glyph';
    const arrow = icon('back', 'chevron');
    if (arrow) glyph.append(arrow);
    backButton.append(glyph, document.createTextNode(level.title));
  }

  clearInterval(ticking);
  list.replaceChildren();
  focused = -1;
  level.rows.forEach((row) => list.append(build(row)));
  panel.hidden = false;
  measure();
}

/** "Ctrl+Shift+F9" as keycaps, in this platform's notation (⌃ ⇧ on a Mac). */
const MAC_KEYS = { WIN: '⌘', SUPER: '⌘', META: '⌘', CMD: '⌘', COMMAND: '⌘', CTRL: '⌃', CONTROL: '⌃', ALT: '⌥', OPTION: '⌥', OPT: '⌥', SHIFT: '⇧', FN: 'fn', GLOBE: 'fn' };
function keys(chord) {
  const caps = document.createElement('span');
  caps.className = 'keys';
  for (const part of chord.split('+').map((token) => token.trim()).filter(Boolean)) {
    const key = document.createElement('kbd');
    key.textContent = platform === 'macos' ? MAC_KEYS[part.toUpperCase()] ?? part : part;
    caps.append(key);
  }
  caps.setAttribute('aria-label', chord);
  return caps;
}

/** When the last dictation happened, the way Home's history says it. */
function when(ms) {
  const date = new Date(ms);
  if (!Number.isFinite(date.getTime())) return '';
  const start = (value) => new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime();
  const days = Math.round((start(new Date()) - start(date)) / 86_400_000);
  const clock = date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  if (days === 0) return clock;
  if (days === 1) return `Yesterday, ${clock}`;
  return date.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
}

const elapsed = (since) => {
  const seconds = Math.max(0, Math.floor((Date.now() - since) / 1000));
  const clock = `${Math.floor(seconds / 60) % 60}:${String(seconds % 60).padStart(2, '0')}`;
  return seconds >= 3600 ? `${Math.floor(seconds / 3600)}:${clock.padStart(5, '0')}` : clock;
};

const BADGES = { recording: 'mic', working: 'loader', failed: 'alert', starting: 'hourglass' };

function status(row) {
  const item = document.createElement('button');
  item.type = 'button';
  item.className = 'row status';
  item.dataset.kind = 'status';
  item.dataset.tone = row.tone || 'ready';
  if (row.id) item.dataset.id = row.id;
  item.setAttribute('role', 'menuitem');
  item.title = 'Open TrickTic Dictate';

  const badge = document.createElement('span');
  badge.className = 'badge';
  badge.append(BADGES[row.tone] ? icon(BADGES[row.tone], row.tone === 'working' ? 'spin' : '', 18) : mark(20));

  const text = document.createElement('span');
  text.className = 'status-text';
  const title = document.createElement('strong');
  title.textContent = row.label;
  const detail = document.createElement('small');
  // A model, a language and a mode name: any of them can be Hebrew.
  const value = document.createElement('bdi');
  value.textContent = row.detail || '';
  detail.append(value);
  text.append(title, detail);
  item.append(badge, text);

  // The status row counts the recording up while the menu is open; the shell
  // sends when it started rather than re-sending the menu every second.
  const since = Number(row.since_ms);
  if (row.tone === 'recording' && Number.isFinite(since) && since > 0) {
    const tick = () => { title.textContent = `${row.label} · ${elapsed(since)}`; };
    tick();
    ticking = setInterval(tick, 1000);
  }
  wire(item, row);
  return item;
}

function build(row) {
  if (row.kind === 'divider') {
    const line = document.createElement('div');
    line.className = 'separator';
    line.setAttribute('role', 'separator');
    return line;
  }
  if (row.kind === 'heading') {
    const heading = document.createElement('p');
    heading.className = 'heading';
    const at = Number(row.at_ms);
    heading.textContent = Number.isFinite(at) && at > 0 && when(at) ? `${row.label} · ${when(at)}` : row.label;
    return heading;
  }
  if (row.kind === 'note') {
    const note = document.createElement('p');
    note.className = 'note';
    // A transcript preview is the user's own speech and carries its own
    // direction. `bdi` orders it correctly without turning the whole row
    // around: a Hebrew quotation still starts where every other label does.
    const text = document.createElement('bdi');
    text.textContent = row.label;
    note.append(text);
    // A note is not a menu item. Assistive technology should read it as the
    // text it is rather than announce a control that cannot be activated.
    return note;
  }
  if (row.kind === 'status') return status(row);

  const item = document.createElement('button');
  item.type = 'button';
  item.className = 'row';
  item.dataset.kind = row.kind;
  if (row.id) item.dataset.id = row.id;
  if (row.id === 'quit' || row.id === 'cancel') item.classList.add('danger');
  if (row.id === 'dictate') item.classList.add('primary');
  const enabled = row.enabled !== false;
  item.disabled = !enabled;
  const toggles = row.kind === 'check' || row.kind === 'switch';
  item.setAttribute('role', toggles ? 'menuitemcheckbox' : 'menuitem');
  if (!enabled) item.setAttribute('aria-disabled', 'true');
  if (toggles) item.setAttribute('aria-checked', String(!!row.checked));
  if (row.kind === 'submenu') item.setAttribute('aria-haspopup', 'true');

  const glyph = document.createElement('span');
  glyph.className = 'glyph';
  const symbol = row.kind === 'check'
    ? (row.checked ? icon('check') : null)
    : icon(row.icon);
  if (symbol) glyph.append(symbol);

  const label = document.createElement('span');
  label.className = 'label';
  label.textContent = row.label;

  const trailing = document.createElement('span');
  trailing.className = 'detail';
  if (row.kind === 'switch') {
    const track = document.createElement('span');
    track.className = 'switch';
    track.setAttribute('aria-hidden', 'true');
    trailing.append(track);
  } else if (row.keys && row.detail) {
    trailing.append(keys(row.detail));
  } else if (row.detail) {
    // A current value or a reason; a microphone or mode can be named in Hebrew.
    const value = document.createElement('bdi');
    value.className = 'value';
    value.textContent = row.detail;
    trailing.append(value);
  }
  if (row.kind === 'submenu') {
    const chevron = icon('chevron', 'chevron', 14);
    if (chevron) trailing.append(chevron);
  }

  item.append(glyph, label, trailing);
  if (row.kind === 'submenu') item.__rows = row.rows || [];
  wire(item, row);
  return item;
}

function wire(item, row) {
  item.addEventListener('click', () => activate(item, row));
  item.addEventListener('mousemove', () => {
    if (item.disabled) return;
    const index = items().indexOf(item);
    if (index !== focused) { focused = index; highlight(); }
  });
}

function activate(element, row) {
  if (element.disabled) return;
  if (row.kind === 'submenu') {
    stack.push({ title: row.label, rows: element.__rows });
    render();
    return;
  }
  if (!row.id) return;
  void api.core.invoke('tray_menu_command', { id: row.id }).catch(() => dismiss());
}

const items = () => [...list.querySelectorAll('.row:not([disabled])')];

function highlight() {
  const all = items();
  all.forEach((item, index) => item.classList.toggle('active', index === focused));
  if (focused >= 0) all[focused].scrollIntoView({ block: 'nearest' });
}

function move(step) {
  const all = items();
  if (!all.length) return;
  focused = focused < 0
    ? (step > 0 ? 0 : all.length - 1)
    : (focused + step + all.length) % all.length;
  highlight();
}

function back() {
  if (stack.length < 2) { dismiss(); return; }
  stack.pop();
  render();
}

/**
 * Reports the size the rows actually need, in CSS pixels. The shell sizes the
 * window to it and only then shows it, so the first paint happens off screen
 * rather than as a flash at whatever size the previous menu left behind.
 */
function measure() {
  // Read synchronously rather than waiting for a frame. The window is still
  // hidden at this point, and a hidden webview is not painting, so a
  // `requestAnimationFrame` callback may never arrive — and the menu would
  // never be shown. Reading `offsetWidth` forces layout on its own.
  const margin = 14;
  const width = Math.ceil(panel.offsetWidth + 2 * margin);
  const height = Math.ceil(panel.offsetHeight + 2 * margin);
  void api.core.invoke('tray_menu_measured', { width, height }).catch(() => {});
}

backButton.addEventListener('click', back);
backButton.addEventListener('mousemove', () => { focused = -1; highlight(); });

// A click that reaches the window but not the panel landed in the shadow
// margin, which is off the menu.
document.addEventListener('mousedown', (event) => {
  if (!panel.contains(event.target)) dismiss();
});

document.addEventListener('keydown', (event) => {
  const rtl = document.documentElement.dir === 'rtl';
  const forward = rtl ? 'ArrowLeft' : 'ArrowRight';
  const backward = rtl ? 'ArrowRight' : 'ArrowLeft';
  switch (event.key) {
    case 'ArrowDown': move(1); break;
    case 'ArrowUp': move(-1); break;
    case 'Home': focused = -1; move(1); break;
    case 'End': focused = -1; move(-1); break;
    case forward: {
      const item = items()[focused];
      if (item && item.dataset.kind === 'submenu') item.click();
      break;
    }
    case backward: back(); break;
    case 'Escape': back(); break;
    case 'Enter':
    case ' ': {
      const item = items()[focused];
      if (item) item.click();
      break;
    }
    default: return;
  }
  event.preventDefault();
});

// Two dismissals, because they fail in different conditions: the shell's own
// `Focused(false)` covers the window losing focus to another application, and
// this covers the webview losing it while the window keeps it.
window.addEventListener('blur', dismiss);

api.event.listen('writer-tray-menu', ({ payload }) => show(payload));
// A window that finished loading after the menu was published — or reloaded
// while it was open — asks for the rows rather than showing an empty panel.
void api.core.invoke('tray_menu_ready').then(show).catch(() => {});

// Writer draws its own menus. The WebView's own list — Back, Reload, Save
// as — is about the browser rather than about anything in this window, and
// no theme can repaint it, so it never appears.
addEventListener('contextmenu', (event) => event.preventDefault());
