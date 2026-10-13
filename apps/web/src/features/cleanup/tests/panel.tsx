import { createRoot } from 'react-dom/client';
import { useState } from 'react';
import type { Job } from '@metrix-writer/contracts';
import { CleanupPanel } from '../CleanupPanel';
const initial: Job = {
  id: 'eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee', created_at_ms: 1, updated_at_ms: 1,
  request: { recording_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', idempotency_key: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', options: {
    selection: { provider: 'openai', model: 'gpt-4o-mini-transcribe' }, input_language: 'mixed', output_language: { mode: 'preserve' },
    vocabulary: { hints: [], corrections: [], snippets: [] }, cleanup: { mode: 'off' },
  } },
  status: { state: 'succeeded', result: { transcript: { raw_text: 'שלום hello Foo@EXAMPLE.com', selection: {provider: 'openai', model: 'gpt-4o-mini-transcribe'}, execution_location: 'cloud', input_language: 'mixed', output_language: {mode: 'preserve'} }, cleanup: {state: new URLSearchParams(location.search).has('pending') ? 'pending' : 'off'} } },
};
function App() {
  const [job, setJob] = useState(initial);
  return <CleanupPanel job={job} onJobChange={setJob} />;
}
createRoot(document.getElementById('root')!).render(<App />);
