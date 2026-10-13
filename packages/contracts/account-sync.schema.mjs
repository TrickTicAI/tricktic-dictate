// Canonical, transport-neutral v1 schema. No platform store is serialized wholesale.
const ref = name => ({ $ref: `#/definitions/${name}` });
const object = properties => ({ type: 'object', properties,
  required: Object.keys(properties), additionalProperties: false });
const choice = (...values) => ({ type: 'string', enum: values });
const literal = value => ({ const: value, type: typeof value });
const text = (maxLength, minLength = 0) => ({ type: 'string', minLength, maxLength,
  pattern: '^[^\\u0000]*$' });
const array = (items, maxItems) => ({ type: 'array', items, maxItems });
const uint = { type: 'integer', minimum: 0, maximum: Number.MAX_SAFE_INTEGER };
const positive = { ...uint, minimum: 1 };
const uuid = { type: 'string', format: 'uuid', pattern: '^[0-9a-f-]{36}$' };
const bool = { type: 'boolean' };
const version = literal(1);
const category = choice('settings', 'dictionary', 'transcripts');
const uid = text(128, 1); // Opaque Firebase identity, never email or a filesystem path.
const outputLanguage = { oneOf: [object({ mode: literal('preserve') }),
  object({ mode: literal('translate'), language: choice('en', 'he') })] };
const model = object({ provider: choice('openai', 'gemini', 'grok', 'openrouter', 'local'),
  model: { type: 'string', minLength: 1, maxLength: 256,
    pattern: '^[A-Za-z0-9][A-Za-z0-9._:/~-]*$' } });
const cleanup = { oneOf: [object({ mode: literal('off') }), object({
  mode: literal('enabled'), selection: ref('PortableModel'),
  reasoning: choice('provider_default', 'off', 'minimal', 'low', 'medium', 'high'),
})] };
// Super polishing instructions plus the writing preferences the desktop Modes
// page edits (MW-SETTINGS-8). The phone uses the same text for live polishing;
// bytes and blankness are enforced per platform (16384 bytes, non-blank).
const polish = object({ instructions: text(16384, 1), formatting: object({
  rewrite: choice('light', 'polished', 'concise'),
  layout: choice('automatic', 'single_line'),
  lists: bool, remove_fillers: bool, self_corrections: bool, spoken_punctuation: bool,
}) });

// One record per field: independent offline settings edits do not collide.
export const settingKeys = [
  'ui_language', 'input_language', 'output_language', 'transcription_model',
  'dictation_mode', 'cleanup', 'delivery', 'theme', 'polish_instructions',
];
export const settingIds = Object.fromEntries(settingKeys.map((key, index) =>
  [key, `00000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`]));
const settingValues = [choice('en', 'he'), choice('en', 'he', 'mixed'), outputLanguage,
  ref('PortableModel'), choice('regular', 'super', 'command'), ref('PortableCleanup'),
  choice('raw', 'dictionary', 'cleaned'), choice('system', 'light', 'dark'), ref('PortablePolish')];
const setting = { oneOf: settingKeys.map((key, index) => object({
  kind: literal('setting'), key: literal(key), value: settingValues[index],
})) };
const dictionary = { oneOf: [
  object({ kind: literal('hint'), enabled: bool, text: text(8192, 1) }),
  object({ kind: literal('correction'), enabled: bool, from: text(8192, 1), to: text(8192, 1) }),
  object({ kind: literal('snippet'), enabled: bool, phrase: text(8192, 1), text: text(8192, 1) }),
] };
const original = object({ kind: literal('original'), attempt_id: uuid,
  recorded_at_ms: uint, input_language: choice('en', 'he', 'mixed'),
  output_language: outputLanguage, selection: ref('PortableModel'), text: text(131072),
});
const derived = kind => object({ kind: literal(kind), original_record_id: uuid,
  parent_record_id: uuid, created_at_ms: uint, text: text(131072) });
const transcript = { oneOf: [original, derived('corrected'), derived('cleaned'), derived('edited')] };
const data = { settings: ref('PortableSetting'), dictionary: ref('PortableDictionaryEntry'),
  transcripts: ref('PortableTranscript') };
const content = cat => ({ oneOf: [
  object({ state: literal('live'), data: data[cat] }),
  object({ state: literal('deleted') }),
] });
const identity = { schema_version: version, id: uuid, origin_device_id: uuid };
const record = cat => object({ ...identity, category: literal(cat), revision: positive,
  content: content(cat) });
const binding = { uid, account_epoch: positive };
const operation = cat => object({ schema_version: version, ...binding,
  consent_generation: positive, category: literal(cat), operation_id: uuid,
  device_id: uuid, record_id: uuid, origin_device_id: uuid, base_revision: uint,
  ...(cat === 'transcripts' ? { history_enrollment_id: uuid } : {}),
  content: content(cat) });
const toggle = object({ enabled: bool, generation: positive });
const history = { oneOf: [object({ mode: literal('future_only'), enrollment_id: uuid }),
  object({ mode: literal('include_existing'), enrollment_id: uuid })] };
