# Edge Python DOM

A JavaScript runtime that lets Edge Python scripts manipulate the DOM at near-native speed. It exposes a Python-facing API for querying, mutating, and listening to real DOM nodes — no canvas, no virtual layer, no JSON serialization. The bridge between the WebAssembly module and the browser is engineered to minimize every crossing: nodes travel as opaque `u32` handles, strings move zero-copy through shared linear memory, and DOM operations are batched into a command buffer flushed once per microtask.

## Architecture

The runtime keeps a JS-side table of real DOM nodes addressed by index from WebAssembly. A Python call like `query("#app")` returns the integer slot, not a serialized object, and subsequent operations carry only that integer back across the boundary. When the engine supports `externref`, the handle is the `externref` itself and the slot table is managed by the JavaScript GC; otherwise a generational handle table on the JS side detects use-after-free. Strings move without copying: WebAssembly writes UTF-8 into linear memory, and JavaScript reads them through a `Uint8Array` view re-created per call (linear memory may grow and detach the underlying buffer). When the JS String Builtins proposal is available, the runtime imports JS string operations directly and skips decoding entirely.

```python
el = query("#app") # Returns a u32 handle.
set_text(el, "hello") # One crossing, string passed by pointer + length.
```

Mutations issued inside the same microtask accumulate in a command buffer carved out of linear memory and apply in a single drain at the microtask boundary — one boundary crossing for an arbitrary number of DOM operations, the same coalescing principle GPUs use for draw calls. For bulk work (initial render, list virtualization), a `SharedArrayBuffer` path is selected when the host serves the page with the required cross-origin isolation headers.

```js
// Host side, conceptual
const nodes = [];
function host_create_element(ptr, len) {
    const tag = decoder.decode(new Uint8Array(memory.buffer, ptr, len));
    nodes.push(document.createElement(tag));
    return nodes.length - 1;
}
```

## Repository Layout

```bash
├── Cargo.toml
├── README.md
├── src
│   └── main.rs
└── tests
    └── main.rs
```

## References

1. **Haas et al.**, *[Bringing the Web up to Speed with WebAssembly](https://dl.acm.org/doi/10.1145/3062341.3062363)* (PLDI 2017). WebAssembly design and FFI rationale.
2. **Holmes**, *[When Is WebAssembly Going to Get DOM Support?](https://queue.acm.org/detail.cfm?id=3746174)* (ACM Queue 2026). WASM↔DOM bridge and glue patterns.
3. **WebAssembly CG**, *[Reference Types Proposal Overview](https://github.com/WebAssembly/reference-types/blob/master/proposals/reference-types/Overview.md)* (2020). `externref` host references.
4. **WebAssembly CG**, *[JS String Builtins Proposal Overview](https://github.com/WebAssembly/js-string-builtins/blob/main/proposals/js-string-builtins/Overview.md)* (2024). Glue-free JS string interop.
5. **Calderón**, *[16 Patterns for Crossing the WebAssembly Boundary](https://dev.to/rafacalderon/16-patterns-for-crossing-the-webassembly-boundary-and-the-one-that-wants-to-kill-them-all-5kb)* (2026). JS↔WASM crossing pattern catalog.

## License

MIT OR Apache-2.0
