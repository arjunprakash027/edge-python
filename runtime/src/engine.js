/*
Engine orchestrator (runs inside the Worker). Lifecycle: `load` once → many `run` cycles → `dispose`. Each run instantiates compiler_lib fresh so prior-run state cannot leak.
*/

import { MemoryCache } from './cache/memory.js';
import { bfsPrefetch } from './prefetch.js';
import { makeCompilerEnv } from './env.js';
import { makeRt } from './rt.js';
import { nativeTable, resetNativeTable } from './native.js';

const SOURCE_LIMIT = 1 << 20; // 1 MiB
const TE = new TextEncoder();
const TD = new TextDecoder();

/* Packed status from `run_start` / `run_resume`; mirrors `compiler/src/main/exports.rs`. */
const STATUS_KIND_SHIFT = 29;
const STATUS_PAYLOAD_MASK = (1 << STATUS_KIND_SHIFT) - 1;
const STATUS_DONE = 0;
const STATUS_PENDING_TIMER = 1;
const STATUS_PENDING_FRAME = 2;
const STATUS_PENDING_EVENT = 3;
const STATUS_ERROR = 4;

// Worker-lifetime state
let wasmModule = null;
let compilerExports = null;
let cache = null;
let integrityActive = false;
let loaders = [];
let importsMap = null;
// Resolves run()'s current `await` when a `PendingEvent` wake-up arrives via `pushEvent`.
let eventWaiter = null;
// Source/missing caches persist across runs so the BFS doesn't re-fetch every module — and especially doesn't re-probe 404'd `packages.json` paths — on every Run-button press. Wiped by `clearCache()`.
const fetchedSources = new Map();
const knownMissing = new Set();

export async function load({ wasmUrl, integrity = true, loaders: loaderUrls = [], imports = null, version = null }) {
    const t0 = performance.now();
    importsMap = imports;

    cache = await openCache(integrity);
    integrityActive = cache instanceof MemoryCache ? false : Boolean(integrity);

    if (integrityActive) {
        const stored = await cache.getVersion();
        if (!version || stored !== version) {
            await cache.clear();
            if (version) await cache.setVersion(version);
        }
    }

    loaders = await Promise.all(
        loaderUrls.map(async (url) => (await import(url)).default)
    );

    const response = await fetch(wasmUrl);
    if (!response.ok) throw new Error(`fetch failed for '${wasmUrl}' (${response.status})`);
    const wrapped = new Response(response.body, { headers: { 'Content-Type': 'application/wasm' } });
    wasmModule = await WebAssembly.compileStreaming(wrapped);

    return { integrityActive, loadMs: performance.now() - t0 };
}

/* If the script defines `main`, drive it automatically — keeps user scripts free of explicit `run(main())`. */
const AUTO_ENTRY = '\n"main" in globals() and run(main())\n';

export async function run({ src, entryDir = '', baseUrl = null, onLine }) {
    if (!wasmModule) throw new Error('engine.load() must be called before run()');

    src = src + AUTO_ENTRY;
    const srcBytes = TE.encode(src);
    if (srcBytes.length > SOURCE_LIMIT) throw new Error(`source exceeds ${SOURCE_LIMIT} bytes`);

    let lockfile = new Map();
    if (integrityActive) {
        try { lockfile = await cache.loadLockfile(); }
        catch { /* lockfile load failure is non-fatal; treat as empty */ }
    }

    const env = makeCompilerEnv({
        getExports: () => compilerExports,
        onLine: onLine ?? (() => {}),
        fetchedSources,
        lockfile,
        integrityActive,
    });

    const { exports } = await WebAssembly.instantiate(wasmModule, { env });
    compilerExports = exports;
    exports.reset_modules();
    resetNativeTable();

    const rt = makeRt(() => compilerExports);

    const writeSrc = () => new Uint8Array(exports.memory.buffer).set(srcBytes, exports.src_ptr());
    writeSrc();

    await bfsPrefetch(src, exports, lockfile, {
        cache,
        baseUrl,
        entryDir,
        knownMissing,
        importsMap,
        integrityActive,
        fetchedSources,
        compilerExports: exports,
        rt,
        loaders,
    });

    // `wasm_alloc` during prefetch may have grown memory and detached our src view.
    writeSrc();

    // Driver loop: `run_start` then `run_resume` after each host wake-up until Done / Error.
    const t0 = performance.now();
    let status = exports.run_start(srcBytes.length);
    while (true) {
        const kind = (status >>> STATUS_KIND_SHIFT) & 7;
        if (kind === STATUS_DONE || kind === STATUS_ERROR) break;
        if (kind === STATUS_PENDING_TIMER) {
            const deadlineNs = exports.last_yield_deadline_ns();
            const nowNs = BigInt(Date.now()) * 1_000_000n;
            const waitMs = deadlineNs > nowNs ? Number((deadlineNs - nowNs) / 1_000_000n) : 0;
            await new Promise(r => setTimeout(r, waitMs));
        } else if (kind === STATUS_PENDING_FRAME) {
            await new Promise(r => requestAnimationFrame(r));
        } else if (kind === STATUS_PENDING_EVENT) {
            await new Promise(r => { eventWaiter = r; });
        } else {
            // Unknown kind — bail out instead of looping forever.
            break;
        }

        status = exports.run_resume();
    }
    const len = status & STATUS_PAYLOAD_MASK;
    const ms = performance.now() - t0;
    const out = len > 0
        ? TD.decode(new Uint8Array(exports.memory.buffer, exports.out_ptr(), len))
        : '';

    if (integrityActive) {
        try { await cache.saveLockfile(lockfile); }
        catch { /* persistence failure is non-fatal; lockfile lives in-memory until next save */ }
    }

    return { out, ms };
}

/* Push a string into the paused VM's `event_queue`; wakes `receive()` and resolves the driver's await. */
export function pushEvent(message) {
    if (!compilerExports) return false;
    const bytes = TE.encode(String(message));
    const ptr = compilerExports.wasm_alloc(bytes.length);
    new Uint8Array(compilerExports.memory.buffer, ptr, bytes.length).set(bytes);
    const status = compilerExports.run_push_event(ptr, bytes.length);
    compilerExports.wasm_free(ptr, bytes.length);
    if (eventWaiter) {
        const w = eventWaiter;
        eventWaiter = null;
        w();
    }
    return status === 0;
}

/* Browser bridges fire `CustomEvent("edge-python-event")`; engine routes the detail string to `pushEvent`. */
if (typeof window !== 'undefined') {
    window.addEventListener('edge-python-event', (e) => {
        if (typeof e.detail === 'string') pushEvent(e.detail);
    });
}

export function reset() {
    if (compilerExports) compilerExports.reset_modules();
    resetNativeTable();
}

export async function clearCache() {
    fetchedSources.clear();
    knownMissing.clear();
    if (cache) await cache.clear();
}

export function dispose() {
    wasmModule = null;
    compilerExports = null;
    cache = null;
    loaders = [];
    importsMap = null;
    fetchedSources.clear();
    knownMissing.clear();
    resetNativeTable();
}

async function openCache(integrity) {
    if (!integrity) return new MemoryCache();
    try {
        const { IdbCache } = await import('./cache/idb.js');
        const idb = new IdbCache();
        await idb.open();
        return idb;
    } catch (e) {
        console.warn(
            '[edge-python] integrity:true requested but IndexedDB unavailable; '
            + 'running with in-memory cache. Check worker.integrityActive to detect.',
            e?.message ?? ''
        );
        return new MemoryCache();
    }
}
