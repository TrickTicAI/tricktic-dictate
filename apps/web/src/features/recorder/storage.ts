import type { CaptureState, RecordingMetadata } from '@metrix-writer/contracts';
import { PRODUCT_NAME } from '../../product';

export type LocalSession = RecordingMetadata & { chunk_count: number };
export interface RecordingStore {
  create(session: LocalSession): Promise<void>;
  append(id: string, blob: Blob, duration: number): Promise<void>;
  finish(id: string, state: CaptureState, duration: number): Promise<void>;
  get(id: string): Promise<LocalSession | undefined>;
  list(limit?: number): Promise<LocalSession[]>;
  export(id: string): Promise<Blob>;
  delete(id: string): Promise<void>;
}

// Audio and its byte counter commit together. No transcript/provider operation
// has access to a deletion path in this module.
export class IndexedRecordingStore implements RecordingStore {
  private database?: Promise<IDBDatabase>;
  constructor(private name = 'metrix-writer-recordings') {}

  private open() {
    return this.database ??= new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open(this.name, 1);
      request.onupgradeneeded = () => {
        const sessions = request.result.createObjectStore('sessions', { keyPath: 'id' });
        sessions.createIndex('created', 'created_at_ms');
        request.result.createObjectStore('chunks', { keyPath: ['id', 'index'] });
      };
      request.onsuccess = () => {
        request.result.onversionchange = () => { request.result.close(); this.database = undefined; };
        resolve(request.result);
      };
      request.onerror = () => { this.database = undefined; reject(request.error); };
      request.onblocked = () => { this.database = undefined; reject(new Error(`Close other ${PRODUCT_NAME} tabs to open recording storage.`)); };
    });
  }

  private async write(run: (tx: IDBTransaction) => void) {
    const db = await this.open();
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction(['sessions', 'chunks'], 'readwrite', { durability: 'strict' });
      tx.oncomplete = () => resolve();
      tx.onabort = () => reject(tx.error ?? new Error('Recording storage transaction aborted.'));
      tx.onerror = () => {}; // onabort reports the final transaction outcome.
      try { run(tx); } catch (error) { tx.abort(); reject(error); }
    });
  }

  create(session: LocalSession) {
    return this.write((tx) => { tx.objectStore('sessions').add(session); });
  }

  append(id: string, blob: Blob, duration: number) {
    return this.write((tx) => {
      const sessions = tx.objectStore('sessions');
      const request = sessions.get(id);
      request.onsuccess = () => {
        const session = request.result as LocalSession | undefined;
        if (!session) { tx.abort(); return; }
        tx.objectStore('chunks').add({ id, index: session.chunk_count, blob });
        sessions.put({ ...session, chunk_count: session.chunk_count + 1,
          byte_length: session.byte_length + blob.size, duration_ms: duration });
      };
    });
  }

  finish(id: string, state: CaptureState, duration: number) {
    return this.write((tx) => {
      const sessions = tx.objectStore('sessions');
      const request = sessions.get(id);
      request.onsuccess = () => {
        if (request.result) sessions.put({ ...request.result, capture_state: state, duration_ms: duration });
      };
    });
  }

  async get(id: string) {
    const db = await this.open();
    return new Promise<LocalSession | undefined>((resolve, reject) => {
      const tx = db.transaction('sessions');
      const read = tx.objectStore('sessions').get(id);
      tx.oncomplete = () => resolve(read.result as LocalSession | undefined);
      tx.onabort = () => reject(tx.error);
    });
  }

  async list(limit = 100) {
    const db = await this.open();
    return new Promise<LocalSession[]>((resolve, reject) => {
      const result: LocalSession[] = [];
      const tx = db.transaction('sessions');
      const cursor = tx.objectStore('sessions').index('created').openCursor(null, 'prev');
      cursor.onsuccess = () => {
        if (cursor.result && result.length < Math.max(1, limit)) {
          result.push(cursor.result.value as LocalSession);
          cursor.result.continue();
        }
      };
      tx.oncomplete = () => resolve(result);
      tx.onabort = () => reject(tx.error);
    });
  }

  async export(id: string) {
    const db = await this.open();
    return new Promise<Blob>((resolve, reject) => {
      const tx = db.transaction(['sessions', 'chunks']);
      const session = tx.objectStore('sessions').get(id);
      const chunks: Blob[] = [];
      const cursor = tx.objectStore('chunks').openCursor(IDBKeyRange.bound([id, 0], [id, Number.MAX_SAFE_INTEGER]));
      cursor.onsuccess = () => {
        if (cursor.result) { chunks.push(cursor.result.value.blob); cursor.result.continue(); }
      };
      tx.oncomplete = () => {
        if (!session.result) { reject(new Error('Recording no longer exists.')); return; }
        resolve(new Blob(chunks, { type: (session.result as LocalSession).mime_type }));
      };
      tx.onabort = () => reject(tx.error);
    });
  }

  delete(id: string) {
    return this.write((tx) => {
      tx.objectStore('sessions').delete(id);
      tx.objectStore('chunks').delete(IDBKeyRange.bound([id, 0], [id, Number.MAX_SAFE_INTEGER]));
    });
  }
}
