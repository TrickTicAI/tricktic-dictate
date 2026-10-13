import type { PortableBackup, OutboxOperation, SyncResponse, PortableSetting } from '@metrix-writer/contracts/account-sync';

const setting: PortableSetting = { kind: 'setting', key: 'input_language', value: 'mixed' };
const polish: PortableSetting = { kind: 'setting', key: 'polish_instructions', value: { instructions: 'Polish this dictation.', formatting: { rewrite: 'polished', layout: 'automatic', lists: true, remove_fillers: true, self_corrections: true, spoken_punctuation: true } } };
// @ts-expect-error Polish instructions carry instructions and formatting, not a bare string.
const wrongPolish: PortableSetting = { kind: 'setting', key: 'polish_instructions', value: 'polished' };
// @ts-expect-error Input language never accepts a provider/model object.
const wrongSetting: PortableSetting = { kind: 'setting', key: 'input_language', value: { provider: 'local', model: 'fixture' } };
// @ts-expect-error Ordinary settings exports do not include transcripts or credentials.
const wrongBackup: PortableBackup = { format: 'metrix-writer-portable', format_version: 1, transcripts: [] };
// @ts-expect-error An operation must bind UID, epoch, consent and base revision.
const unbound: OutboxOperation = { schema_version: 1, category: 'dictionary', operation_id: 'fixture' };
function status(response: SyncResponse): string {
  if (response.status === 'acknowledged') return String(response.record.revision);
  if (response.status === 'conflict') return String(response.current.revision);
  return response.reason;
}
void [setting, polish, wrongPolish, wrongSetting, wrongBackup, unbound, status];
