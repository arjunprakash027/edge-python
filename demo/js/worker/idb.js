/* 
IDB persistence: `cas` (hash->bytes), `lockfile` (spec->hash). Opened lazily so import-free scripts skip IDB entirely. 
*/

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
const reqDone = (req) => new Promise((res, rej) => {
    req.onsuccess = () => res(req.result);
    req.onerror = () => rej(req.error);
});

export const idbGet = async (store, key) => reqDone(tx(await openIdb(), store, 'readonly').get(key));
export const idbPut = async (store, key, value) => reqDone(tx(await openIdb(), store, 'readwrite').put(value, key));
export const idbClear = async (store) => reqDone(tx(await openIdb(), store, 'readwrite').clear());

// Stream every entry in `store` to `onEntry(key, value)`; resolves when done.
export const idbCursor = async (store, onEntry) => {
    const db = await openIdb();
    return new Promise((res, rej) => {
        const r = tx(db, store, 'readonly').openCursor();
        r.onsuccess = () => {
            const c = r.result;
            if (!c) return res();
            onEntry(c.key, c.value);
            c.continue();
        };
        r.onerror = () => rej(r.error);
    });
};

// Bulk-put an iterable of [key, value] entries inside one transaction.
export const idbPutAll = async (store, entries) => {
    const s = tx(await openIdb(), store, 'readwrite');
    for (const [k, v] of entries) s.put(v, k);
};
