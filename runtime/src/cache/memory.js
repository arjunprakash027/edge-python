/*
In-memory cache backend; same shape as `cache/idb.js`. Used when `integrity:false` or as fallback when IDB is unavailable. Methods are synchronous but callers `await` them uniformly — `await sync_value` resolves immediately, so the interface stays interchangeable with `IdbCache` without lying with `async` keywords.
*/

export class MemoryCache {
    constructor() {
        this.cas = new Map(); // hash -> bytes
        this.lockfile = new Map(); // spec -> hash
    }

    open() { /* no-op */ }

    getBytes(hash) {
        return this.cas.get(hash) ?? null;
    }

    putBytes(hash, bytes) {
        this.cas.set(hash, bytes);
    }

    loadLockfile() {
        return new Map(this.lockfile);
    }

    saveLockfile(entries) {
        for (const [k, v] of entries) this.lockfile.set(k, v);
    }

    clear() {
        this.cas.clear();
        this.lockfile.clear();
    }

    setVersion(_version) { /* no-op: nothing to invalidate across sessions */ }

    getVersion() { return null; }
}
