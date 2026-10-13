// The one reference implementation of the cleanup contract lives in
// packages/contracts. A deployment copies that directory next to this service,
// keeping the same relative layout (see README.md).
import { readFileSync } from 'node:fs';
import Ajv from 'ajv';
import addFormats from 'ajv-formats';

const contracts = new URL('../../../../packages/contracts/', import.meta.url);
export { callerError, planCleanup, MAX_AUTH_AGE_SECONDS } from '../../../../packages/contracts/account-sync.cleanup.mjs';
export { paths } from '../../../../packages/contracts/account-sync.firestore.mjs';

const ajv = new Ajv({ strict: true, allErrors: false });
addFormats(ajv);
ajv.addSchema(JSON.parse(readFileSync(new URL('account-sync.schema.json', contracts), 'utf8')), 'account-sync');
export const validCleanupRequest = ajv.compile({ $ref: 'account-sync#/definitions/AccountCleanupRequest' });
export const validOperationId = id => typeof id === 'string' &&
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id);
