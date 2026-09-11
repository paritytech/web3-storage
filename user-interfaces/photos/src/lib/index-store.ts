// SPDX-License-Identifier: GPL-3.0-only
//
// Per-drive persistence for the client-maintained `LocalIndex`, so a reload
// needn't re-download the whole drive to recompute its anchored root. On load the
// persisted index is revalidated against the on-chain anchor (see `album.state.ts`),
// so a stale snapshot is harmless — it just falls back to a provider recompute.
//
// IndexedDB (not localStorage): a large photo library's entry set can exceed
// localStorage's ~5MB budget, and binary roots serialize awkwardly there. A tiny
// hand-rolled promise wrapper keeps the app dependency-free.

import { LocalIndex, type SerializedIndexEntry } from '@/lib/local-index'

const DB_NAME = 'photos'
const DB_VERSION = 1
const STORE = 'drive-index'

function hasIndexedDb(): boolean {
  return typeof indexedDB !== 'undefined'
}

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION)
    request.onupgradeneeded = () => {
      const db = request.result
      if (!db.objectStoreNames.contains(STORE)) db.createObjectStore(STORE)
    }
    request.onsuccess = () => resolve(request.result)
    request.onerror = () => reject(request.error)
  })
}

/** Run `work` against the store in one transaction, resolving when it commits. */
async function withStore<T>(
  mode: IDBTransactionMode,
  work: (store: IDBObjectStore) => IDBRequest<T>,
): Promise<T> {
  const db = await openDb()
  try {
    return await new Promise<T>((resolve, reject) => {
      const tx = db.transaction(STORE, mode)
      const request = work(tx.objectStore(STORE))
      tx.oncomplete = () => resolve(request.result)
      tx.onabort = tx.onerror = () => reject(tx.error)
    })
  } finally {
    db.close()
  }
}

/**
 * Load the persisted index for `key` (`${network}:${driveId}`), or null if absent
 * or IndexedDB is unavailable. Never throws — a persistence miss degrades to a
 * provider recompute, so failures are swallowed to a null.
 */
export async function loadIndex(key: string): Promise<LocalIndex | null> {
  if (!hasIndexedDb()) return null
  try {
    const data = await withStore<SerializedIndexEntry[] | undefined>('readonly', (store) => store.get(key))
    return data ? LocalIndex.fromJSON(data) : null
  } catch {
    return null
  }
}

/** Persist `index` under `key`. Never throws — anchoring must not fail on a cache write. */
export async function saveIndex(key: string, index: LocalIndex): Promise<void> {
  if (!hasIndexedDb()) return
  try {
    await withStore('readwrite', (store) => store.put(index.toJSON(), key))
  } catch {
    // A failed persist just means the next reload recomputes from the provider.
  }
}

/** Drop the persisted index for `key`. Never throws. */
export async function clearIndex(key: string): Promise<void> {
  if (!hasIndexedDb()) return
  try {
    await withStore('readwrite', (store) => store.delete(key))
  } catch {
    // Nothing to clean up on failure.
  }
}
