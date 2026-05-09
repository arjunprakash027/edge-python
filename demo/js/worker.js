const source_size = 1 << 20; // 1 MiB limit.

let wasmModule = null;

/* Per-spec source cache. Keyed by the canonical spec the parser will see
   (so a quoted relative `./helpers.py` inside `./lib/a.py` is stored under
   `./lib/helpers.py`, matching the bridge resolver's join_relative output).
   Survives across runs in worker scope; cleared by `clearCache` and by
   IDB-CAS invalidation. Feeds two consumers:
     • `register_code_module` for `.py` files -> REGISTRY (parser).
     • `host_fetch_bytes` for `packages.json` and integrity-checked URLs. */
const fetchedSources = new Map();

/* packages.json specs known to 404 — populated as BFS encounters missing
   manifests, consulted before re-enqueuing across runs. Lives in worker
   scope so a Run-button mash doesn't re-probe dead URLs every time.
   Cleared by clearCache. */
const knownMissing = new Set();

/* IndexedDB persistence: lockfile (spec -> sha256-hex) and CAS (hash -> bytes).
   Lockfile is the auto-generated companion of the user's packages.json; CAS
   holds raw bytes content-addressed so two URLs serving identical content
   share storage and the integrity primitive doubles as cache validation.

   Open lazily — workers can run scripts without imports and never need IDB. */
const IDB_NAME = 'edgepython';
const IDB_VER = 1;
let idbPromise = null;
let baseUrl = null;
let entryDir = '';

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
        // spec is the canonical name the parser sees (e.g. './lib/format.py');
        // entryDir is the URL prefix where the project physically lives
        // (e.g. 'runtime/'). Keep them separate: register/lookup uses spec,
        // fetch uses entryDir + spec.
        const path = (spec.includes('://') || spec.startsWith('/')) ? spec : entryDir + spec;
        const url = path.includes('://')
            ? path
            : new URL(path, baseUrl ?? self.location.href).toString();
        resp = await fetch(url);
        } catch (e) {
            console.warn(`fetch failed for '${spec}':`, e);
            return null;
        }
        if (!resp.ok) {
            if (resp.status === 404 && spec.endsWith('packages.json')) {
                knownMissing.add(spec);
            } else {
                console.warn(`${resp.status} for '${spec}' at ${resp.url}`);
            }
            return null;
        }
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
     • its bytes to fetchedSources (so host_fetch_bytes can serve them),
     • either a register_code_module call (.py) or a recursive queue
       expansion (packages.json),
     • a queued sibling packages.json next to .py files (opportunistic —
       a 404 ends that arm of the search silently).

   The queue holds canonical specs (matching what the bridge resolver will
   look up); transitive relative imports are joined to their importer's dir
   before queuing. */
/* Native module dispatch table. JS owns the table; compiler.wasm refers
   to entries by their u32 id (assigned at register_native_module time).
   Each entry holds a guest export function. host_call_native looks up by
   id and routes the call through the universal handle ABI. */
const nativeTable = [];

/* Build the env imports a guest module declares. They translate the
   guest's view (its own pointers in its own linear memory) into the
   compiler's view (compiler memory + handle table) and back. */
function makeGuestEnv(compilerExports) {
    const compMem = () => new Uint8Array(compilerExports.memory.buffer);
    const compView = () => new DataView(compilerExports.memory.buffer);

    return (guestExports) => ({
        edge_op: (op, recv, name_ptr, name_len, argv_ptr, argc, out) => {
            const guestMem = new Uint8Array(guestExports.memory.buffer);
            const guestView = new DataView(guestExports.memory.buffer);
            // Stage name and argv in compiler memory.
            const cName = compilerExports.wasm_alloc(Math.max(1, name_len));
            const cArgv = compilerExports.wasm_alloc(Math.max(4, argc * 4));
            const cOut  = compilerExports.wasm_alloc(4);
            if (name_len) compMem().set(guestMem.subarray(name_ptr, name_ptr + name_len), cName);
            for (let i = 0; i < argc; i++) {
                const h = guestView.getUint32(argv_ptr + i * 4, true);
                compView().setUint32(cArgv + i * 4, h, true);
            }
            const ret = compilerExports.host_edge_op(op, recv, cName, name_len, cArgv, argc, cOut);
            if (ret === 0 && out) {
                const h = compView().getUint32(cOut, true);
                guestView.setUint32(out, h, true);
            }
            return ret;
        },

        edge_encode: (tag, ptr, len) => {
            const guestMem = new Uint8Array(guestExports.memory.buffer);
            const cPtr = compilerExports.wasm_alloc(Math.max(1, len));
            if (len) compMem().set(guestMem.subarray(ptr, ptr + len), cPtr);
            return compilerExports.host_edge_encode(tag, cPtr, len);
        },

        edge_decode: (h, out_tag, dst, dst_max) => {
            const guestView = new DataView(guestExports.memory.buffer);
            const cTag = compilerExports.wasm_alloc(4);
            const cBuf = compilerExports.wasm_alloc(Math.max(1, dst_max));
            const ret = compilerExports.host_edge_decode(h, cTag, cBuf, dst_max);
            // Always write the tag back (caller may inspect it on err too).
            const tag = compView().getUint32(cTag, true);
            guestView.setUint32(out_tag, tag, true);
            if (ret > 0) {
                const guestMem = new Uint8Array(guestExports.memory.buffer);
                guestMem.set(compMem().subarray(cBuf, cBuf + ret), dst);
            }
            return ret;
        },

        edge_release: (h) => compilerExports.host_edge_release(h),

        edge_throw: (kind, msg_ptr, msg_len) => {
            const guestMem = new Uint8Array(guestExports.memory.buffer);
            const cMsg = compilerExports.wasm_alloc(Math.max(1, msg_len));
            if (msg_len) compMem().set(guestMem.subarray(msg_ptr, msg_ptr + msg_len), cMsg);
            compilerExports.host_edge_throw(kind, cMsg, msg_len);
        },

        edge_take_error: (out_kind, dst, dst_max) => {
            const guestView = new DataView(guestExports.memory.buffer);
            const cKind = compilerExports.wasm_alloc(4);
            const cBuf = compilerExports.wasm_alloc(Math.max(1, dst_max));
            const ret = compilerExports.host_edge_take_error(cKind, cBuf, dst_max);
            if (ret >= 0) {
                guestView.setUint32(out_kind, compView().getUint32(cKind, true), true);
                if (ret > 0) {
                    const guestMem = new Uint8Array(guestExports.memory.buffer);
                    guestMem.set(compMem().subarray(cBuf, cBuf + ret), dst);
                }
            }
            return ret;
        },
    });
}

