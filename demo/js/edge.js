/* 
Edge Python browser loader: loads `compiler_lib.wasm`, prefetches imports, routes `.wasm` natives, decodes `print()`/errors. See /reference/wasm-abi. 
*/

const TEXT_DECODER = new TextDecoder();
const TEXT_ENCODER = new TextEncoder();
const SOURCE_LIMIT = 1 << 20; // 1 MiB

export class EdgePython {
    constructor(importMap) {
        this.instance = null;
        this.exports = null;
        this.importMap = importMap || {};
        // Callback id -> JS fn taking BigInt[] returning BigInt; `host_call_native(id, ...)` from WASM routes here.
        this.callbacks = [];
        // Per-spec module cache persisting across runs; entries `{ kind, bytes }` also feed `host_fetch_bytes` for integrity checks.
        this.cache = new Map();
        this.outputHandler = null;
        this.bufferedOutput = [];
    }

    /* Create + initialize the runtime. `wasmUrl` points at compiler_lib.wasm; `imports` maps bare names to `.py`/`.wasm` URLs. */
    static async create({ wasmUrl = './compiler_lib.wasm', imports = {} } = {}) {
        const ep = new EdgePython(imports);
        const env = {
            host_print: ep._handlePrint.bind(ep),
            host_call_native: ep._handleNativeCall.bind(ep),
            host_fetch_bytes: ep._handleFetchBytes.bind(ep),
        };
        const { instance } = await WebAssembly.instantiateStreaming(fetch(wasmUrl), { env });
        ep.instance = instance;
        ep.exports = instance.exports;
        return ep;
    }

    /* Streaming output callback, called per `print()` line; if unset, output buffers and is returned by `run()`. */
    onOutput(handler) { this.outputHandler = handler; }

    // Invalidate the per-spec cache so next `run()` refetches every import instead of reusing in-memory bytes.
    clearCache() { this.cache.clear(); }

    /* Run a script: prefetch + register imports, compile, execute. Returns buffered output or throws on parse/runtime/fetch error. */
    async run(src) {
        this.exports.reset_modules();
        this.callbacks = [];
        this.bufferedOutput = [];

        const srcBytes = TEXT_ENCODER.encode(src);
        if (srcBytes.length > SOURCE_LIMIT) throw new Error('Edge Python: source exceeds 1 MiB limit');

        this._writeSrc(srcBytes);

        /* Bare-name imports need host-side resolution against `importMap`; WASM's pre-scanner skips them because they're not quoted strings. */
        const specs = [...new Set([
            ...this._scanStringImports(srcBytes.length),
            ...this._scanBareImports(src),
        ])];
        await Promise.all(specs.map(spec => this._resolveAndRegister(spec)));

        // `wasm_alloc` may have grown linear memory during registration, detaching the buffer view we wrote `srcBytes` into. Re-write.
        this._writeSrc(srcBytes);

        const outLen = this.exports.run(srcBytes.length);
        if (outLen > 0) {
            const diag = this._readStr(this.exports.out_ptr(), outLen);
            if (diag.startsWith('error:') || diag.includes('Error:')) throw new Error(diag);
        }

        return this.bufferedOutput.join('\n');
    }

    // Internal: WASM <-> JS plumbing

    /* Fresh views per call, where `wasm_alloc` may grow linear memory between round-trips, detaching any cached ArrayBuffer view. */
    _readStr(ptr, len) { return TEXT_DECODER.decode(new Uint8Array(this.exports.memory.buffer, ptr, len)); }
    _setU32(ptr, v) { new DataView(this.exports.memory.buffer).setUint32(ptr, v, true); }

    _writeSrc(bytes) {
        new Uint8Array(this.exports.memory.buffer, this.exports.src_ptr(), bytes.length).set(bytes);
    }

    _scanStringImports(srcLen) {
        const len = this.exports.extract_imports(srcLen);
        if (len === 0) return [];
        return this._readStr(this.exports.out_ptr(), len).split('\n').filter(Boolean);
    }

    _scanBareImports(src) {
        // Matches `from <name> import` (bare-name) at line start; quoted-string forms have `"`, excluded by the identifier class.
        const re = /^\s*from\s+([A-Za-z_]\w*)\s+import/gm;
        const out = new Set();
        for (const m of src.matchAll(re)) if (m[1] in this.importMap) out.add(m[1]);
        return [...out];
    }

