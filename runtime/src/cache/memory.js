/*
In-memory cache backend; same shape as `cache/idb.js`. Used when `integrity:false` or as fallback when IDB is unavailable.
*/

export class MemoryCache {
    constructor() {
        this.cas = new Map(); // hash -> bytes
        this.lockfile = new Map(); // spec -> hash
    }

    async open() { /* no-op */ }

    async getBytes(hash) {
        return this.cas.get(hash) ?? null;
    }

    async putBytes(hash, bytes) {
        this.cas.set(hash, bytes);
    }

    async loadLockfile() {
        return new Map(this.lockfile);
    }

    async saveLockfile(entries) {
        for (const [k, v] of entries) this.lockfile.set(k, v);
    }

    async clear() {
        this.cas.clear();
        this.lockfile.clear();
    }

    async setVersion(_version) { /* no-op: nothing to invalidate across sessions */ }

    async getVersion() { return null; }
}
