/* Edge Python — official browser loader.
 *
 * Single-file JS shim that consumers include alongside compiler_lib.wasm.
 * Wraps the WASM-side concerns the user shouldn't have to think about:
 *   1. Loading the WASM module and wiring up host imports (js_print,
 *      js_call_native, js_fetch_bytes).
 *   2. Pre-fetching the script's imports and registering them with the WASM
 *      runtime (since the WASM compiler is sync and browser fetch is async).
 *   3. For .wasm modules: instantiating each separately, walking exports,
 *      and routing `from "url.wasm" import f` calls back into the right
 *      WebAssembly instance via js_call_native.
 *   4. Decoding `print()` output and surfacing parse / runtime errors.
 *
 * Modules can be `.py` source or `.wasm` binaries that follow the wire format
 * documented at /reference/wasm-abi (every export is `extern "C" fn(u64, ...)
 * -> u64`, each u64 a NaN-boxed Val). The shim handles either flavor uniformly
 * — script authors don't see the difference.
 *
 * Usage (no JS knowledge required from the consumer):
 *
 *     <script type="module">
 *       import { EdgePython } from './edge.js';
 *
 *       const ep = await EdgePython.create({
 *         imports: { "math": "https://example.com/math.wasm" }
 *       });
 *       ep.onOutput(line => console.log(line));
 *       await ep.run(`
 *         from math import add
 *         from "https://example.com/utils.py" import normalize
 *         print(add(2, 3))
 *         print(normalize("  hi  "))
 *       `);
 *     </script>
 */

const TEXT_DECODER = new TextDecoder();
const TEXT_ENCODER = new TextEncoder();

export class EdgePython {
    constructor(importMap) {
        this.instance = null;
        this.exports = null;
        this.importMap = importMap || {};
        // Maps a callback id (assigned monotonically per .wasm-module export)
        // to a JS function that, when invoked with an array of BigInts,
        // returns a BigInt result. WASM-side native bindings dispatch through
        // `js_call_native(id, ...)` which routes here.
        this.callbacks = [];
        // Per-spec module cache, persists across `run()` calls so the second
        // run of the same script reuses fetched bytes instead of re-hitting
        // the network. Each entry: `{ kind: 'code' | 'native', bytes: Uint8Array }`.
        // The bytes also feed `js_fetch_bytes` for `#sha256-...` integrity
        // verification — same buffer, two consumers.
        this.cache = new Map();
        this.outputHandler = null;
        this.bufferedOutput = [];
    }

    /* Create and initialize an Edge Python runtime.
     *
     * Options:
     *   wasmUrl   — URL to fetch compiler_lib.wasm. Defaults to './compiler_lib.wasm'.
     *   imports   — { name: url } map for `from <name> import x`. URLs may be
     *               http(s)://, or relative paths resolved against the page URL.
     *               Both `.py` and `.wasm` files are loaded.
     */
    static async create({ wasmUrl = './compiler_lib.wasm', imports = {} } = {}) {
        const ep = new EdgePython(imports);
        const env = {
            js_print: (ptr, len) => ep._handlePrint(ptr, len),
            js_call_native: (id, argsPtr, argsLen) => ep._handleNativeCall(id, argsPtr, argsLen),
            js_fetch_bytes: (specPtr, specLen, outLenPtr) =>
                ep._handleFetchBytes(specPtr, specLen, outLenPtr),
        };
        const wasm = await WebAssembly.instantiateStreaming(fetch(wasmUrl), { env });
        ep.instance = wasm.instance;
        ep.exports = wasm.instance.exports;
        return ep;
    }

    /* Set a streaming output callback. Called once per `print()` line as the
     * VM executes. If unset, output buffers internally and is returned by run(). */
    onOutput(handler) { this.outputHandler = handler; }

    // Invalidate the per-spec module cache. Next run() refetches every
    // import from the network instead of reusing in-memory bytes.
    clearCache() { this.cache.clear(); }

