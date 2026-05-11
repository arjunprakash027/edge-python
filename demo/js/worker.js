/* Web Worker entry point.
   Pure wiring: compile WASM on `load`, BFS-prefetch + run on `run`, clear
   caches on `clearCache`. The actual logic lives in ./worker/*. */

import { idbClear, idbCursor, idbPutAll } from './worker/idb.js';
import { bfsPrefetch } from './worker/prefetch.js';
import { nativeTable } from './worker/native.js';

const SOURCE_LIMIT = 1 << 20; // 1 MiB

let wasmModule = null;

/* Shared state across the worker's lifetime; passed into fetch/prefetch
   instead of being module-level statics so each subsystem is testable
   in isolation. */
const ctx = {
    baseUrl: null,
    entryDir: '',
    /* Per-spec source cache keyed by the canonical spec the parser sees
       (so a quoted relative `./helpers.py` inside `./lib/a.py` is stored
       under `./lib/helpers.py`, matching the bridge resolver's
       join_relative output). Survives across runs; cleared by clearCache
       and by IDB-CAS invalidation. Feeds two consumers:
         • register_code_module for `.py` files -> REGISTRY (parser).
         • host_fetch_bytes for `packages.json` and integrity-checked URLs. */
    fetchedSources: new Map(),
    /* packages.json specs known to 404 — populated as BFS encounters
       missing manifests, consulted before re-enqueuing across runs so a
       Run-button mash doesn't re-probe dead URLs every time. */
    knownMissing: new Set(),
};

const handlers = {
    /* Wipe per-run sources, the IDB CAS, and the lockfile. Next run()
       re-fetches every import from the network and rebuilds the lockfile. */
    clearCache: async () => {
        ctx.fetchedSources.clear();
        ctx.knownMissing.clear();
        await idbClear('cas');
        await idbClear('lockfile');
    },

    load: async ({ url, opts, baseUrl: b }) => {
        if (b) ctx.baseUrl = b;
        try {
            const t0 = performance.now();
            // Compile without instantiating so each run can build a fresh instance.
            wasmModule = await WebAssembly.compileStreaming(fetch(url, opts));
            self.postMessage({ type: 'ready', ms: performance.now() - t0 });
        } catch (err) {
            self.postMessage({ type: 'error', message: err.message });
        }
    },

    run: async ({ src, baseUrl: b, entryDir: e = '' }) => {
        if (b) ctx.baseUrl = b;
        ctx.entryDir = e;   // fresh per run; overrides previous value even if empty
        const srcBytes = new TextEncoder().encode(src);

        if (srcBytes.length > SOURCE_LIMIT) {
            self.postMessage({ type: 'result', out: `Error: Source exceeds ${SOURCE_LIMIT} bytes` });
            return;
        }

        // Hydrate the lockfile from IDB into a session-local Map. Mutations
        // made during BFS are persisted only after a successful run, so a
        // mid-run failure doesn't leave a half-written lockfile.
        const lockfile = new Map();
        try {
            await idbCursor('lockfile', (k, v) => lockfile.set(k, v));
        } catch { /* private mode / IDB blocked: in-memory only */ }

        // host_print streams output line-by-line as the VM executes; the
        // shared `exports` reference lets the closures reach the same WASM
        // memory the BFS pre-fetch wrote into.
        let exports;
        const imports = { env: {
            host_print: (ptr, len) => {
                const line = new TextDecoder().decode(
                    new Uint8Array(exports.memory.buffer, ptr, len)
                );
                self.postMessage({ type: 'line', line });
            },
            /* Invoked by compiler.wasm when the script calls a native (a
               function imported from a .wasm module). Routes through
               `nativeTable` populated during BFS by instantiateNativeModule.
               argv/out pointers are in compiler memory; we copy argv to the
               guest's memory before calling, then read the result handle
               the guest wrote and write it back to the compiler-side out_ptr.

               Per the v1 ABI, guest functions have signature
                 fn(argv: *const u32, argc: u32, out: *mut u32) -> i32
               called with pointers in their OWN linear memory, so this shim
               arranges that staging. */
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
            /* The bridge's resolver calls this for two purposes:
                 1. Verify a `#sha256-...` integrity fragment on a URL import.
                 2. Walk up looking for `<dir>/packages.json` during bare-name
                    resolution.
               Both consume from ctx.fetchedSources; a 0 return signals "no
               bytes for this spec" so walk-up can keep walking.

               `hashPtr`, when non-zero, points at 32 bytes the parser
               expects the returned content to hash to. We compare against
               the worker's lockfile entry; if they disagree, return 0 so
               the parser surfaces a clean drift diagnostic. */
            host_fetch_bytes: (specPtr, specLen, hashPtr, outLenPtr) => {
                const spec = new TextDecoder().decode(
                    new Uint8Array(exports.memory.buffer, specPtr, specLen)
                );
                const bytes = ctx.fetchedSources.get(spec);
                if (bytes === undefined) {
                    new DataView(exports.memory.buffer).setUint32(outLenPtr, 0, true);
                    return 0;
                }
                if (hashPtr !== 0) {
                    const expected = new Uint8Array(exports.memory.buffer, hashPtr, 32);
                    const knownHex = lockfile.get(spec);
                    if (knownHex) {
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
            await bfsPrefetch(src, exports, lockfile, ctx);
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
            try { await idbPutAll('lockfile', lockfile); }
            catch { /* IDB blocked — accept loss of persistence */ }

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
