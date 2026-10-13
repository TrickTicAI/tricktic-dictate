/* Generated from account-sync.schema.mjs. Run node generate.mjs --write. */

/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "RemoteRecord".
 */
export type RemoteRecord =
  | {
      schema_version: 1;
      id: string;
      origin_device_id: string;
      category: "settings";
      revision: number;
      content:
        | {
            state: "live";
            data: PortableSetting;
          }
        | {
            state: "deleted";
          };
    }
  | {
      schema_version: 1;
      id: string;
      origin_device_id: string;
      category: "dictionary";
      revision: number;
      content:
        | {
            state: "live";
            data: PortableDictionaryEntry;
          }
        | {
            state: "deleted";
          };
    }
  | {
      schema_version: 1;
      id: string;
      origin_device_id: string;
      category: "transcripts";
      revision: number;
      content:
        | {
            state: "live";
            data: PortableTranscript;
          }
        | {
            state: "deleted";
          };
    };
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "PortableSetting".
 */
export type PortableSetting =
  | {
      kind: "setting";
      key: "ui_language";
      value: "en" | "he";
    }
  | {
      kind: "setting";
      key: "input_language";
      value: "en" | "he" | "mixed";
    }
  | {
      kind: "setting";
      key: "output_language";
      value:
        | {
            mode: "preserve";
          }
        | {
            mode: "translate";
            language: "en" | "he";
          };
    }
  | {
      kind: "setting";
      key: "transcription_model";
      value: PortableModel;
    }
  | {
      kind: "setting";
      key: "dictation_mode";
      value: "regular" | "super" | "command";
    }
  | {
      kind: "setting";
      key: "cleanup";
      value: PortableCleanup;
    }
  | {
      kind: "setting";
      key: "delivery";
      value: "raw" | "dictionary" | "cleaned";
    }
  | {
      kind: "setting";
      key: "theme";
      value: "system" | "light" | "dark";
    }
  | {
      kind: "setting";
      key: "polish_instructions";
      value: PortablePolish;
    };
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "PortableCleanup".
 */
export type PortableCleanup =
  | {
      mode: "off";
    }
  | {
      mode: "enabled";
      selection: PortableModel;
      reasoning: "provider_default" | "off" | "minimal" | "low" | "medium" | "high";
    };
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "PortableDictionaryEntry".
 */
export type PortableDictionaryEntry =
  | {
      kind: "hint";
      enabled: boolean;
      text: string;
    }
  | {
      kind: "correction";
      enabled: boolean;
      from: string;
      to: string;
    }
  | {
      kind: "snippet";
      enabled: boolean;
      phrase: string;
      text: string;
    };
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "PortableTranscript".
 */
export type PortableTranscript =
  | {
      kind: "original";
      attempt_id: string;
      recorded_at_ms: number;
      input_language: "en" | "he" | "mixed";
      output_language:
        | {
            mode: "preserve";
          }
        | {
            mode: "translate";
            language: "en" | "he";
          };
      selection: PortableModel;
      text: string;
    }
  | {
      kind: "corrected";
      original_record_id: string;
      parent_record_id: string;
      created_at_ms: number;
      text: string;
    }
  | {
      kind: "cleaned";
      original_record_id: string;
      parent_record_id: string;
      created_at_ms: number;
      text: string;
    }
  | {
      kind: "edited";
      original_record_id: string;
      parent_record_id: string;
      created_at_ms: number;
      text: string;
    };
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "OutboxOperation".
 */
export type OutboxOperation =
  | {
      schema_version: 1;
      uid: string;
      account_epoch: number;
      consent_generation: number;
      category: "settings";
      operation_id: string;
      device_id: string;
      record_id: string;
      origin_device_id: string;
      base_revision: number;
      content:
        | {
            state: "live";
            data: PortableSetting;
          }
        | {
            state: "deleted";
          };
    }
  | {
      schema_version: 1;
      uid: string;
      account_epoch: number;
      consent_generation: number;
      category: "dictionary";
      operation_id: string;
      device_id: string;
      record_id: string;
      origin_device_id: string;
      base_revision: number;
      content:
        | {
            state: "live";
            data: PortableDictionaryEntry;
          }
        | {
            state: "deleted";
          };
    }
  | {
      schema_version: 1;
      uid: string;
      account_epoch: number;
      consent_generation: number;
      category: "transcripts";
      operation_id: string;
      device_id: string;
      record_id: string;
      origin_device_id: string;
      base_revision: number;
      history_enrollment_id: string;
      content:
        | {
            state: "live";
            data: PortableTranscript;
          }
        | {
            state: "deleted";
          };
    };
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "SyncResponse".
 */
