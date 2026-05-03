const source_size = 1 << 20; // 1 MiB limit.

let wasmModule = null;

const handlers = {
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
        let exports;
        const imports = { env: { js_print: (ptr, len) => {
            const line = new TextDecoder().decode(
                new Uint8Array(exports.memory.buffer, ptr, len)
            );
            self.postMessage({ type: 'line', line });
        }}};

        // A WASM trap (Rust panic, stack overflow, OOM) leaves the instance
        // unusable but `wasmModule` stays valid — next run gets a fresh one.
        // Surface the trap as a result so main thread clears `busy` and the UI
        // recovers instead of hanging on "Running..." forever.
        try {
            ({ exports } = await WebAssembly.instantiate(wasmModule, imports));

            new Uint8Array(exports.memory.buffer).set(srcBytes, exports.src_ptr());

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