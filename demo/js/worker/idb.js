/* IndexedDB persistence for the worker:
   - `cas`      content-addressable store, hash -> bytes (Uint8Array)
   - `lockfile` spec -> hash, auto-companion of the user's packages.json
   Open lazily — scripts without imports never touch IDB. */

const IDB_NAME = 'edgepython';
const IDB_VER = 1;
let idbPromise = null;

export function openIdb() {
    if (idbPromise) return idbPromise;
    idbPromise = new Promise((resolve, reject) => {
        const req = self.indexedDB.open(IDB_NAME, IDB_VER);
        req.onupgradeneeded = () => {
            const db = req.result;
            if (!db.objectStoreNames.contains('cas')) db.createObjectStore('cas');
            if (!db.objectStoreNames.contains('lockfile')) db.createObjectStore('lockfile');
        };
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(req.error);
    });
    return idbPromise;
}

const tx = (db, store, mode) => db.transaction(store, mode).objectStore(store);

export const idbGet = async (store, key) => {
    const db = await openIdb();
    return new Promise((res, rej) => {
        const r = tx(db, store, 'readonly').get(key);
        r.onsuccess = () => res(r.result);
        r.onerror = () => rej(r.error);
    });
};

export const idbPut = async (store, key, value) => {
    const db = await openIdb();
    return new Promise((res, rej) => {
        const r = tx(db, store, 'readwrite').put(value, key);
        r.onsuccess = () => res();
        r.onerror = () => rej(r.error);
    });
};

export const idbClear = async (store) => {
    const db = await openIdb();
    return new Promise((res, rej) => {
        const r = tx(db, store, 'readwrite').clear();
        r.onsuccess = () => res();
        r.onerror = () => rej(r.error);
    });
};

// Stream every entry in `store` to `onEntry(key, value)`; resolves when done.
export const idbCursor = async (store, onEntry) => {
    const db = await openIdb();
    return new Promise((res, rej) => {
        const cur = tx(db, store, 'readonly').openCursor();
        cur.onsuccess = () => {
            const c = cur.result;
            if (!c) return res();
            onEntry(c.key, c.value);
            c.continue();
        };
        cur.onerror = () => rej(cur.error);
    });
};

// Bulk-put an iterable of [key, value] entries inside one transaction.
export const idbPutAll = async (store, entries) => {
    const db = await openIdb();
    const s = tx(db, store, 'readwrite');
    for (const [k, v] of entries) s.put(v, k);
};
