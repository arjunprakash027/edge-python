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

// Worker-lifetime state
let wasmModule = null;
let compilerExports = null;
let cache = null;
let integrityActive = false;
let loaders = [];
let importsMap = null;

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

export async function run({ src, entryDir = '', baseUrl = null, onLine }) {
    if (!wasmModule) throw new Error('engine.load() must be called before run()');

    const srcBytes = TE.encode(src);
    if (srcBytes.length > SOURCE_LIMIT) throw new Error(`source exceeds ${SOURCE_LIMIT} bytes`);

    let lockfile = new Map();
    if (integrityActive) {
        try { lockfile = await cache.loadLockfile(); }
        catch { /* lockfile load failure is non-fatal; treat as empty */ }
    }

    const fetchedSources = new Map();
    const knownMissing = new Set();

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

    const t0 = performance.now();
    const len = exports.run(srcBytes.length);
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

export function reset() {
    if (compilerExports) compilerExports.reset_modules();
    resetNativeTable();
}

export async function clearCache() {
    if (cache) await cache.clear();
}

export function dispose() {
    wasmModule = null;
    compilerExports = null;
    cache = null;
    loaders = [];
    importsMap = null;
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
