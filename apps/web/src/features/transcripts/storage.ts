import type { Job, TranscriptionRequest } from '@metrix-writer/contracts';
import type { ModeProfile } from '../modes/api';
export interface Attempt { id: string; request: TranscriptionRequest; job?: Job; history?: Job[]; capturedMode?: ModeProfile }
const database = () => new Promise<IDBDatabase>((resolve, reject) => {
  const request = indexedDB.open('metrix-writer-transcription', 1);
  request.onupgradeneeded = () => request.result.createObjectStore('attempts', { keyPath: 'id' });
  request.onsuccess = () => resolve(request.result);
  request.onerror = () => reject(new Error('Could not open saved transcription requests. Allow browser storage before retrying.'));
});
export async function attempts(): Promise<Attempt[]> {
  const db = await database();
  try { return await new Promise((resolve, reject) => {
    const read = db.transaction('attempts').objectStore('attempts').getAll();
    read.onsuccess = () => resolve(read.result as Attempt[]);
    read.onerror = () => reject(read.error);
  }); } finally { db.close(); }
}
/** Removes one attempt after its recording has been discarded on purpose. */
export async function deleteAttempt(id: string) {
  const db = await database();
  try { await new Promise<void>((resolve, reject) => {
    const tx = db.transaction('attempts', 'readwrite');
    tx.objectStore('attempts').delete(id);
    tx.oncomplete = () => resolve();
    tx.onabort = tx.onerror = () => reject(new Error('Could not remove the saved transcription request from this browser.'));
  }); } finally { db.close(); }
}
export async function saveAttempt(attempt: Attempt) {
  const db = await database();
  try { await new Promise<void>((resolve, reject) => {
    const tx = db.transaction('attempts', 'readwrite');
    tx.objectStore('attempts').put(attempt);
    tx.oncomplete = () => resolve();
    tx.onabort = tx.onerror = () => reject(new Error('Could not save the transcription request in this browser. Audio has not been deleted.'));
  }); } finally { db.close(); }
}