    /* Run a script. Pre-fetches and registers every import the script declares,
     * then compiles and executes. Returns the buffered output (joined with \n)
     * or throws on parse / runtime / fetch errors. */
    async run(src) {
        this.exports.reset_modules();
        this.callbacks = [];
        // Cache survives across runs — the same script run twice doesn't
        // re-fetch its imports. To force a refresh, call clearCache().
        this.bufferedOutput = [];

        const srcBytes = TEXT_ENCODER.encode(src);
        if (srcBytes.length > (1 << 20)) {
            throw new Error('Edge Python: source exceeds 1 MiB limit');
        }

        // 1. Write source to the WASM SRC buffer.
        this._writeSrc(srcBytes);

        // 2. Pre-scan: ask WASM for the list of quoted-string imports.
        const stringSpecs = this._scanStringImports(srcBytes.length);

        // 3. Bare-name imports: scan the source for `from <name> import` and
        //    look them up in the import map. WASM's pre-scanner skips these
        //    because they need host-side resolution.
        const bareSpecs = this._scanBareImports(src);

        // 4. Resolve every spec to a URL, fetch in parallel, then register
        //    each one with the WASM runtime.
        const allSpecs = [...new Set([...stringSpecs, ...bareSpecs])];
        await Promise.all(allSpecs.map(spec => this._resolveAndRegister(spec)));

        // 5. Memory may have grown via wasm_alloc; the original src bytes might
        //    be in a stale view. Re-write to be safe (cheap).
        this._writeSrc(srcBytes);

        // 6. Run.
        const outLen = this.exports.run(srcBytes.length);
        if (outLen > 0) {
            const errOrDiag = TEXT_DECODER.decode(
                new Uint8Array(this.exports.memory.buffer, this.exports.out_ptr(), outLen)
            );
            if (errOrDiag.startsWith('error:') || errOrDiag.includes('Error:')) {
                throw new Error(errOrDiag);
            }
        }

        return this.bufferedOutput.join('\n');
    }

    // ─── Internal: WASM ↔ JS plumbing ────────────────────────────────────────

    _writeSrc(bytes) {
        const ptr = this.exports.src_ptr();
        new Uint8Array(this.exports.memory.buffer, ptr, bytes.length).set(bytes);
    }

    _scanStringImports(srcLen) {
        const len = this.exports.extract_imports(srcLen);
        if (len === 0) return [];
        const view = new Uint8Array(this.exports.memory.buffer, this.exports.out_ptr(), len);
        return TEXT_DECODER.decode(view).split('\n').filter(s => s.length > 0);
    }

    _scanBareImports(src) {
        // Match `from <name> import ...` at start of line (with optional leading
        // whitespace). Avoids matching `from "..." import` since that has a
        // quote, not an identifier.
        const re = /^\s*from\s+([A-Za-z_]\w*)\s+import/gm;
        const out = new Set();
        for (const m of src.matchAll(re)) {
            const name = m[1];
            if (name in this.importMap) out.add(name);
        }
        return [...out];
    }

    async _resolveAndRegister(spec) {
        // Strip the integrity fragment for the registry key — the WASM
        // compiler strips it internally before looking up, so registering
        // under the clean spec keeps both sides aligned.
        const cleanSpec = spec.split('#')[0];

        // Cache hit: re-register from cached bytes without re-hitting the
        // network. WASM-side `reset_modules()` cleared the registry at
        // run() start, so we re-issue register_*_module either way; what's
        // saved is the fetch round-trip.
        const cached = this.cache.get(cleanSpec);
        if (cached) {
            if (cached.kind === 'code') {
                this._registerCodeModule(cleanSpec, TEXT_DECODER.decode(cached.bytes));
            } else {
                await this._registerNativeModule(cleanSpec, cached.bytes.buffer);
            }
            return;
        }

        const url = this.importMap[spec] || spec;
        const response = await fetch(url);
        if (!response.ok) {
            throw new Error(`Edge Python: failed to fetch '${url}' (HTTP ${response.status})`);
        }
        const cleanUrl = url.split('?')[0].split('#')[0];

        if (cleanUrl.endsWith('.py')) {
            const text = await response.text();
            const bytes = TEXT_ENCODER.encode(text);
            this.cache.set(cleanSpec, { kind: 'code', bytes });
            this._registerCodeModule(cleanSpec, text);
        } else if (cleanUrl.endsWith('.wasm')) {
            const buf = await response.arrayBuffer();
            const bytes = new Uint8Array(buf);
            this.cache.set(cleanSpec, { kind: 'native', bytes });
            await this._registerNativeModule(cleanSpec, buf);
        } else {
            throw new Error(`Edge Python: unknown module type for '${url}' (expected .py or .wasm)`);
        }
    }