/* Instantiate a guest .wasm module against the universal ABI. Returns
   the list of callable export names so the host can register them with
   compiler.wasm. The guest is required to export `__edge_alloc(size) ->
   ptr` for the host to stage argv arrays in guest memory. */
async function instantiateNativeModule(spec, bytes, compilerExports) {
    const envFactory = makeGuestEnv(compilerExports);
    let guest;
    const env = envFactory({ get memory() { return guest.exports.memory; } });
    const { instance } = await WebAssembly.instantiate(bytes, { env });
    guest = instance;

    if (typeof instance.exports.__edge_alloc !== 'function') {
        throw new Error(
            `native module '${spec}' must export '__edge_alloc(size: u32) -> *mut u8';` +
            ` see /reference/wasm-abi for the contract`
        );
    }

    // Discover callable exports. Skip the ABI plumbing (memory,
    // __edge_alloc) and toolchain post-link helpers (__data_end, etc.).
    const SKIP = new Set(['memory', '__edge_alloc']);
    const names = [];
    const fns = [];
    for (const k of Object.keys(instance.exports)) {
        if (SKIP.has(k) || k.startsWith('__')) continue;
        if (typeof instance.exports[k] !== 'function') continue;
        names.push(k);
        const fn = instance.exports[k];
        // Annotate the function with its guest's allocator so host_call_native
        // can stage argv without re-resolving the instance every call.
        fn.__edge_alloc = instance.exports.__edge_alloc;
        fn.__edge_memory = instance.exports.memory;
        fns.push(fn);
    }
    return { names, fns };
}

async function bfsPrefetch(rootSrc, exports, lockfile) {
    const decoder = new TextDecoder();
    const visited = new Set();
    const queue = [];
    // Root script's quoted imports anchor BFS in their canonical form
    // (no entryDir prefix — the parser sees them that way too). The URL
    // prefix for physical fetch is applied inside fetchWithLockfile.
    for (const q of scanStringImports(rootSrc)) queue.push(q);
    if (!knownMissing.has('packages.json')) queue.push('packages.json');

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

        if (spec.endsWith('.wasm')) {
            // Native module: instantiate against the universal ABI, then
            // register its callable exports with compiler.wasm so the
            // EdgePython parser sees them as `from "<spec>" import name`.
            const { names, fns } = await instantiateNativeModule(spec, bytes, exports);
            const baseId = nativeTable.length;
            for (const fn of fns) nativeTable.push(fn);

            const specBytes = new TextEncoder().encode(spec);
            const namesBytes = new TextEncoder().encode(names.join('\n'));
            const sp = exports.wasm_alloc(specBytes.length);
            const np = exports.wasm_alloc(Math.max(1, namesBytes.length));
            new Uint8Array(exports.memory.buffer, sp, specBytes.length).set(specBytes);
            new Uint8Array(exports.memory.buffer, np, namesBytes.length).set(namesBytes);
            exports.register_native_module(sp, specBytes.length, np, namesBytes.length, baseId);
            // Native modules don't carry transitive Python imports, but
            // they CAN carry sibling packages.json (e.g. for bundled
            // companions). Try opportunistically.
            const wasmManifest = dirOf(spec) + 'packages.json';
            if (!knownMissing.has(wasmManifest)) queue.push(wasmManifest);
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
        const pyManifest = dir + 'packages.json';
        if (!knownMissing.has(pyManifest)) queue.push(pyManifest);
    }
}

