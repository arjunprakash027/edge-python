# Edge Python DOM

A host capability that lets Edge Python scripts manipulate the DOM with the same per-op cost the JS engine's own DOM bindings pay. Distributed as a custom embedder (`dom-edge.wasm`) plus its matching JS host (`@edge-python/dom-runtime`); scripts see it as an ordinary native module:

```python
from dom import query, set_text

el = query("#app")           # u32 handle, the node never crosses
set_text(el, "hello")        # ptr + len string, one boundary crossing
```

Nodes travel as opaque `u32` handles (or `externref` when the engine supports it), strings live in WASM linear memory and are read in-place via typed-array views, and DOM operations accumulate in a binary command buffer flushed once per microtask. No JSON, no structured clone, no virtual layer.

## Architecture

The runtime keeps a generational handle table on the JS side that maps `u32` slots to real DOM nodes. A Python call like `query("#app")` returns the integer slot, not a serialised object; subsequent operations carry only that integer back across the boundary. When the engine supports `externref`, the handle is the `externref` itself and the JS GC manages lifecycle directly; otherwise the generational counter on each slot catches use-after-free on any reused index.

Strings cross by pointer and length: `compiler_lib`'s `HeapObj::Str` is already UTF-8 in linear memory, and JavaScript reads it via a `Uint8Array` view re-created per call (linear memory may grow and detach the underlying buffer). The UTF-8 → UTF-16 conversion runs natively in the engine. When the JS String Builtins proposal is available, the bridge imports JS string operations directly and the engine handles conversion at the calling-convention level.

For renders touching many nodes, operations are encoded into a tight binary stream in linear memory and drained in a single boundary crossing — the only place the bridge pays for binary layout above the encoding floor, in exchange for collapsing N crossings into 1 (the same coalescing trade-off Vulkan and Metal command buffers make). The library picks the tier from the call context: event handlers dispatch directly, render-frame callbacks batch automatically. The user never flushes manually.

## Relationship to Edge Python

`edge-python-dom` is a **host capability** in the sense defined by [Edge Python's writing-modules reference](https://docs.edgepython.com/reference/writing-modules#path-c-host-capability) — Path C. Same pattern as `print` and `input`: a Path B in-process binding shipped as part of a custom embedder, with additional host imports the embedder declares against its JS runtime. The sealed v1 [plugin ABI](https://docs.edgepython.com/reference/wasm-abi) is not touched.

## Repository layout

```bash
├── Cargo.toml
├── README.md
├── LICENSE.md
├── src
│   └── main.rs
└── tests
    └── main.rs
```

Target workspace once fleshed out: `dom-mod/` (Path B bindings linked into the custom embedder), `dom-embed/` (the embedder that produces `dom-edge.wasm`), `dom-runtime/` (npm package providing the JS host imports).

## References

1. **Haas et al.**, *[Bringing the Web up to Speed with WebAssembly](https://dl.acm.org/doi/10.1145/3062341.3062363)* (PLDI 2017). WebAssembly design and FFI rationale.
2. **Holmes**, *[When Is WebAssembly Going to Get DOM Support?](https://queue.acm.org/detail.cfm?id=3746174)* (ACM Queue 2026). WASM↔DOM bridge and glue patterns.
3. **WebAssembly CG**, *[Reference Types Proposal Overview](https://github.com/WebAssembly/reference-types/blob/master/proposals/reference-types/Overview.md)* (2020). `externref` host references.
4. **WebAssembly CG**, *[JS String Builtins Proposal Overview](https://github.com/WebAssembly/js-string-builtins/blob/main/proposals/js-string-builtins/Overview.md)* (2024). Glue-free JS string interop.
5. **Calderón**, *[16 Patterns for Crossing the WebAssembly Boundary](https://dev.to/rafacalderon/16-patterns-for-crossing-the-webassembly-boundary-and-the-one-that-wants-to-kill-them-all-5kb)* (2026). JS↔WASM crossing pattern catalog.

## License

MIT OR Apache-2.0