    async _resolveAndRegister(spec) {
        // Strip integrity fragment from the registry key: WASM compiler strips internally, so both sides stay aligned.
        const cleanSpec = spec.split('#')[0];

        let entry = this.cache.get(cleanSpec);
        if (!entry) {
            const url = this.importMap[spec] || spec;
            const response = await fetch(url);
            if (!response.ok) throw new Error(`Edge Python: failed to fetch '${url}' (HTTP ${response.status})`);
            const cleanUrl = url.split('?')[0].split('#')[0];
            if (cleanUrl.endsWith('.py')) {
                entry = { kind: 'code',   bytes: TEXT_ENCODER.encode(await response.text()) };
            } else if (cleanUrl.endsWith('.wasm')) {
                entry = { kind: 'native', bytes: new Uint8Array(await response.arrayBuffer()) };
            } else {
                throw new Error(`Edge Python: unknown module type for '${url}' (expected .py or .wasm)`);
            }
            this.cache.set(cleanSpec, entry);
        }

        if (entry.kind === 'code') this._registerCodeModule(cleanSpec, entry.bytes);
        else await this._registerNativeModule(cleanSpec, entry.bytes);
    }

    _registerCodeModule(spec, srcBytes) {
        const specBytes = TEXT_ENCODER.encode(spec);
        const specPtr = this._allocAndWrite(specBytes);
        const srcPtr = this._allocAndWrite(srcBytes);
        this.exports.register_code_module(specPtr, specBytes.length, srcPtr, srcBytes.length);
    }

    /* Instantiate .wasm, walk exports, register each as a callable id; `host_call_native` -> `_handleNativeCall(id)` reaches the right instance. */
    async _registerNativeModule(spec, bytes) {
        const module = await WebAssembly.compile(bytes);
        const instance = await WebAssembly.instantiate(module, { env: {} });

        const fnNames = WebAssembly.Module.exports(module)
            .filter(e => e.kind === 'function')
            .map(e => e.name);

        const baseId = this.callbacks.length;
        for (const name of fnNames) {
            const wasmFn = instance.exports[name];
            // Callbacks pass BigInt[] (raw u64 wire) and return BigInt; the .wasm itself unpacks i64/f64/bool.
            this.callbacks.push((argsBigInts) => {
                const result = wasmFn(...argsBigInts);
                return typeof result === 'bigint' ? result : BigInt(result);
            });
        }

        const specBytes = TEXT_ENCODER.encode(spec);
        const namesBytes = TEXT_ENCODER.encode(fnNames.join('\n'));
        const specPtr = this._allocAndWrite(specBytes);
        const namesPtr = this._allocAndWrite(namesBytes);
        this.exports.register_native_module(specPtr, specBytes.length, namesPtr, namesBytes.length, baseId);
    }

    _allocAndWrite(bytes) {
        const ptr = this.exports.wasm_alloc(bytes.length);
        // Re-acquire the memory view AFTER alloc: growth may have detached any prior view over the old buffer.
        new Uint8Array(this.exports.memory.buffer, ptr, bytes.length).set(bytes);
        return ptr;
    }

    _handlePrint(ptr, len) {
        const text = this._readStr(ptr, len);
        if (this.outputHandler) this.outputHandler(text);
        else this.bufferedOutput.push(text);
    }

    _handleNativeCall(id, argsPtr, argsLen) {
        const callback = this.callbacks[id];
        if (!callback) throw new Error(`Edge Python: no callback registered for id ${id}`);
        // Read args as BigUint64 — Rust's Val wire format (NaN-boxed u64); `.wasm` unpacks i64/f64/bool itself.
        return callback(Array.from(new BigUint64Array(this.exports.memory.buffer, argsPtr, argsLen)));
    }

    /* Serve cached bytes for `#sha256-...` verification; cache keys by spec (no lockfile check), parser re-hashes for defence. */
    _handleFetchBytes(specPtr, specLen, _hashPtr, outLenPtr) {
        const spec = this._readStr(specPtr, specLen);
        const cached = this.cache.get(spec);
        if (!cached) { this._setU32(outLenPtr, 0); return 0; }
        const ptr = this._allocAndWrite(cached.bytes);
        this._setU32(outLenPtr, cached.bytes.length);
        return ptr;
    }
}
