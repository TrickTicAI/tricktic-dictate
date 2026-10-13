import type { ContractCatalog } from '@metrix-writer/contracts';

type Request = ContractCatalog['transcription_request'];
type Job = ContractCatalog['job']['data'];
const request: Request = {
  api_version: '1',
  data: {
    recording_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
    idempotency_key: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc',
    options: {
      selection: { provider: 'openai', model: 'fixture-stt' },
      input_language: 'mixed', output_language: { mode: 'preserve' },
      vocabulary: { hints: [], corrections: [], snippets: [] },
      cleanup: { mode: 'off' },
    },
  },
};
void request;
function display(job: Job): string {
  if (job.status.state === 'succeeded') {
    const result = job.status.result;
    return result.cleanup.state === 'succeeded'
      ? result.cleanup.cleaned_text : result.transcript.raw_text;
  }
  return job.status.state === 'failed' ? job.status.error.message : job.status.state;
}
void display;
// @ts-expect-error A successful job must carry its original transcript result.
const invalid: Job['status'] = { state: 'succeeded' };
void invalid;
