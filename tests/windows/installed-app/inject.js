// External acceptance instrumentation; never distributed. No provider responses are mocked.
(() => {
  const a = window.acceptance = { streams: 0, uploads: 0, submissions: [], states: [], levels: [], copied: null, deny: false, silence: false, lose: null, storageFailures: 0 };
  const post = window.chrome.webview.postMessage.bind(window.chrome.webview);
  window.chrome.webview.postMessage = message => {
    if (message.kind === 'snapshot' && a.states.at(-1) !== message.state) a.states.push(message.state);
    if (message.kind === 'level' && a.levels.length < 1000) a.levels.push(message.rms);
    return post(message);
  };
  // Decode the explicitly supplied public/consented clip into a stream. Nothing connects
  // to AudioContext.destination, so neither speakers nor the host microphone are used.
  navigator.mediaDevices.getUserMedia = async () => {
    if (a.deny) throw new DOMException('Injected permission failure', 'NotAllowedError');
    if (a.delayMedia) await new Promise(resolve => { a.releaseMedia = resolve; });
    a.streams++;
    const context = new AudioContext();
    await context.resume();
    const output = context.createMediaStreamDestination();
    const source = context.createBufferSource();
    const bytes = Uint8Array.from(atob(window.acceptanceClip), c => c.charCodeAt(0));
    source.buffer = await context.decodeAudioData(bytes.buffer);
    source.loop = !!a.loop;
    const gain = context.createGain(); gain.gain.value = a.silence ? 0 : 1;
    source.connect(gain).connect(output); source.start(context.currentTime + 0.3);
    output.stream.getTracks().forEach(track => {
      const stop = track.stop.bind(track);
      track.stop = () => { stop(); void context.close(); };
    });
    return output.stream;
  };
  Object.defineProperty(navigator, 'clipboard', { value: { writeText: async text => { a.copied = text; } } });
  const add = IDBObjectStore.prototype.add;
  IDBObjectStore.prototype.add = function (...args) {
    if (a.storageFail && this.name === 'sessions') { a.storageFailures++; throw new DOMException('Injected recording-store failure', 'QuotaExceededError'); }
    return add.apply(this, args);
  };
  const fetch = window.fetch.bind(window);
  window.fetch = async (input, init) => {
    const path = typeof input === 'string' ? input : input.url;
    if (path === '/api/v1/recordings' && init?.method === 'POST') a.uploads++;
    if (path === '/api/v1/transcriptions' && init?.method === 'POST') a.submissions.push(JSON.parse(init.body).data);
    const response = await fetch(input, init);
    if (a.lose === path && init?.method === 'POST') { a.lose = null; throw new TypeError('Injected lost response after commit'); }
    return response;
  };
  a.api = async (path, method = 'GET', data) => {
    const response = await window.fetch('/api/v1/' + path, { method, ...(data === undefined ? {} : { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ api_version: '1', data }) }) });
    if (!response.ok) throw new Error('API status ' + response.status + ' at ' + path);
    return (await response.json()).data;
  };
  a.select = async () => {
    const settings = await a.api('settings');
    settings.transcription.selection = { provider: 'local', model: window.acceptanceConfig.model };
    settings.transcription.input_language = window.acceptanceConfig.language;
    return a.api('settings', 'PUT', settings);
  };
  a.saved = async id => {
    const database = await new Promise((resolve, reject) => {
      const request = indexedDB.open('metrix-writer-recordings', 1);
      request.onsuccess = () => resolve(request.result); request.onerror = () => reject(request.error);
    });
    try {
      const stored = await new Promise((resolve, reject) => {
        const tx = database.transaction(['sessions', 'chunks']);
        const metadata = tx.objectStore('sessions').get(id);
        const chunks = tx.objectStore('chunks').getAll(IDBKeyRange.bound([id, 0], [id, Number.MAX_SAFE_INTEGER]));
        tx.oncomplete = () => resolve({ metadata: metadata.result, chunks: chunks.result });
        tx.onabort = () => reject(tx.error);
      });
      const bytes = await new Blob(stored.chunks.map(chunk => chunk.blob)).arrayBuffer();
      const decoder = new AudioContext();
      let duration;
      try { duration = (await decoder.decodeAudioData(bytes.slice(0))).duration * 1000; }
      finally { await decoder.close(); }
      return { metadata: stored.metadata, bytes: bytes.byteLength, decoded_duration_ms: duration, sha256: [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map(v => v.toString(16).padStart(2, '0')).join('') };
    } finally { database.close(); }
  };
})();