const handlers = {
    /* Wipe the per-run source cache, the IDB CAS, and the lockfile. Next
       run() re-fetches every import from the network and rebuilds the
       lockfile from scratch. Useful when the user wants to pick up an
       upstream change that would otherwise be hidden behind cached bytes. */
    clearCache: async () => {
        fetchedSources.clear();
        knownMissing.clear();
        await idbClear('cas');
        await idbClear('lockfile');
    },

    load: async ({ url, opts, baseUrl: b }) => {
        if (b) baseUrl = b;
        try {
            const t0 = performance.now();
            // Compile without instantiating to allow multiple runs from the same module.
            wasmModule = await WebAssembly.compileStreaming(fetch(url, opts));
            self.postMessage({ type: 'ready', ms: performance.now() - t0 });
        } catch (err) {
            self.postMessage({ type: 'error', message: err.message });
        }
    },

    run: async ({ src, baseUrl: b, entryDir: e = '' }) => {
        if (b) baseUrl = b;
        entryDir = e;   // fresh per run; overrides previous value even if empty
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

        // host_print streams output line-by-line as the VM executes; the
        // shared `exports` reference lets the closures reach the same WASM
        // memory the BFS pre-fetch wrote into. host_call_native is required
        // by the WASM ABI even for code-only modules, so a stub throws.
        let exports;
        const imports = { env: {
            host_print: (ptr, len) => {
                const line = new TextDecoder().decode(
                    new Uint8Array(exports.memory.buffer, ptr, len)
                );
                self.postMessage({ type: 'line', line });
            },
            // Invoked by compiler.wasm when the script calls a native
            // (a function imported from a .wasm module). Routes through
            // `nativeTable` populated during BFS by
            // `instantiateNativeModule`. The argv/out pointers are in
            // compiler memory; we copy argv to the guest's memory before
            // calling, then read the result handle the guest wrote and
            // write it back to the compiler-side `out_ptr`.
            //
            // Per the v1 ABI, guest functions have signature
            //   fn(argv: *const u32, argc: u32, out: *mut u32) -> i32
            // and are called with pointers in their OWN linear memory,
            // so this shim arranges that staging.
            host_call_native: (id, argv_ptr, argc, out_ptr) => {
                const fn = nativeTable[id];
                if (!fn) throw new Error(`native id ${id} not registered`);
                const guestAlloc = fn.__edge_alloc;
                const guestMem   = fn.__edge_memory;
                const guestView  = new DataView(guestMem.buffer);
                const compView   = new DataView(exports.memory.buffer);

                // Stage argv + out in guest memory.
                const g_argv = guestAlloc(Math.max(4, argc * 4));
                const g_out  = guestAlloc(4);
                for (let i = 0; i < argc; i++) {
                    const h = compView.getUint32(argv_ptr + i * 4, true);
                    guestView.setUint32(g_argv + i * 4, h, true);
                }

                const status = fn(g_argv, argc, g_out);
                if (status === 0) {
                    const h = guestView.getUint32(g_out, true);
                    compView.setUint32(out_ptr, h, true);
                }
                return status;
            },
            // The bridge's resolver calls this for two purposes:
            //   1. Verify a `#sha256-...` integrity fragment on a URL import.
            //   2. Walk up looking for `<dir>/packages.json` during bare-name
            //      resolution.
            // Both consume from the same fetchedSources map; a 0 return
            // signals "no bytes for this spec" so walk-up can keep walking.
            //
            // `hashPtr`, when non-zero, points at 32 bytes the parser
            // expects the returned content to hash to. The host MUST
            // verify and return 0 on mismatch — the parser trusts this
            // answer, so a host that returns wrong bytes silently
            // breaks the lockfile contract. We compare against the
            // worker's lockfile entry for the spec; if the lockfile
            // disagrees with the parser's expectation, drift error.
            host_fetch_bytes: (specPtr, specLen, hashPtr, outLenPtr) => {
                const spec = new TextDecoder().decode(
                    new Uint8Array(exports.memory.buffer, specPtr, specLen)
                );
                const bytes = fetchedSources.get(spec);
                if (bytes === undefined) {
                    new DataView(exports.memory.buffer).setUint32(outLenPtr, 0, true);
                    return 0;
                }
                if (hashPtr !== 0) {
                    const expected = new Uint8Array(exports.memory.buffer, hashPtr, 32);
                    const knownHex = lockfile.get(spec);
                    if (knownHex) {
                        // Compare hex form of the parser's expected hash to the lockfile's.
                        const hex = [...expected].map(b => b.toString(16).padStart(2, '0')).join('');
                        if (hex !== knownHex) {
                            new DataView(exports.memory.buffer).setUint32(outLenPtr, 0, true);
                            return 0;
                        }
                    }
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
            // Each run starts with a fresh compiler.wasm instance, so any
            // native function pointers from a previous run are stale.
            nativeTable.length = 0;

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
