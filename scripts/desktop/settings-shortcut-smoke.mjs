// Native Windows shortcut recovery after settings save, in a fresh profile.
// Capture is held at the WebView boundary: no microphone or model is used.
// A probe-only marker removes our own hook; production renewal must restore it.
import assert from 'node:assert/strict';
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

if (process.platform !== 'win32' || !process.argv[2]) throw Error('Run on Windows: node settings-shortcut-smoke.mjs <diagnostic-exe>');
const physical = process.argv.includes('--physical');

const root = await mkdtemp(join(tmpdir(), 'writer-in-app-'));
await mkdir(join(root, 'State'));
await writeFile(join(root, 'shortcut-probe'), '');
await writeFile(join(root, 'State', 'onboarding.json'), '{"completed_version":1}');
await writeFile(join(root, 'State', 'tauri-preferences.json'), JSON.stringify({
  recording: 'Win+Alt', cancel: 'Ctrl+Alt+Shift+F7', toggle: true,
}));
const app = spawn(resolve(process.argv[2]), ['--user-root', root], {
  env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: '--remote-debugging-port=0' }, stdio: 'ignore',
});
const sockets = [];
const receipt = {};
let phase = 'first window';
let mainWindow;
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, timeout = 30000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try { const value = await check(); if (value) return value; } catch {}
    await pause(100);
  }
  throw Error('Native in-app dictation condition timed out');
}
async function connect(profile, suffix, instanceRoot = root) {
  const target = await until(async () => {
    const port = Number((await readFile(join(instanceRoot, profile, 'EBWebView', 'DevToolsActivePort'), 'utf8')).split('\n')[0]);
    const tabs = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
    return tabs.find(tab => tab.type === 'page' && tab.url.endsWith(suffix));
  });
  const socket = new WebSocket(target.webSocketDebuggerUrl); sockets.push(socket);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let serial = 0;
  const waiting = new Map();
  socket.onmessage = event => {
    const data = JSON.parse(event.data); const pending = waiting.get(data.id);
    if (pending) { waiting.delete(data.id); clearTimeout(pending.timer); data.error ? pending.reject(Error(data.error.message)) : pending.resolve(data.result); }
  };
  function call(method, params = {}) {
    return new Promise((resolve, reject) => {
      const id = ++serial;
      const timer = setTimeout(() => { waiting.delete(id); reject(Error(`${method} timed out`)); }, 10000);
      waiting.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }));
    });
  }
  return async expression => {
    const result = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
}
async function native(action, processId = app.pid, windowHandle = mainWindow) {
  const script = `
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class WriterInput {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern short GetAsyncKeyState(int key);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
  [StructLayout(LayoutKind.Sequential)] public struct KeyInput { public ushort vk, scan; public uint flags, time; public UIntPtr extra; }
  [StructLayout(LayoutKind.Explicit, Size=40)] public struct Input { [FieldOffset(0)] public uint kind; [FieldOffset(8)] public KeyInput key; }
  [DllImport("user32.dll", SetLastError=true)] public static extern uint SendInput(uint count, Input[] inputs, int size);
  public static void Send(byte vk, bool up, ulong tag) {
    var input = new Input { kind = 1, key = new KeyInput { vk = vk, flags = (up ? 2u : 0u) | (vk == 0x5b || vk == 0x5c ? 1u : 0u), extra = new UIntPtr(tag) } };
    if (SendInput(1, new[] { input }, 40) != 1) throw new Exception("SendInput rejected fixture key: " + Marshal.GetLastWin32Error());
  }
  public delegate IntPtr Hook(int code, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll")] public static extern IntPtr SetWindowsHookEx(int id, Hook callback, IntPtr module, uint thread);
  [DllImport("user32.dll")] public static extern IntPtr CallNextHookEx(IntPtr hook, int code, IntPtr wp, IntPtr lp);
  [StructLayout(LayoutKind.Sequential)] public struct Message { public IntPtr hwnd; public uint msg; public UIntPtr wp; public IntPtr lp; public uint time; public int x, y; public uint priv; }
  [DllImport("user32.dll")] public static extern int GetMessage(out Message msg, IntPtr hwnd, uint min, uint max);
  static Hook observer;
  public static int Observed;
  public static void Observe() {
    var ready = new System.Threading.ManualResetEvent(false);
    var installed = false;
    var thread = new System.Threading.Thread(() => {
      observer = (code, wp, lp) => { if (code >= 0 && Marshal.ReadInt64(lp,16) == 0x57525042) System.Threading.Interlocked.Increment(ref Observed); return CallNextHookEx(IntPtr.Zero,code,wp,lp); };
      installed = SetWindowsHookEx(13, observer, IntPtr.Zero, 0) != IntPtr.Zero; ready.Set();
      Message msg; while (GetMessage(out msg, IntPtr.Zero, 0, 0) > 0) {}
    }); thread.IsBackground = true; thread.Start(); ready.WaitOne();
    if (!installed) throw new Exception("Fixture input observer could not start");
  }
  public delegate bool WindowCallback(IntPtr hwnd, IntPtr data);
  [DllImport("user32.dll")] public static extern bool EnumWindows(WindowCallback callback, IntPtr data);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint process);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);
  public static bool PanelVisible(uint process) {
    bool visible = false;
    EnumWindows((hwnd, data) => {
      uint owner; GetWindowThreadProcessId(hwnd, out owner);
      var text = new StringBuilder(100); GetWindowText(hwnd, text, 100);
      if (owner == process && text.ToString() == "TrickTic Dictate recording") visible = IsWindowVisible(hwnd);
      return true;
    }, IntPtr.Zero);
    return visible;
  }
  public static IntPtr MainWindow(uint process) {
    IntPtr main = IntPtr.Zero;
    EnumWindows((hwnd, data) => {
      uint owner; GetWindowThreadProcessId(hwnd, out owner);
      var text = new StringBuilder(100); GetWindowText(hwnd, text, 100);
      if (owner == process && text.ToString() == "TrickTic Dictate") main = hwnd;
      return true;
    }, IntPtr.Zero);
    return main;
  }
  public static void Focus(IntPtr hwnd) {
    SetForegroundWindow(hwnd);
  }
  public static uint Owner(IntPtr hwnd) { uint owner; GetWindowThreadProcessId(hwnd, out owner); return owner; }
}
'@
$window = ${windowHandle ? `[IntPtr]${windowHandle}` : `[WriterInput]::MainWindow(${processId})`}
${action}
`;
  const { stdout } = await promisify(execFile)('powershell.exe', ['-NoProfile', '-NonInteractive', '-EncodedCommand', Buffer.from(script, 'utf16le').toString('base64')], { windowsHide: true, timeout: 30000 })
    .catch(error => { throw Error(error.stderr || error.message || 'Native fixture command failed'); });
  return stdout.trim();
}
async function editor(main) {
  phase += ': owner';
  await until(() => main('!!window.writerDesktopSnapshot'));
  await main(`window.fixtureInvoke = window.__TAURI__.core.invoke;
    window.fixtureCapture = (command, args) => command === 'capture_start'
      ? new Promise((_resolve, reject) => { window.fixtureReject = reject; }) : window.fixtureInvoke(command, args);
    window.__TAURI__.core = {...window.__TAURI__.core, invoke: window.fixtureCapture};
    if (window.__TAURI__.core.invoke !== window.fixtureCapture) throw Error('Cannot isolate microphone capture');
    [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Dictionary').click();`);
  phase += ': dictionary';
  await until(() => main(`!![...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Add word')`));
  await main(`[...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Add word').click()`);
  await until(() => main(`!!document.querySelector('[role=dialog] input')`));
  await main(`document.querySelector('[role=dialog] input').focus()`);
}
async function trigger(processId, windowHandle, keys, tag = 0) {
  assert.equal(await native(`
foreach ($key in @(0x11,0x12,0x10,0x5b,0x5c)) {
  if ([WriterInput]::GetAsyncKeyState($key) -lt 0) { throw 'Release physical modifiers before running the automated test' }
}
[WriterInput]::Focus($window)
if ([WriterInput]::GetForegroundWindow() -ne $window) {
  # A synthetic Alt gesture releases the foreground lock. The unused key masks
  # menu activation; no recording chord is sent until our own window is in front.
  [WriterInput]::keybd_event(0xa4,0,0,[UIntPtr]::Zero)
  try {
    [WriterInput]::keybd_event(0xe8,0,0,[UIntPtr]::Zero)
    [WriterInput]::keybd_event(0xe8,0,2,[UIntPtr]::Zero)
    [WriterInput]::Focus($window)
  } finally { [WriterInput]::keybd_event(0xa4,0,2,[UIntPtr]::Zero) }
}
Start-Sleep -Milliseconds 200
if ([WriterInput]::GetForegroundWindow() -ne $window) { throw 'Fixture did not receive focus' }
[WriterInput]::Observe()
try {
  foreach ($key in @(${keys.join(',')})) { [WriterInput]::Send($key,$false,${tag}) }
  Start-Sleep -Milliseconds 100
} finally {
  foreach ($key in @(${[...keys].reverse().join(',')})) { [WriterInput]::Send($key,$true,${tag}) }
}
Start-Sleep -Milliseconds 300
if (${tag} -ne 0 -and [WriterInput]::Observed -ne ${keys.length * 2}) { throw ('Observer saw ' + [WriterInput]::Observed + ' tagged events') }
'sent'`, processId, windowHandle), 'sent');
}
try {
  const main = await connect('TauriWebView2', '');
  await editor(main);
  mainWindow = await native('$window.ToInt64()');
  assert.notEqual(mainWindow, '0');
  phase = 'enable preservation';
  await main(`window.fixtureInvoke('preferences_get').then(p => window.fixtureInvoke('preferences_save', {preferences: {...p, preserve_clipboard: true}}))`);
  receipt.savedPreservation = await main(`window.fixtureInvoke('preferences_get').then(p => p.preserve_clipboard)`);
  assert.equal(receipt.savedPreservation, true);
  phase = 'shortcut';

  if (physical) {
    await main(`const banner = document.createElement('div'); banner.textContent = 'Isolated dictation test: click the Word field, then press Win + Alt. No microphone is used.';
      banner.style.cssText = 'position:fixed;top:0;left:0;right:0;padding:20px;background:#422284;color:white;z-index:99999'; document.body.append(banner)`);
    console.log('Ready for physical Win+Alt in the isolated Writer Word field. No microphone is used.');
    await until(() => main(`window.writerDesktopSnapshot().state === 'requesting'`), 900000);
  } else {
    // Only diagnostic builds with the private probe marker accept tagged input.
    // Production modifier hooks continue to ignore all injected events.
    await trigger(app.pid, mainWindow, [0x5b, 0xa4], 0x57525042);
    await until(() => main(`window.writerDesktopSnapshot().state === 'requesting'`));
  }
  receipt.shortcutAfterSettings = true;
  if (!physical) {
    phase = 'simulate lost hook';
    await trigger(app.pid, mainWindow, [0x11, 0x12, 0x10, 0x76]);
    await until(() => main(`window.writerDesktopSnapshot().state === 'canceled'`));
    await main(`window.fixtureReject('Fixture microphone intentionally disabled'); new Promise(resolve => setTimeout(resolve, 50))`);
    const probe = async () => JSON.parse(await readFile(join(root, 'State', 'shortcut-probe.json'), 'utf8'));
    await writeFile(join(root, 'remove-shortcut-hook'), '');
    const removed = await until(async () => { const p = await probe(); return p.removed > 0 && p; }, 12000);
    phase = 'hook recovery';
    await until(async () => (await probe()).renewed > removed.renewed, 12000);
    await trigger(app.pid, mainWindow, [0x5b, 0xa4], 0x57525042);
    await until(() => main(`window.writerDesktopSnapshot().state === 'requesting'`));
    receipt.recoveredAfterHookRemoval = true;
    assert.equal(await main(`window.fixtureInvoke('preferences_get').then(p => p.preserve_clipboard)`), true);
  }
  assert.equal(await native(`[WriterInput]::PanelVisible(${app.pid})`), 'True');
  receipt.panelVisible = true;
  receipt.microphoneStayedIdle = await main(`window.fixtureInvoke('capture_status').then(s => !s.active)`);
  assert.equal(receipt.microphoneStayedIdle, true);
  console.log(JSON.stringify({ root, ...receipt }));
} catch (error) {
  receipt.shortcutProbe = await readFile(join(root, 'State', 'shortcut-probe.json'), 'utf8').then(JSON.parse, () => null);
  console.error(JSON.stringify({ root, phase, ...receipt, error: String(error) })); process.exitCode = 1;
} finally {
  for (const socket of sockets) socket.close();
  app.kill();
}