export type SyncResponse =
  | {
      schema_version: 1;
      uid: string;
      account_epoch: number;
      operation_id: string;
      consent_generation: number;
      status: "acknowledged";
      record: RemoteRecord;
    }
  | {
      schema_version: 1;
      uid: string;
      account_epoch: number;
      operation_id: string;
      consent_generation: number;
      status: "conflict";
      current: RemoteRecord;
    }
  | {
      schema_version: 1;
      uid: string;
      account_epoch: number;
      operation_id: string;
      consent_generation: number;
      status: "rejected";
      reason:
        | "wrong_account"
        | "account_unavailable"
        | "stale_epoch"
        | "consent_required"
        | "stale_generation"
        | "history_not_enrolled"
        | "idempotency_conflict"
        | "deleted"
        | "immutable"
        | "invalid_record"
        | "invalid_base"
        | "quota_exceeded"
        | "unsupported_version";
    };
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "ImportReceipt".
 */
export type ImportReceipt =
  | {
      schema_version: 1;
      import_id: string;
      state: "applied";
      local_revision: number;
      rollback_snapshot_id: string;
    }
  | {
      schema_version: 1;
      import_id: string;
      state: "not_applied";
      reason: "invalid" | "unsupported_version" | "conflict" | "stale_preview" | "storage_failed";
    };
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "AccountCleanupRequest".
 */
export type AccountCleanupRequest =
  | {
      schema_version: 1;
      uid: string;
      account_epoch: number;
      operation_id: string;
      expected_consent_revision: number;
      scope: {
        kind: "categories";
        /**
         * @minItems 1
         * @maxItems 3
         */
        categories:
          | ["settings" | "dictionary" | "transcripts"]
          | ["settings" | "dictionary" | "transcripts", "settings" | "dictionary" | "transcripts"]
          | [
              "settings" | "dictionary" | "transcripts",
              "settings" | "dictionary" | "transcripts",
              "settings" | "dictionary" | "transcripts"
            ];
      };
      effect: "cloud_only" | "everywhere";
    }
  | {
      schema_version: 1;
      uid: string;
      account_epoch: number;
      operation_id: string;
      expected_consent_revision: number;
      scope: {
        kind: "account";
      };
      effect: "cloud_only";
    };
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "AccountCleanupReceipt".
 */
export type AccountCleanupReceipt =
  | {
      schema_version: 1;
      uid: string;
      account_epoch: number;
      operation_id: string;
      job_id: string;
      state: "pending" | "running" | "failed_retryable" | "completed";
      fenced_account_epoch: number;
      fenced_consent: ConsentState;
      scope: {
        kind: "categories";
        /**
         * @minItems 1
         * @maxItems 3
         */
        categories:
          | ["settings" | "dictionary" | "transcripts"]
          | ["settings" | "dictionary" | "transcripts", "settings" | "dictionary" | "transcripts"]
          | [
              "settings" | "dictionary" | "transcripts",
              "settings" | "dictionary" | "transcripts",
              "settings" | "dictionary" | "transcripts"
            ];
      };
      effect: "cloud_only" | "everywhere";
    }
  | {
      schema_version: 1;
      uid: string;
      account_epoch: number;
      operation_id: string;
      job_id: string;
      state: "pending" | "running" | "failed_retryable" | "completed";
      fenced_account_epoch: number;
      fenced_consent: ConsentState;
      scope: {
        kind: "account";
      };
      effect: "cloud_only";
    };

/**
 * MW-OUATH-1 v1 catalog. Definitions are wire documents; the catalog is not an endpoint.
 */
