(async () => {
  const result = {};
  try {
    const mounted = () => !!document.querySelector('#root .shell-body > .page') && !!document.querySelector('#root nav[aria-label="Sections"]');
    for (let i=0; i<100 && (!window.writerDesktopSnapshot || !mounted()); i++) await new Promise(r=>setTimeout(r,100));
    result.owner_ready = typeof window.writerDesktopSnapshot === 'function';
    result.react_mounted = mounted();
    const health = await fetch('/api/v1/health').then(r=>r.json());
    result.axum_health = health.service === 'metrix-writer-api';
    const setup = await fetch('/api/v1/setup').then(r=>r.json());
    result.desktop_setup = setup.data?.mode === 'desktop';
    const invoke = window.__TAURI__.core.invoke;
    const sessions = await invoke('capture_list');
    result.native_storage = Array.isArray(sessions);
    const attempts = await invoke('saved_attempts');
    result.raw_history_restored = attempts.some(a => a.job?.status.result?.transcript.raw_text === 'synthetic fixture original');
    try { await invoke('capture_start',{id:crypto.randomUUID(),language:'en'}); result.microphone_disabled=false; } catch { result.microphone_disabled=true; }
    if (sessions.length) {
      const m=sessions[0]; const audio=await fetch(`/native/audio/${m.id}`).then(r=>r.blob());
      result.recovered_audio=audio.size===m.byte_length && m.capture_state==='interrupted';
      const {id,created_at_ms,duration_ms,byte_length,mime_type,input_language,capture_state}=m;
      const form=new FormData();form.append('metadata',JSON.stringify({api_version:'1',data:{id,created_at_ms,duration_ms,byte_length,mime_type,input_language,capture_state}}));form.append('audio',audio,'recording.wav');
      await invoke('capture_import', { id }); result.native_registration = true;
      const upload=await fetch('/api/v1/recordings',{method:'POST',body:form});result.multipart_upload=upload.ok || upload.status===409;
      const retained=await fetch(`/api/v1/recordings/${id}/audio`).then(r=>r.arrayBuffer());
      const digest=async b=>Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',b))).join(',');
      result.identical_retained_audio=await digest(retained)===await digest(await audio.arrayBuffer());
    }
    const usage = await invoke('storage_status');
    result.storage_usage = usage.data_bytes > 0 && usage.audio_bytes > 0 && usage.data_limit_bytes >= usage.data_bytes;
    const today = new Date(); today.setHours(0,0,0,0);
    const preview = await invoke('storage_preview', { beforeMs: today.getTime(), history: true });
    result.storage_date_preview = preview.recordings === 1 && preview.jobs === 1 && preview.before_ms === today.getTime() && preview.history === true;
    try { await invoke('storage_clear', { previewId: crypto.randomUUID() }); result.storage_invalid_confirmation = false; }
    catch { result.storage_invalid_confirmation = true; }
  } catch { result.exception=false; }
  await window.__TAURI__.core.invoke('probe_result',{result});
})();
