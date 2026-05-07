const source_size = 1 << 20; // 1 MiB limit.

let wasmModule = null;

/* Per-spec source cache. Lives in worker scope so it survives across `run`
   messages — successive runs of the same script reuse fetched modules
   instead of re-hitting the network on every click. The bytes also feed
   `js_fetch_bytes` so the parser can verify `#sha256-...` integrity
   fragments using the same buffer. */
const fetchedSources = new Map();

const handlers = {
    // Invalidate the per-spec module cache. Next run() refetches every
    // import from the network instead of reusing in-memory bytes.
    clearCache: () => fetchedSources.clear(),

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

        // js_print is called by the VM on every print(); each line is fired to
        // the main thread immediately as WASM executes, before run() returns.
        // js_call_native is required by the WASM ABI even though the demo only
        // uses code (.py) modules — the host import must be satisfied at
        // instantiate time. Stub throws if a native is reached unexpectedly.
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
            // Serve cached source bytes to the parser when it asks for
            // them to verify a `#sha256-...` integrity fragment. Same
            // `fetchedSources` map the run loop populates — one cache, two
            // consumers (registration + integrity).
            js_fetch_bytes: (specPtr, specLen, outLenPtr) => {
                const spec = new TextDecoder().decode(
                    new Uint8Array(exports.memory.buffer, specPtr, specLen)
                );
                const text = fetchedSources.get(spec);
                if (text === undefined) {
                    new DataView(exports.memory.buffer).setUint32(outLenPtr, 0, true);
                    return 0;
                }
                const bytes = new TextEncoder().encode(text);
                const ptr = exports.wasm_alloc(bytes.length);
                new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
                new DataView(exports.memory.buffer).setUint32(outLenPtr, bytes.length, true);
                return ptr;
            },
        }};

        // A WASM trap (Rust panic, stack overflow, OOM) leaves the instance
        // unusable but `wasmModule` stays valid — next run gets a fresh one.
        // Surface the trap as a result so main thread clears `busy` and the UI
        // recovers instead of hanging on "Running..." forever.
        try {
            ({ exports } = await WebAssembly.instantiate(wasmModule, imports));

            // Stage source then pre-register every quoted code-module import
            // the script declares (`from "./..." import` etc.). Each spec is
            // fetched relative to the worker URL and registered with the WASM
            // runtime BEFORE run() — the parser is sync and consults its
            // resolver at compile time.
            const writeSrc = () => {
                new Uint8Array(exports.memory.buffer).set(srcBytes, exports.src_ptr());
            };
            writeSrc();

            const importsLen = exports.extract_imports(srcBytes.length);
            const specStr = importsLen
                ? new TextDecoder().decode(new Uint8Array(
                    exports.memory.buffer, exports.out_ptr(), importsLen))
                : '';
            const specs = specStr ? specStr.split('\n').filter(Boolean) : [];

            for (const spec of specs) {
                let text = fetchedSources.get(spec);
                if (text === undefined) {
                    const url = new URL(spec, self.location.href).toString();
                    text = await fetch(url).then(r => {
                        if (!r.ok) throw new Error(`HTTP ${r.status} fetching ${spec}`);
                        return r.text();
                    });
                    fetchedSources.set(spec, text);
                }
                const specBytes = new TextEncoder().encode(spec);
                const codeBytes = new TextEncoder().encode(text);
                const sp = exports.wasm_alloc(specBytes.length);
                const cp = exports.wasm_alloc(codeBytes.length);
                new Uint8Array(exports.memory.buffer, sp, specBytes.length).set(specBytes);
                new Uint8Array(exports.memory.buffer, cp, codeBytes.length).set(codeBytes);
                exports.register_code_module(sp, specBytes.length, cp, codeBytes.length);
            }
            // wasm_alloc may have grown linear memory, invalidating earlier views.
            // Re-write the source at the (still valid) src_ptr offset.
            writeSrc();

            const t0 = performance.now();
            const len = exports.run(srcBytes.length);
            const ms = performance.now() - t0;

            // `out` is empty on success (streamed); non-empty only for errors.
            const out = new TextDecoder().decode(
                new Uint8Array(exports.memory.buffer, exports.out_ptr(), len)
            );

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