export interface AccountSyncCatalog {
  record: RemoteRecord;
  outbox: OutboxOperation;
  consent: ConsentState;
  device: DeviceSyncState;
  response: SyncResponse;
  backup: PortableBackup;
  import_preview: ImportPreview;
  import_receipt: ImportReceipt;
  rollback: RollbackRequest;
  cleanup_request: AccountCleanupRequest;
  cleanup_receipt: AccountCleanupReceipt;
}
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "PortableModel".
 */
export interface PortableModel {
  provider: "openai" | "gemini" | "grok" | "openrouter" | "local";
  model: string;
}
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "PortablePolish".
 */
export interface PortablePolish {
  instructions: string;
  formatting: {
    rewrite: "light" | "polished" | "concise";
    layout: "automatic" | "single_line";
    lists: boolean;
    remove_fillers: boolean;
    self_corrections: boolean;
    spoken_punctuation: boolean;
  };
}
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "ConsentState".
 */
export interface ConsentState {
  schema_version: 1;
  uid: string;
  account_epoch: number;
  revision: number;
  settings: {
    enabled: boolean;
    generation: number;
  };
  dictionary: {
    enabled: boolean;
    generation: number;
  };
  transcripts:
    | {
        enabled: false;
        generation: number;
        history: {
          mode: "off";
        };
      }
    | {
        enabled: true;
        generation: number;
        history:
          | {
              mode: "future_only";
              enrollment_id: string;
            }
          | {
              mode: "include_existing";
              enrollment_id: string;
            };
      };
}
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "DeviceSyncState".
 */
export interface DeviceSyncState {
  schema_version: 1;
  uid: string;
  account_epoch: number;
  device_id: string;
  reconciled_generations: {
    settings: number;
    dictionary: number;
    transcripts: number;
  };
}
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "PortableBackup".
 */
export interface PortableBackup {
  format: "metrix-writer-portable";
  format_version: 1;
  export_id: string;
  created_at_ms: number;
  /**
   * @maxItems 9
   */
  settings:
    | []
    | [ExportSetting]
    | [ExportSetting, ExportSetting]
    | [ExportSetting, ExportSetting, ExportSetting]
    | [ExportSetting, ExportSetting, ExportSetting, ExportSetting]
    | [ExportSetting, ExportSetting, ExportSetting, ExportSetting, ExportSetting]
    | [ExportSetting, ExportSetting, ExportSetting, ExportSetting, ExportSetting, ExportSetting]
    | [ExportSetting, ExportSetting, ExportSetting, ExportSetting, ExportSetting, ExportSetting, ExportSetting]
    | [
        ExportSetting,
        ExportSetting,
        ExportSetting,
        ExportSetting,
        ExportSetting,
        ExportSetting,
        ExportSetting,
        ExportSetting
      ]
    | [
        ExportSetting,
        ExportSetting,
        ExportSetting,
        ExportSetting,
        ExportSetting,
        ExportSetting,
        ExportSetting,
        ExportSetting,
        ExportSetting
      ];
  /**
   * @maxItems 600
   */
  dictionary: ExportDictionaryEntry[];
}
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "ExportSetting".
 */
export interface ExportSetting {
  schema_version: 1;
  id: string;
  origin_device_id: string;
  category: "settings";
  data: PortableSetting;
}
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "ExportDictionaryEntry".
 */
export interface ExportDictionaryEntry {
  schema_version: 1;
  id: string;
  origin_device_id: string;
  category: "dictionary";
  data: PortableDictionaryEntry;
}
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "ImportPreview".
 */
export interface ImportPreview {
  schema_version: 1;
  import_id: string;
  export_id: string;
  mode: "merge" | "replace_selected";
  /**
   * @minItems 1
   * @maxItems 2
   */
  categories: ["settings" | "dictionary"] | ["settings" | "dictionary", "settings" | "dictionary"];
  disposition: "restore_once" | "restore_and_sync";
  expected_local_revision: number;
  /**
   * @maxItems 608
   */
  apply_ids: string[];
  /**
   * @maxItems 608
   */
  preserve_ids: string[];
  /**
   * @maxItems 608
   */
  conflict_ids: string[];
}
/**
 * This interface was referenced by `AccountSyncCatalog`'s JSON-Schema
 * via the `definition` "RollbackRequest".
 */
export interface RollbackRequest {
  schema_version: 1;
  import_id: string;
  rollback_snapshot_id: string;
  expected_local_revision: number;
}
