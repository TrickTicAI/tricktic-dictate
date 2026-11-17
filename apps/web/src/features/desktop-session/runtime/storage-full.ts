import { ApiFailure } from '../../transcripts/api';

/**
 * Saved data reached its storage limit. The shell rejects a refused native
 * start or import with exactly this code, and the HTTP API answers 507; the
 * session reports it as its error code so every surface can say so plainly.
 */
export const STORAGE_FULL = 'storage_full';
export const isStorageFull = (error: unknown) => error === STORAGE_FULL || error instanceof ApiFailure && error.status === 507;
/** Where to free space, as the Settings sidebar names it. */
export const STORAGE_PLACE = 'Settings › Privacy & data › Storage';
