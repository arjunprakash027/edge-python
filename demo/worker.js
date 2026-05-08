const source_size = 1 << 20; // 1 MiB limit.

let wasmModule = null;

/* Per-spec source cache. Keyed by the canonical spec the parser will see
   (so a quoted relative `./helpers.py` inside `./lib/a.py` is stored under
   `./lib/helpers.py`, matching the bridge resolver's join_relative output).
   Survives across runs in worker scope; cleared by `clearCache` and by
   IDB-CAS invalidation. Feeds two consumers:
     • `register_code_module` for `.py` files → REGISTRY (parser).
     • `js_fetch_bytes` for `packages.json` and integrity-checked URLs. */
const fetchedSources = new Map();

/* IndexedDB persistence: lockfile (spec → sha256-hex) and CAS (hash → bytes).
   Lockfile is the auto-generated companion of the user's packages.json; CAS
   holds raw bytes content-addressed so two URLs serving identical content
   share storage and the integrity primitive doubles as cache validation.

   Open lazily — workers can run scripts without imports and never need IDB. */
const IDB_NAME = 'edgepython';
const IDB_VER = 1;
let idbPromise = null;

function openIdb() {
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
const idbGet = async (store, key) => {
    const db = await openIdb();
    return new Promise((res, rej) => {
        const r = tx(db, store, 'readonly').get(key);
        r.onsuccess = () => res(r.result);
        r.onerror = () => rej(r.error);
    });
};
const idbPut = async (store, key, value) => {
    const db = await openIdb();
    return new Promise((res, rej) => {
        const r = tx(db, store, 'readwrite').put(value, key);
        r.onsuccess = () => res();
        r.onerror = () => rej(r.error);
    });
};
const idbClear = async (store) => {
    const db = await openIdb();
    return new Promise((res, rej) => {
        const r = tx(db, store, 'readwrite').clear();
        r.onsuccess = () => res();
        r.onerror = () => rej(r.error);
    });
};

const sha256Hex = async (bytes) => {
    const digest = await crypto.subtle.digest('SHA-256', bytes);
    return [...new Uint8Array(digest)].map(b => b.toString(16).padStart(2, '0')).join('');
};

/* Spec-string helpers. Kept in lockstep with the Rust bridge's
   modules::packages::manifest helpers so a transitively-imported module's
   relative path canonicalizes the same way on both sides. */
const dirOf = (spec) => {
    const i = spec.lastIndexOf('/');
    return i === -1 ? '' : spec.slice(0, i + 1);
};

const parentDir = (dir) => {
    if (dir === '') return null;
    const trimmed = dir.endsWith('/') ? dir.slice(0, -1) : dir;
    const sch = trimmed.indexOf('://');
    if (sch !== -1 && !trimmed.slice(sch + 3).includes('/')) return null;
    const i = trimmed.lastIndexOf('/');
    if (i === -1) return '';
    return trimmed.slice(0, i + 1);
};

const joinRel = (base, target) => {
    if (target.includes('://') || target.startsWith('/')) return target;
    if (base.includes('://')) return new URL(target, base).toString();
    let b = base, t = target;
    while (t.startsWith('../')) {
        const p = parentDir(b); b = p == null ? '' : p;
        t = t.slice(3);
    }
    if (t === '..') { const p = parentDir(b); return p == null ? '' : p; }
    if (t === '.' || t === '') return b;
    if (b !== '') {
        while (t.startsWith('./')) t = t.slice(2);
        if (!b.endsWith('/')) b += '/';
    }
    return b + t;
};

/* Match the Rust bridge's `scan_string_imports` — collect every quoted spec
   appearing after a `from`. Used to drive BFS pre-fetch without involving
   the WASM compiler for transitive specs. */
const scanStringImports = (src) => {
    const out = [];
    for (const line of src.split('\n')) {
        const t = line.trimStart();
        if (!t.startsWith('from ')) continue;
        const rest = t.slice(5).trimStart();
        if (rest[0] !== '"') continue;
        const end = rest.indexOf('"', 1);
        if (end > 1) out.push(rest.slice(1, end));
    }
    return out;
};

/* Serve `spec` from CAS if the lockfile knows its hash, else fetch the
   canonicalized URL, hash it, and store. Returns the bytes (Uint8Array) or
   null on 404 — null is fine for opportunistic packages.json siblings.
   Drift detection: if the lockfile entry mismatches the freshly-computed
   hash, throw so the user notices a CDN change. */
async function fetchWithLockfile(spec, lockfile) {
    const expected = lockfile.get(spec);
    if (expected) {
        const cached = await idbGet('cas', expected);
        if (cached) return new Uint8Array(cached);
    }
    let resp;
    try {
        const url = spec.includes('://') ? spec : new URL(spec, self.location.href).toString();
        resp = await fetch(url);
    } catch { return null; }
    if (!resp.ok) return null;
    const bytes = new Uint8Array(await resp.arrayBuffer());
    const hash = await sha256Hex(bytes);
    if (expected && expected !== hash) {
        throw new Error(
            `integrity drift for '${spec}'\n  locked: sha256-${expected}\n  remote: sha256-${hash}`
        );
    }
    await idbPut('cas', hash, bytes);
    lockfile.set(spec, hash);
    return bytes;
}

/* BFS over the dependency graph. Each visited spec contributes:
     • its bytes to fetchedSources (so js_fetch_bytes can serve them),
     • either a register_code_module call (.py) or a recursive queue
       expansion (packages.json),
     • a queued sibling packages.json next to .py files (opportunistic —
       a 404 ends that arm of the search silently).

   The queue holds canonical specs (matching what the bridge resolver will
   look up); transitive relative imports are joined to their importer's dir
   before queuing. */
async function bfsPrefetch(rootSrc, exports, lockfile) {
    const decoder = new TextDecoder();
    const visited = new Set();
    const queue = [];
    // Root script lives at the worker URL's directory; its quoted imports
    // anchor BFS, plus a sibling packages.json so bare-name imports in the
    // entry resolve via walk-up.
    for (const q of scanStringImports(rootSrc)) queue.push(q);
    queue.push('packages.json');

    while (queue.length) {
        const spec = queue.shift();
        if (visited.has(spec)) continue;
        visited.add(spec);

        const bytes = await fetchWithLockfile(spec, lockfile);
        if (!bytes) continue;
        fetchedSources.set(spec, bytes);

        if (spec.endsWith('packages.json')) {
            let parsed;
            try { parsed = JSON.parse(decoder.decode(bytes)); }
            catch { continue; }   // bad JSON surfaces at compile time via the bridge
            const dir = dirOf(spec);
            for (const target of Object.values(parsed.imports || {})) {
                queue.push(joinRel(dir, target));
            }
            if (parsed.extends) {
                const extDir = joinRel(dir, parsed.extends);
                const extDirSlash = extDir.endsWith('/') ? extDir : extDir + '/';
                queue.push(extDirSlash + 'packages.json');
            }
            continue;
        }

        // .py module: hand to the parser via REGISTRY, then expand its own
        // quoted imports + sibling packages.json.
        const specBytes = new TextEncoder().encode(spec);
        const sp = exports.wasm_alloc(specBytes.length);
        const cp = exports.wasm_alloc(bytes.length);
        new Uint8Array(exports.memory.buffer, sp, specBytes.length).set(specBytes);
        new Uint8Array(exports.memory.buffer, cp, bytes.length).set(bytes);
        exports.register_code_module(sp, specBytes.length, cp, bytes.length);

        const dir = dirOf(spec);
        for (const q of scanStringImports(decoder.decode(bytes))) {
            queue.push(joinRel(dir, q));
        }
        queue.push(dir + 'packages.json');
    }
}

const handlers = {
    /* Wipe the per-run source cache, the IDB CAS, and the lockfile. Next
       run() re-fetches every import from the network and rebuilds the
       lockfile from scratch. Useful when the user wants to pick up an
       upstream change that would otherwise be hidden behind cached bytes. */
    clearCache: async () => {
        fetchedSources.clear();
        await idbClear('cas');
        await idbClear('lockfile');
    },

    load: async ({ url, opts }) => {
        try {
            const t0 = performance.now();
            // Compile without instantiating to allow multiple runs from the same module.
            wasmModule = await WebAssembly.compileStreaming(fetch(url, opts));
            self.postMessage({ type: 'ready', ms: performance.now() - t0 });
        } catch (err) {
            self.postMessage({ type: 'error', message: err.message });
        }
    },

    run: async ({ src }) => {
        const srcBytes = new TextEncoder().encode(src);
        if (srcBytes.length > source_size) {
            self.postMessage({ type: 'result', out: `Error: Source exceeds ${source_size} bytes` });
            return;
        }

        // Hydrate the lockfile from IDB into a session-local Map. Mutations
        // made during BFS are persisted only after a successful run, so a
        // mid-run failure doesn't leave a half-written lockfile.
        let lockfile;
        try {
            const db = await openIdb();
            lockfile = new Map();
            await new Promise((res, rej) => {
                const cur = tx(db, 'lockfile', 'readonly').openCursor();
                cur.onsuccess = () => {
                    const c = cur.result;
                    if (!c) return res();
                    lockfile.set(c.key, c.value);
                    c.continue();
                };
                cur.onerror = () => rej(cur.error);
            });
        } catch {
            lockfile = new Map();   // private mode / IDB blocked: in-memory only
        }

        // js_print streams output line-by-line as the VM executes; the
        // shared `exports` reference lets the closures reach the same WASM
        // memory the BFS pre-fetch wrote into. js_call_native is required
        // by the WASM ABI even for code-only modules, so a stub throws.
        let exports;
        const imports = { env: {
            js_print: (ptr, len) => {
                const line = new TextDecoder().decode(
                    new Uint8Array(exports.memory.buffer, ptr, len)
                );
                self.postMessage({ type: 'line', line });
            },
            js_call_native: () => {
                throw new Error('demo worker does not register native modules');
            },
            // The bridge's resolver calls this for two purposes:
            //   1. Verify a `#sha256-...` integrity fragment on a URL import.
            //   2. Walk up looking for `<dir>/packages.json` during bare-name
            //      resolution.
            // Both consume from the same fetchedSources map; a 0 return
            // signals "no bytes for this spec" so walk-up can keep walking.
            js_fetch_bytes: (specPtr, specLen, outLenPtr) => {
                const spec = new TextDecoder().decode(
                    new Uint8Array(exports.memory.buffer, specPtr, specLen)
                );
                const bytes = fetchedSources.get(spec);
                if (bytes === undefined) {
                    new DataView(exports.memory.buffer).setUint32(outLenPtr, 0, true);
                    return 0;
                }
                const ptr = exports.wasm_alloc(bytes.length);
                new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
                new DataView(exports.memory.buffer).setUint32(outLenPtr, bytes.length, true);
                return ptr;
            },
        }};

        try {
            ({ exports } = await WebAssembly.instantiate(wasmModule, imports));
            exports.reset_modules();

            // Stage entry source, then run BFS pre-fetch over the graph.
            const writeSrc = () => {
                new Uint8Array(exports.memory.buffer).set(srcBytes, exports.src_ptr());
            };
            writeSrc();
            await bfsPrefetch(src, exports, lockfile);
            // wasm_alloc may have grown linear memory during BFS, invalidating
            // earlier views. Re-write the source at the (still valid) offset.
            writeSrc();

            const t0 = performance.now();
            const len = exports.run(srcBytes.length);
            const ms = performance.now() - t0;

            const out = new TextDecoder().decode(
                new Uint8Array(exports.memory.buffer, exports.out_ptr(), len)
            );

            // Persist any new lockfile entries collected during this run.
            // Done after a successful run so failed runs don't pin partial
            // hashes the user can't easily revert.
            try {
                const db = await openIdb();
                const store = tx(db, 'lockfile', 'readwrite');
                for (const [k, v] of lockfile) store.put(v, k);
            } catch { /* IDB blocked — accept loss of persistence */ }

            self.postMessage({ type: 'result', out, ms });
        } catch (err) {
            self.postMessage({
                type: 'result',
                out: `Runtime trap: ${err?.message ?? err}`,
                ms: 0,
            });
        }
    },
};

self.onmessage = ({ data }) => handlers[data.type]?.(data);