const consent = object({ schema_version: version, ...binding,
  revision: positive, settings: toggle, dictionary: toggle,
  transcripts: { oneOf: [
    object({ enabled: literal(false), generation: positive, history: object({ mode: literal('off') }) }),
    object({ enabled: literal(true), generation: positive, history }),
  ] },
});
// Per-device acknowledgement of fenced generations (MW-OUATH-5). It gates
// tombstone compaction and `everywhere` resets; it carries no content or hardware facts.
const device = object({ schema_version: version, ...binding, device_id: uuid,
  reconciled_generations: object({ settings: positive, dictionary: positive, transcripts: positive }) });
const exportRecord = cat => object({ ...identity, category: literal(cat), data: data[cat] });
const backup = object({ format: literal('metrix-writer-portable'), format_version: version,
  export_id: uuid, created_at_ms: uint,
  settings: array(ref('ExportSetting'), 9), dictionary: array(ref('ExportDictionaryEntry'), 600),
});
const scope = { oneOf: [object({ kind: literal('categories'),
  categories: { ...array(category, 3), minItems: 1, uniqueItems: true } }),
  object({ kind: literal('account') })] };
const cleanupVariants = fields => ({ oneOf: scope.oneOf.map((scopeSchema, index) =>
  object({ ...fields, scope: scopeSchema,
    effect: index === 0 ? choice('cloud_only', 'everywhere') : literal('cloud_only') })) });
const cleanupRequest = cleanupVariants({ schema_version: version, ...binding,
  operation_id: uuid, expected_consent_revision: positive,
});
const cleanupReceipt = cleanupVariants({ schema_version: version, ...binding,
  operation_id: uuid, job_id: uuid,
  state: choice('pending', 'running', 'failed_retryable', 'completed'),
  fenced_account_epoch: positive, fenced_consent: ref('ConsentState'),
});
const importPreview = object({ schema_version: version, import_id: uuid, export_id: uuid,
  mode: choice('merge', 'replace_selected'),
  categories: { ...array(choice('settings', 'dictionary'), 2), minItems: 1, uniqueItems: true },
  disposition: choice('restore_once', 'restore_and_sync'),
  expected_local_revision: uint,
  apply_ids: { ...array(uuid, 608), uniqueItems: true },
  preserve_ids: { ...array(uuid, 608), uniqueItems: true },
  conflict_ids: { ...array(uuid, 608), uniqueItems: true },
});
const importReceipt = { oneOf: [object({ schema_version: version, import_id: uuid,
  state: literal('applied'), local_revision: positive, rollback_snapshot_id: uuid }),
  object({ schema_version: version, import_id: uuid, state: literal('not_applied'),
    reason: choice('invalid', 'unsupported_version', 'conflict', 'stale_preview', 'storage_failed') }),
] };
const rollback = object({ schema_version: version, import_id: uuid,
  rollback_snapshot_id: uuid, expected_local_revision: positive });
const response = { oneOf: [
  object({ schema_version: version, ...binding, operation_id: uuid,
    consent_generation: positive, status: literal('acknowledged'), record: ref('RemoteRecord') }),
  object({ schema_version: version, ...binding, operation_id: uuid,
    consent_generation: positive, status: literal('conflict'), current: ref('RemoteRecord') }),
  object({ schema_version: version, ...binding, operation_id: uuid,
    consent_generation: positive, status: literal('rejected'), reason: choice('wrong_account',
      'account_unavailable', 'stale_epoch', 'consent_required', 'stale_generation',
      'history_not_enrolled', 'idempotency_conflict', 'deleted', 'immutable',
      'invalid_record', 'invalid_base', 'quota_exceeded', 'unsupported_version') }),
] };

export default {
  $schema: 'http://json-schema.org/draft-07/schema#',
  title: 'AccountSyncCatalog',
  description: 'MW-OUATH-1 v1 catalog. Definitions are wire documents; the catalog is not an endpoint.',
  ...object({ record: ref('RemoteRecord'), outbox: ref('OutboxOperation'),
    consent: ref('ConsentState'), device: ref('DeviceSyncState'), response: ref('SyncResponse'), backup: ref('PortableBackup'),
    import_preview: ref('ImportPreview'), import_receipt: ref('ImportReceipt'),
    rollback: ref('RollbackRequest'), cleanup_request: ref('AccountCleanupRequest'),
    cleanup_receipt: ref('AccountCleanupReceipt') }),
  definitions: {
    PortableModel: model, PortableCleanup: cleanup, PortablePolish: polish, PortableSetting: setting,
    PortableDictionaryEntry: dictionary, PortableTranscript: transcript,
    RemoteRecord: { oneOf: Object.keys(data).map(record) },
    OutboxOperation: { oneOf: Object.keys(data).map(operation) },
    ConsentState: consent, DeviceSyncState: device, SyncResponse: response,
    ExportSetting: exportRecord('settings'), ExportDictionaryEntry: exportRecord('dictionary'),
    PortableBackup: backup, ImportPreview: importPreview, ImportReceipt: importReceipt,
    RollbackRequest: rollback, AccountCleanupRequest: cleanupRequest,
    AccountCleanupReceipt: cleanupReceipt,
  },
};
