(async () => {
  const { label, commands } = window.writerSecurityFixture;
  const result = {};
  const invoke = window.__TAURI__.core.invoke;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const progress = step => invoke('security_probe_receipt', { result: { ...result, [step]: true }, done: false });
  try {
    await invoke('security_probe_receipt', { result: { started: true }, done: false });
    for (const command of commands) {
      const allowed = label === 'main' ? command !== 'overlay_action' && !command.startsWith('tray_menu_')
        : label === 'overlay' ? command === 'overlay_action'
        : label === 'tray-menu' ? command.startsWith('tray_menu_') : false;
      if (allowed) continue; // No destructive positive calls, no fixture delivery.
      try { await invoke(command, {}); result[`denied:${command}`] = false; }
      catch (e) { result[`denied:${command}`] = String(e).includes('Untrusted document.'); }
    }
    for (const command of ['plugin:shell|execute', 'plugin:fs|read_file', 'plugin:opener|open_url', 'plugin:webview|create_webview_window']) {
      try { await invoke(command, {}); result[`denied:${command}`] = false; }
      catch { result[`denied:${command}`] = true; }
    }
    await invoke('security_probe_receipt', { result: { commands_done: true }, done: false });
    const health = await fetch('/api/v1/health').then(async r => ({ type: r.headers.get('content-type'), body: await r.text() }));
    result.api_relay_scope = label === 'main' ? health.body.includes('metrix-writer-api') : !health.body.includes('metrix-writer-api');
    await invoke('security_probe_receipt', { result: { health_done: true }, done: false });
    if (label === 'main') {
      result.main_read = typeof (await invoke('data_location')).root === 'string';
      await progress('main_read_done');
      try { await invoke('capture_start', { id: crypto.randomUUID(), language: 'en' }); result.microphone_disabled = false; }
      catch (e) { result.microphone_disabled = String(e).includes('Capture is disabled'); }
      await progress('capture_check_done');
      const payload = '<img id=security-payload src=x onerror=window.securityExecuted=true><script>window.securityExecuted=true</script> שלום';
      const waitFor = async predicate => { const until = Date.now() + 6000; while (!predicate() && Date.now() < until) await pause(100); return !!predicate(); };
      const button = text => Array.from(document.querySelectorAll('button')).find(b => b.getAttribute('aria-label') === text || b.textContent.trim() === text || (text === 'Snippets' && b.textContent.trim().startsWith('Snippets')));
      await waitFor(() => document.querySelector('.session-row'));
      document.querySelector('.session-row')?.click();
      result.hostile_transcript_is_text = await waitFor(() => Array.from(document.querySelectorAll('.transcript-text')).some(e => e.textContent === payload));
      await progress('transcript_check_done');
      button('Dictionary')?.click();
      await waitFor(() => button('Snippets'));
      button('Snippets')?.click();
      result.hostile_snippet_is_text = await waitFor(() => Array.from(document.querySelectorAll('.entry-value')).some(e => e.textContent === payload));
      await progress('snippet_check_done');
      // Inject only a fake model note into the real setup response. The actual
      // model row, React renderer and packaged CSP remain unchanged.
      const originalFetch = window.fetch.bind(window);
      window.fetch = async (...args) => {
        const response = await originalFetch(...args);
        if (String(args[0]).endsWith('/api/v1/setup')) {
          const value = await response.clone().json();
          for (const model of value.data?.models ?? []) model.notes = payload;
          return new Response(JSON.stringify(value), {status: response.status, headers: response.headers});
        }
        return response;
      };
      button('Settings')?.click();
      await progress('settings_open_requested');
      await waitFor(() => button('Speech engine'));
      button('Speech engine')?.click();
      await progress('engine_open_requested');
      await waitFor(() => document.querySelector('.setup-model-open'));
      document.querySelector('.setup-model-open')?.click();
      result.hostile_model_note_is_text = await waitFor(() => document.querySelector('.setup-notes')?.textContent === payload);
      await progress('model_note_check_done');
      result.hostile_content_has_no_elements = !document.querySelector('#security-payload') && window.securityExecuted !== true;
      window.fetch = originalFetch;
      const violations = new Set();
      document.addEventListener('securitypolicyviolation', e => violations.add(e.effectiveDirective));
      const inline = document.createElement('script'); inline.textContent = 'window.securityExecuted=true'; document.body.append(inline);
      const img = document.createElement('img'); img.setAttribute('onerror', 'window.securityExecuted=true'); img.src = '/missing-security-fixture'; document.body.append(img);
      for (const src of ['index.html', 'https://evil.invalid/frame']) {
        const frame = document.createElement('iframe'); frame.src = src; document.body.append(frame);
      }
      const frame = document.createElement('iframe'); frame.srcdoc = '<script>parent.securityExecuted=true</script>'; document.body.append(frame);
      const base = document.createElement('base'); base.href = 'https://evil.invalid/'; document.head.append(base);
      await pause(700);
      result.inline_script_blocked = violations.has('script-src-elem') || violations.has('script-src');
      result.frames_blocked = violations.has('frame-src');
      result.base_blocked = violations.has('base-uri');
      result.no_injected_execution = window.securityExecuted !== true;
      result.base_unchanged = new URL(document.baseURI).host === new URL(location.href).host;
    }
    if (label === 'overlay') { await invoke('overlay_action', { action: 'ready' }); result.overlay_ready = true; }
    if (label === 'tray-menu') { await invoke('tray_menu_ready'); result.tray_ready = true; }
    await progress('content_checks_done');
    window.open('https://evil.invalid/popup', '_blank');
    window.open('/index.html', '_blank');
    const original = location.href;
    await progress('popup_checks_done');
    for (const target of ['https://evil.invalid/page', '/api/v1/health', '/%69ndex.html']) {
      await progress(`navigation_started:${target}`);
      location.href = target;
      await pause(250);
      result[`navigation_blocked:${target}`] = location.href === original;
      await progress(`navigation_done:${target}`);
    }
  } catch { result.fixture_completed = false; }
  await invoke('security_probe_receipt', { result });
})();
