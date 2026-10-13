import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import Ajv from 'ajv';
import addFormats from 'ajv-formats';

const read = (path) => JSON.parse(readFileSync(new URL(path, import.meta.url), 'utf8'));
const schema = read('./schema.json');
const examples = read('./examples.json');
const retry = read('./retry-examples.json');
const ajv = new Ajv({ strict: true, allErrors: true });
addFormats(ajv);
ajv.addFormat('uint64', {
  type: 'number',
  validate: value => Number.isSafeInteger(value) && value >= 0,
});
const validate = ajv.compile(schema);
const accept = (payload) => assert.equal(validate(payload), true, JSON.stringify(validate.errors));
const reject = (change) => {
  const payload = structuredClone(examples);
  change(payload);
  assert.equal(validate(payload), false);
};

test('recording, request, raw result, settings and error wire examples', () => accept(examples));
test('retryable failure and new attempt retain the original recording', () => {
  accept({ ...examples, job: retry.failed_job, transcription_request: retry.retry_request });
  assert.equal(retry.failed_job.data.request.recording_id, retry.retry_request.data.recording_id);
  assert.equal(retry.retry_request.data.retry_of_job_id, retry.failed_job.data.id);
  assert.notEqual(retry.retry_request.data.idempotency_key, retry.failed_job.data.request.idempotency_key);
});
test('cleanup failure retains successful raw transcription', () => {
  const payload = structuredClone(examples);
  payload.job.data.status.result.cleanup = { state: 'failed', error: examples.error.error };
  accept(payload);
  assert.equal(payload.job.data.status.result.transcript.raw_text, 'שלום Metrix');
});
test('English, Hebrew, mixed preservation and explicit translation are distinct', () => {
  for (const input_language of ['en', 'he', 'mixed']) {
    const payload = structuredClone(examples);
    payload.transcription_request.data.options.input_language = input_language;
    accept(payload);
    payload.transcription_request.data.options.output_language = { mode: 'translate', language: 'en' };
    accept(payload);
  }
});
test('invalid ID, version, duration, credentials and impossible success are rejected', () => {
  reject(p => p.transcription_request.data.recording_id = '../private');
  reject(p => p.transcription_request.api_version = '2');
  reject(p => p.recording.data.metadata.duration_ms = -1);
  reject(p => p.job.data.updated_at_ms = Number.MAX_SAFE_INTEGER + 1);
  reject(p => p.transcription_request.data.options.selection.api_key = 'fixture-only');
  reject(p => p.job.data.status = { state: 'succeeded' });
  reject(p => p.job.data.status.error = examples.error.error);
  reject(p => p.transcription_request.data.options.output_language = { mode: 'preserve', language: 'en' });
  reject(p => p.job.data.status.result.transcript.execution_location = 'device');
});
