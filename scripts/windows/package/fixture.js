// Hidden native test only. Fake-device Chromium flag supplies synthetic audio.
// Provider *discovery* is a fixture; uploads and provider failures hit the real API.
window.__integratedStreams = 0;
window.__integratedSubmissions = 0;
window.__integratedUploads = 0;
const getUserMedia = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
navigator.mediaDevices.getUserMedia = (...args) => { window.__integratedStreams++; return getUserMedia(...args); };
const request = window.fetch.bind(window);
window.fetch = async (input, init) => {
  const path = typeof input === 'string' ? input : input.url;
  if (path === '/api/v1/providers') return new Response(JSON.stringify({ providers: [{ id: 'openai', name: 'OpenAI', configured: true, available: false, status: 'unverified' }] }), { headers: { 'Content-Type': 'application/json' } });
  if (path === '/api/v1/transcriptions' && init?.method === 'POST') window.__integratedSubmissions++;
  if (path === '/api/v1/recordings' && init?.method === 'POST') window.__integratedUploads++;
  return request(input, init);
};
(async () => {
  if (document.readyState !== 'complete') await new Promise(resolve => window.addEventListener('load', resolve, { once: true }));
  const response = await request('/api/v1/settings');
  if (!response.ok) { window.__integratedReady = 'settings-http:' + response.status; return; }
  const settings = (await response.json()).data;
  settings.transcription.selection = { provider: 'openai', model: 'gpt-transcribe' };
  settings.transcription.input_language = 'en';
  const saved = await request('/api/v1/settings', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ api_version: '1', data: settings }) });
  if (!saved.ok) { window.__integratedReady = 'settings-rejected:' + saved.status; return; }
  // The app's next bounded settings poll must read the persisted selection.
  setTimeout(() => { window.__integratedReady = true; }, 4000);
})().catch(error => { window.__integratedReady = 'fixture-error:' + error.message; });
