/* 
Web Worker entry. Wires `load`/`run`/`clearCache` to ./worker/*; actual logic lives there. 
*/

import { idbClear, idbCursor, idbGet, idbPut, idbPutAll } from './worker/idb.js';
import { bfsPrefetch } from './worker/prefetch.js';
import { nativeTable } from './worker/native.js';

const SOURCE_LIMIT = 1 << 20; // 1 MiB
const TD = new TextDecoder();
const TE = new TextEncoder();

let wasmModule = null;

/* Worker-lifetime shared state, passed into fetch/prefetch (not module statics) so each subsystem stays testable in isolation. */
const ctx = {
    baseUrl: null,
    entryDir: '',
    /* Cache keyed by parser's canonical spec; feeds register_code_module and host_fetch_bytes; survives across runs, clearCache wipes. */
    fetchedSources: new Map(),
    /* 404-known packages.json specs, populated by BFS; consulted on re-enqueue so Run-button mashes don't re-probe dead URLs. */
    knownMissing: new Set(),
};

const handlers = {
    /* Wipe per-run sources, IDB CAS, and lockfile; next run() re-fetches every import and rebuilds the lockfile. */
    clearCache: async () => {
        ctx.fetchedSources.clear();
        ctx.knownMissing.clear();
        await idbClear('cas');
        await idbClear('lockfile');
    },

    load: async ({ body, baseUrl, version }) => {
        if (baseUrl) ctx.baseUrl = baseUrl;

        // Run the IDB version-check alongside compile rather than gating it; compileStreaming dominates wall-time.
        const idbWork = (async () => {
            try {
                const stored = await idbGet('lockfile', '\0v');
                if (!version || stored !== version) {
                    await idbClear('cas');
                    await idbClear('lockfile');
                    if (version) await idbPut('lockfile', '\0v', version); // '\0' isolates sentinel — canonical specs never contain null bytes
                }
            } catch { /* IDB blocked: nothing to invalidate */ }
        })();

        try {
            const t0 = performance.now();
            // Wrap the transferred stream so compileStreaming gets the required Content-Type without a second fetch.
            const response = new Response(body, { headers: { 'Content-Type': 'application/wasm' } });
            // Compile without instantiating so each run can build a fresh instance.
            const [module] = await Promise.all([
                WebAssembly.compileStreaming(response),
                idbWork,
            ]);
            wasmModule = module;
            self.postMessage({ type: 'ready', ms: performance.now() - t0 });
        } catch (err) {
            self.postMessage({ type: 'error', message: err.message });
        }
    },

    run: async ({ src, baseUrl, entryDir = '' }) => {
        if (baseUrl) ctx.baseUrl = baseUrl;
        ctx.entryDir = entryDir; // fresh per run; overrides previous value even if empty
        const srcBytes = TE.encode(src);

        if (srcBytes.length > SOURCE_LIMIT) {
            self.postMessage({ type: 'result', out: `Error: Source exceeds ${SOURCE_LIMIT} bytes` });
            return;
        }

        // Hydrate lockfile from IDB into a session-local Map; BFS mutations persist only after a successful run.
        const lockfile = new Map();
        try { await idbCursor('lockfile', (k, v) => { if (k !== '\0v') lockfile.set(k, v); }); }
        catch { /* private mode / IDB blocked: in-memory only */ }

        // `host_print` streams output as the VM executes; closures share `exports` to read the BFS-staged WASM memory.
        let exports;
        /* Fresh views per call — `wasm_alloc` may grow linear memory between round-trips, detaching any cached ArrayBuffer view. */
        const readStr = (ptr, len) => TD.decode(new Uint8Array(exports.memory.buffer, ptr, len));
        const setU32 = (ptr, v) => new DataView(exports.memory.buffer).setUint32(ptr, v, true);

        const imports = { env: {
            host_print: (ptr, len) => self.postMessage({ type: 'line', line: readStr(ptr, len) }),
            /* Compiler.wasm -> native call through `nativeTable`: stage argv guest-side, then copy guest's result back to compiler `out_ptr`. */
            host_call_native: (id, argv_ptr, argc, out_ptr) => {
                const fn = nativeTable[id];
                if (!fn) throw new Error(`native id ${id} not registered`);
                const guestView = new DataView(fn.__edge_memory.buffer);
                const compView = new DataView(exports.memory.buffer);

                // Stage argv + out in guest memory.
                const g_argv = fn.__edge_alloc(Math.max(4, argc * 4));
                const g_out  = fn.__edge_alloc(4);
                for (let i = 0; i < argc; i++) {
                    guestView.setUint32(g_argv + i * 4, compView.getUint32(argv_ptr + i * 4, true), true);
                }

                const status = fn(g_argv, argc, g_out);
                if (status === 0) compView.setUint32(out_ptr, guestView.getUint32(g_out, true), true);
                return status;
            },
            /* Bridge resolver hook: serves `ctx.fetchedSources` for `#sha256-...` verification and `<dir>/packages.json` walk-up; returns 0 (drift) on lockfile mismatch. */
            host_fetch_bytes: (specPtr, specLen, hashPtr, outLenPtr) => {
                const spec = readStr(specPtr, specLen);
                const bytes = ctx.fetchedSources.get(spec);
                if (bytes === undefined) { setU32(outLenPtr, 0); return 0; }
                if (hashPtr !== 0) {
                    const knownHex = lockfile.get(spec);
                    if (knownHex) {
                        const expected = new Uint8Array(exports.memory.buffer, hashPtr, 32);
                        const hex = [...expected].map(b => b.toString(16).padStart(2, '0')).join('');
                        if (hex !== knownHex) { setU32(outLenPtr, 0); return 0; }
                    }
                }
                const ptr = exports.wasm_alloc(bytes.length);
                new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
                setU32(outLenPtr, bytes.length);
                return ptr;
            },
        }};

        try {
            ({ exports } = await WebAssembly.instantiate(wasmModule, imports));
            exports.reset_modules();
            // Fresh compiler.wasm instance each run, so previous-run native function pointers are stale.
            nativeTable.length = 0;

            // Stage entry source, then run BFS pre-fetch over the graph.
            const writeSrc = () => new Uint8Array(exports.memory.buffer).set(srcBytes, exports.src_ptr());
            writeSrc();
            await bfsPrefetch(src, exports, lockfile, ctx);
            // BFS-time `wasm_alloc` may have grown memory and invalidated views; re-write source at the still-valid offset.
            writeSrc();

            const t0 = performance.now();
            const len = exports.run(srcBytes.length);
            const ms = performance.now() - t0;
            const out = readStr(exports.out_ptr(), len);

            // Persist new lockfile entries post-run; deferred until success so failed runs don't pin partial hashes.
            try { await idbPutAll('lockfile', lockfile); }
            catch { /* IDB blocked — accept loss of persistence */ }

            self.postMessage({ type: 'result', out, ms });
        } catch (err) {
            self.postMessage({ type: 'result', out: `Runtime trap: ${err?.message ?? err}`, ms: 0 });
        }
    },
};

self.onmessage = ({ data }) => handlers[data.type]?.(data);
