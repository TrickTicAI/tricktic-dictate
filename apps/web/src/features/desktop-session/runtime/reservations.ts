/** A tiny receipt per reserved recording ID prevents reopening an evicted start
 * after cancellation or document recreation, including permission-only sessions. */
export async function reserveSession(id: string): Promise<boolean> {
  const db = await new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open('metrix-writer-desktop-receipts', 1);
    request.onupgradeneeded = () => request.result.createObjectStore('starts');
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  try {
    return await new Promise<boolean>((resolve, reject) => {
      const tx = db.transaction('starts', 'readwrite', { durability: 'strict' });
      const store = tx.objectStore('starts');
      let added = false;
      const read = store.get(id);
      read.onsuccess = () => { if (!read.result) { store.add(true, id); added = true; } };
      tx.oncomplete = () => resolve(added);
      tx.onabort = () => reject(tx.error);
    });
  } finally { db.close(); }
}
