// Compiled replay harness: actual WebView owner -> native capture journal ->
// managed local model -> durable mode -> production delivery. No microphone.
(async () => {
  const result = {};
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const invoke = window.__TAURI__.core.invoke;
  try {
    for (let i = 0; i < 900; i++) {
      const setup = await fetch('/api/v1/setup').then(r => r.json());
      if (window.writerDesktopTranscriptionReady?.() && setup.data?.runtime?.worker?.state === 'ready') break;
      await pause(100);
    }
    result.owner_ready = typeof window.writerDesktopSnapshot === 'function';
    const send = action => invoke('dictation_probe_command', { action });
    await send('start');
    for (let i = 0; i < 200; i++) {
      if (window.writerDesktopSnapshot().state === 'capturing') break;
      await pause(25);
    }
    result.capture_started = window.writerDesktopSnapshot().state === 'capturing';
    const id = window.writerDesktopSnapshot().session_id;
    for (let i = 0; i < 4000; i++) {
      const state = await invoke('capture_status');
      if (state.elapsed >= window.writerReplayDurationMs) break;
      await pause(10);
    }
    const stopped = performance.now();
    let visibleMs = null, visibleText = '';
    const observer = new MutationObserver(() => {
      const node = document.querySelector('.transcript-text');
      if (visibleMs === null && node?.textContent.trim()) {
        visibleText = node.textContent.trim(); visibleMs = performance.now() - stopped;
      }
    });
    observer.observe(document.getElementById('root'), { subtree: true, childList: true, characterData: true });
    await send('stop');
    let receipt;
    for (let i = 0; i < 2000; i++) {
      try { receipt = await invoke('dictation_probe_receipt'); if (receipt) break; } catch {}
      await pause(10);
    }
    const elapsedMs = performance.now() - stopped;
    await invoke('dictation_probe_receipt', { elapsedMs, visibleMs });
    observer.disconnect();
    result.final_delivery = receipt?.pasted === true;
    const snapshot = window.writerDesktopSnapshot();
    const job = await fetch(`/api/v1/jobs/${snapshot.job_id}`).then(r => r.json());
    result.real_transcript = job.data?.status?.state === 'succeeded' && !!job.data.status.result.transcript.raw_text.trim();
    result.transcript_visible = visibleMs !== null && visibleText === job.data.status.result.transcript.raw_text.trim();
    const native = await fetch(`/native/audio/${id}`).then(r => r.arrayBuffer());
    const server = await fetch(`/api/v1/recordings/${id}/audio`).then(r => r.arrayBuffer());
    const hash = async b => Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', b))).join(',');
    result.audio_retained = native.byteLength > 80 && await hash(native) === await hash(server);
  } catch { result.exception = false; }
  await invoke('probe_result', { result });
})();
