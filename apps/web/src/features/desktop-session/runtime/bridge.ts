import { isTauri, invoke, listen } from './native';
import { parseMessage, type Ack, type Level, type Snapshot } from '../contracts/protocol';
import type { SessionOwner } from './owner';

interface WebViewTransport {
  postMessage(message: Ack | Snapshot | Level): void;
  addEventListener(type: 'message', listener: (event: MessageEvent<unknown>) => void): void;
  removeEventListener(type: 'message', listener: (event: MessageEvent<unknown>) => void): void;
}
declare global {
  interface Window {
    writerDesktopSnapshot?: () => Snapshot;
    writerDesktopTranscriptionReady?: () => boolean;
  }
}
/** Install once on the persistent owner, never a transient React component.
 * Native transport separately checks both top-level source and current document. */
export function installOwnerBridge(owner: SessionOwner, supplied?: WebViewTransport) {
  if (isTauri() && !supplied) return installTauriBridge(owner);
  if (location.origin !== 'https://writer.metrix.invalid') throw new Error('untrusted_origin');
  const transport = supplied ?? (window as unknown as { chrome?: { webview?: WebViewTransport } }).chrome?.webview;
  if (!transport || window.writerDesktopSnapshot) throw new Error('bridge_unavailable_or_duplicate');
  let enabled = false;
  let disposed = false;
  const controls: (Snapshot | Ack)[] = [];
  let latest: Level | undefined;
  let scheduled = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const fail = () => {
    enabled = false; controls.length = 0; latest = undefined;
    // Overflow/transport loss drains capture; a missing stop must not leave the
    // microphone running while the native observer has lost authoritative state.
    void owner.stopAndDrain();
  };
  const flush = () => {
    scheduled = false;
    if (!enabled || disposed) return;
    try {
      for (const message of controls.splice(0)) transport.postMessage(message);
    } catch { fail(); }
  };
  const send = (message: Snapshot | Ack | Level) => {
    if (!enabled || disposed) return;
    parseMessage(JSON.stringify(message));
    if (message.kind === 'level') {
      latest = message;
      if (!timer) timer = setTimeout(() => {
        timer = undefined;
        const sample = latest; latest = undefined;
        if (enabled && sample && owner.snapshot.state === 'capturing' && sample.revision === owner.snapshot.revision) {
          try { transport.postMessage(sample); } catch { fail(); }
        }
      }, 50);
    } else {
      if (controls.length >= 32) { fail(); return; }
      controls.push(message);
      if (!scheduled) { scheduled = true; queueMicrotask(flush); }
    }
  };
  const receive = (event: MessageEvent<unknown>) => {
    if (!enabled || disposed) return;
    try {
      const command = parseMessage(typeof event.data === 'string' ? event.data : JSON.stringify(event.data));
      if (command.kind !== 'command') { fail(); return; }
      void owner.dispatch(command).then(send).catch(fail);
    } catch { fail(); }
  };
  window.writerDesktopSnapshot = () => { controls.length = 0; latest = undefined; enabled = !disposed; return owner.snapshot; };
  const unsubscribe = owner.subscribe(send);
  transport.addEventListener('message', receive);
  return () => {
    disposed = true; fail(); clearTimeout(timer); unsubscribe();
    transport.removeEventListener('message', receive);
    delete window.writerDesktopSnapshot;
  };
}

function installTauriBridge(owner: SessionOwner) {
  let disposed = false;
  const ready = listen<unknown>('writer-command', data => {
    try {
      const command = parseMessage(JSON.stringify(data));
      if (command.kind !== 'command') return;
      void owner.dispatch(command).then(ack => {
        // A start the owner cannot take up is reported back, so the shell stops
        // waiting for it and the panel says why instead of doing nothing.
        if (command.action === 'start' && ack.outcome === 'rejected') {
          void invoke('session_refused', { sessionId: command.session_id, code: ack.error_code }).catch(() => {});
        }
      });
    } catch { void owner.stopAndDrain(); }
  });
  const quit = listen('writer-quit', () => {
    void owner.stopAndDrain().then(() => invoke('quit_ready')).catch(() => {});
  });
  const send = (message: Snapshot | Level) => { void invoke('session_event', { message }).catch(() => { void owner.stopAndDrain(); }); };
  const unsubscribe = owner.subscribe(send);
  // How many earlier dictations still have text to come, for the panel's count.
  let waiting = 0;
  const unsubscribeWaiting = owner.subscribeWaiting(() => {
    if (owner.waiting === waiting) return;
    waiting = owner.waiting;
    void invoke('session_waiting', { count: waiting }).catch(() => {});
  });
  void ready.then(() => { if (!disposed) send(owner.snapshot); });
  window.writerDesktopSnapshot = () => owner.snapshot;
  window.writerDesktopTranscriptionReady = () => owner.transcriptionReady;
  return () => { disposed = true; unsubscribe(); void ready.then(f => f()); void quit.then(f => f()); delete window.writerDesktopSnapshot; delete window.writerDesktopTranscriptionReady; };
}