    _registerCodeModule(spec, src) {
        const specBytes = TEXT_ENCODER.encode(spec);
        const srcBytes = TEXT_ENCODER.encode(src);
        const specPtr = this._allocAndWrite(specBytes);
        const srcPtr = this._allocAndWrite(srcBytes);
        this.exports.register_code_module(specPtr, specBytes.length, srcPtr, srcBytes.length);
    }

    /* Instantiate the .wasm module with the browser's WebAssembly engine,
     * walk every exported function, and register each as a callable id.
     * When EdgePython invokes the binding, js_call_native routes back here
     * via `_handleNativeCall(id)` which calls into the right instance.
     * The wire format (each export takes/returns u64 NaN-boxed Vals) is
     * documented at /reference/wasm-abi. */
    async _registerNativeModule(spec, bytes) {
        const module = await WebAssembly.compile(bytes);
        const instance = await WebAssembly.instantiate(module, { env: {} });

        const fnNames = WebAssembly.Module.exports(module)
            .filter(e => e.kind === 'function')
            .map(e => e.name);

        const baseId = this.callbacks.length;
        for (const name of fnNames) {
            const wasmFn = instance.exports[name];
            // Each callback receives args as a BigInt[] (raw u64 wire) and
            // returns a BigInt (also raw u64). The .wasm itself does the
            // i64/f64/bool unpacking on its side.
            this.callbacks.push((argsBigInts) => {
                const result = wasmFn(...argsBigInts);
                return typeof result === 'bigint' ? result : BigInt(result);
            });
        }

        const specBytes = TEXT_ENCODER.encode(spec);
        const namesBytes = TEXT_ENCODER.encode(fnNames.join('\n'));
        const specPtr = this._allocAndWrite(specBytes);
        const namesPtr = this._allocAndWrite(namesBytes);
        this.exports.register_native_module(
            specPtr, specBytes.length,
            namesPtr, namesBytes.length,
            baseId
        );
    }

    _allocAndWrite(bytes) {
        const ptr = this.exports.wasm_alloc(bytes.length);
        // Re-acquire memory view AFTER the alloc — the linear memory may have
        // grown, invalidating any prior Uint8Array views over the old buffer.
        new Uint8Array(this.exports.memory.buffer, ptr, bytes.length).set(bytes);
        return ptr;
    }

    _handlePrint(ptr, len) {
        const view = new Uint8Array(this.exports.memory.buffer, ptr, len);
        const text = TEXT_DECODER.decode(view);
        if (this.outputHandler) this.outputHandler(text);
        else this.bufferedOutput.push(text);
    }

    _handleNativeCall(id, argsPtr, argsLen) {
        const callback = this.callbacks[id];
        if (!callback) {
            throw new Error(`Edge Python: no callback registered for id ${id}`);
        }
        // Read args as BigUint64s — that's the wire format the Rust side uses
        // for Val (NaN-boxed u64). Caller treats them as opaque bit patterns;
        // the .wasm module unpacks them per the documented wire format.
        const args = Array.from(
            new BigUint64Array(this.exports.memory.buffer, argsPtr, argsLen)
        );
        return callback(args);
    }

    /* Hand the WASM compiler the host-cached bytes for a spec so it can
     * verify a `#sha256-...` integrity fragment. Returns null (0) if no
     * bytes are cached — the parser treats that as "host doesn't support
     * verification" and surfaces a clean diagnostic. */
    _handleFetchBytes(specPtr, specLen, outLenPtr) {
        const spec = TEXT_DECODER.decode(
            new Uint8Array(this.exports.memory.buffer, specPtr, specLen)
        );
        const cached = this.cache.get(spec);
        if (!cached) {
            new DataView(this.exports.memory.buffer).setUint32(outLenPtr, 0, true);
            return 0;
        }
        // wasm_alloc may grow linear memory, invalidating any view captured
        // before this call — re-acquire DataView/Uint8Array from the current
        // buffer when writing the bytes and out_len.
        const ptr = this.exports.wasm_alloc(cached.bytes.length);
        new Uint8Array(this.exports.memory.buffer, ptr, cached.bytes.length).set(cached.bytes);
        new DataView(this.exports.memory.buffer).setUint32(outLenPtr, cached.bytes.length, true);
        return ptr;
    }
}
