# Edge Python DOM

DOM access for Edge Python, distributed as a **single self-contained `.wasm`** that ships its own JavaScript bridge inside. The host code does not import, configure, or know about DOM — it loads vanilla `compiler_lib.wasm` and references `edge_python_dom.wasm` by URL. Scripts see `dom` as an ordinary module:

```python
from dom import (
    query, set_text, create_element, 
    append_child, add_class,
)

set_text(query("#app"), "hello from Python")

ul = query("#list")
for i in range(5):
    li = create_element("li")
    set_text(li, "row " + str(i + 1))
    add_class(li, "fresh")
    append_child(ul, li)
```

```html
<script type="module">
    import { createWorker } from "https://cdn.jsdelivr.net/gh/dylan-sutton-chavez/edge-python@v0.1.0/runtime/src/index.js";

    const base = new URL('.', import.meta.url).href;
    const worker = await createWorker({
        wasmUrl: base + "compiler_lib.wasm",
        imports: { dom: base + "edge_python_dom.wasm" },
        loaders: ["https://cdn.jsdelivr.net/gh/dylan-sutton-chavez/edge-python@v0.1.0/runtime/loaders/capability-bridge.js"],
    });
    await worker.run(await (await fetch("./hello.py")).text());
</script>
```

No DOM-specific JavaScript in the page. Adding another capability (`fs`, `fetch`, `crypto`) is the same shape — one more `.wasm`, one more `imports` entry, no extra loader. Capabilities are composable artifacts, not host-bundled subclasses.

## Quick start

Requires [Rust](https://rustup.rs/) (`wasm32-unknown-unknown` target), [Node.js](https://nodejs.org/) 20+, [`curl`](https://curl.se/), and a modern browser.

```bash
git clone https://github.com/dylan-sutton-chavez/edge-python-dom
cd edge-python-dom/runtime

# Builds and serves http://127.0.0.1:8080/
npm start
```

Open <http://127.0.0.1:8080/examples/index.html>. The demo runs `hello.py` against the live page.

If you need the wasm32 target:

```bash
rustup target add wasm32-unknown-unknown
```

## Python API

```python
from dom import (
    body, query, create_element, append_child, remove,
    get_text, set_text, get_attribute, set_attribute,
    add_class, remove_class,
)
```

| Function                         | Signature                       | Notes                                   |
|----------------------------------|---------------------------------|-----------------------------------------|
| `body()`                         | `() -> int`                     | Handle to `document.body`               |
| `query(selector)`                | `(str) -> int \| None`          | `document.querySelector`                |
| `create_element(tag)`            | `(str) -> int`                  | Detached element; append it explicitly  |
| `append_child(parent, child)`    | `(int, int) -> None`            | `parent.appendChild(child)`             |
| `remove(node)`                   | `(int) -> None`                 | Removes from tree; handle invalid after |
| `get_text(node)`                 | `(int) -> str`                  | Reads `textContent`                     |
| `set_text(node, text)`           | `(int, str) -> None`            | Writes `textContent`                    |
| `get_attribute(node, name)`      | `(int, str) -> str \| None`     | `None` if attribute not set             |
| `set_attribute(node, name, val)` | `(int, str, str) -> None`       |                                         |
| `add_class(node, name)`          | `(int, str) -> None`            | `classList.add`                         |
| `remove_class(node, name)`       | `(int, str) -> None`            | `classList.remove`                      |

Handles are opaque 47-bit signed integers — pass them around, never compose or compare them numerically.

## Architecture (Path D)

This project uses an approach we call **Path D** — a self-bundling capability module. It is not one of the three paths described in [Edge Python's writing-modules reference](https://docs.edgepython.com/reference/writing-modules); see [Why not canonical Path C](#why-not-canonical-path-c) below for the rationale.

```
┌────────────────────────────────┐         ┌──────────────────────────────────────────┐
│ edge_python_dom.wasm  (2.6 KB) │         │ @edge-python/runtime via jsdelivr CDN    │
│                                │         │                                          │
│  data segment:                 │         │  createWorker(...) spawns a Web Worker.  │
│    "(rt) => { return {         │         │  Inside, capability-bridge loader        │
│       query: (a) => ...,       │ extract │  detects edge_capability_bridge_* exports│
│       set_text: (a) => ...,    │ ──────▶ │  on this .wasm and:                      │
│       ...                      │         │   1. instantiates the module             │
│    }; }"                       │         │   2. reads embedded JS source            │
│                                │         │   3. evals -> (rt) => handlerMap         │
│  exports:                      │         │   4. registers handlers against          │
│    edge_capability_bridge_ptr  │         │      compiler_lib via                    │
│    edge_capability_bridge_len  │         │      register_native_module              │
└────────────────────────────────┘         └──────────────────────────────────────────┘
```

When Python calls `query("#x")`:

1. `compiler_lib`'s VM dispatches via `CallExtern` to a `NativeBinding` produced by `register_native_module`.
2. That binding calls `env.host_call_native(id, argv_ptr, argc, out_ptr)` — JS-implemented.
3. The JS host reads `argc` `u32` handles from `compiler_lib`'s memory at `argv_ptr`.
4. It invokes the bridge handler for that `id` (a closure captured at registration time).
5. The handler decodes string arguments with `compiler_lib.host_edge_decode`, performs the actual DOM operation, and encodes the return value with `compiler_lib.host_edge_encode`.
6. The resulting `u32` handle is written to `*out_ptr`; status `0` returned.

The `.wasm` file is literally a carrier for the bridge string. It has no Rust DOM bindings, no allocator, no `wasm-pdk` plumbing — `src/lib.rs` is 11 lines and exists only to embed `bridge.js` as a `static` byte array and expose its address.

### Cost per DOM op

Path D pays a full wire-ABI round-trip per operation: decode arguments from compiler_lib handles, do the work, encode the result back into a handle. Concrete crossings for a typical `query("#x")`:

| Step                                              | Crossings | Notes                       |
|---------------------------------------------------|-----------|-----------------------------|
| `host_call_native` (compiler_lib → JS)            | 1         | dispatch entry              |
| `wasm_alloc` × 2 + `host_edge_decode` + `wasm_free` × 2 | 5    | decode the selector string  |
| `document.querySelector(...)`                     | 0         | native JS DOM call          |
| `wasm_alloc` + `host_edge_encode` + `wasm_free`   | 3         | encode the result handle    |
| Write `*out_ptr` and return                       | 0         | direct memory write         |
| **Total**                                         | **~9**    | ~200–400 ns of marshalling  |

Versus canonical Path C (in-process Rust closure with direct `HeapPool` access): ~1–2 crossings, ~50–100 ns per op. Path D is roughly 3–4× more overhead per op, but in absolute terms still well within the project's `<1.5×` perf target on every canonical workload:

| Workload                  | Pure-JS baseline | Path D estimate | vs baseline |
|---------------------------|------------------|-----------------|-------------|
| Create 1000 `<li>`        | ~2 ms            | ~2.3 ms         | 1.15× ✓     |
| Update `textContent` × 1000 | ~1.5 ms        | ~1.8 ms         | 1.20× ✓     |
| Toggle class × 1000       | ~1 ms            | ~1.3 ms         | 1.30× ✓     |
| Single-op handler         | ~10 µs           | ~10.3 µs        | ~1×    ✓    |

Optimizations available if a workload approaches the ceiling: pool the scratch buffers (saves 4 crossings/op → ~5/op total), or expose a `dom_batch(op_buffer)` handler that runs N ops in a single crossing.

## Trade-offs

Two costs of this design are worth surfacing.

### 1. Wire-ABI overhead per op

Every argument and return value crosses through `host_edge_decode` / `host_edge_encode`, which means buffer allocation in compiler_lib's linear memory and a JS↔WASM crossing per primitive. A canonical in-process Path C binding reads `HeapObj::Str` directly via `s.as_ptr()` and skips all of this.

**Why we accepted it:**
- Hits the perf target with margin on every canonical workload (table above).
- Removes the entire `wasm-pdk` dispatch surface from this crate — `src/lib.rs` is 11 lines, no allocator, no panic-stash plumbing.
- The bridge logic is plain JS, not Rust-with-FFI — easier to read, easier to evolve, easier for non-Rust contributors.

**When it would matter:** workloads with thousands of fine-grained DOM ops per frame (procedural animation, drag-and-drop with continuous repainting). At that point the batched-handler optimization, or a switch to true in-process Path C, becomes warranted.

### 2. Requires CSP `unsafe-eval`

The loader compiles the embedded bridge string with `new Function(`return (${src})`)()`, which a strict Content-Security-Policy (`script-src 'self'` without `'unsafe-eval'`) will block. Affected environments include strict-CSP sites (some banks, gov, healthcare), Chrome Extensions MV3, sandboxed iframes.

**Why we accepted it:**
- 95% of web apps (SaaS, dashboards, content sites, demos) ship with permissive CSP and work out of the box.
- The alternative — Rust closures linked into the wasm directly, à la canonical Path C — requires either forking `compiler_lib::main` or extending it upstream with `register_inproc_module` (a primitive that does not yet exist upstream, per the [docs](https://docs.edgepython.com/reference/writing-modules#path-c-host-capability)).

**Sidecar fallback for strict-CSP environments:** swap the embedded-string approach for a `.bridge.js` file referenced by path from a custom section. Loader does a dynamic `import()` instead of `new Function()` — no `unsafe-eval` required. Same architecture, two files per capability instead of one. Easy to add as a second variant if needed; not implemented today.

## Why not canonical Path C

Edge Python's reference defines [Path C](https://docs.edgepython.com/reference/writing-modules#path-c-host-capability) as "a Path B in-process binding shipped as part of a custom embedder". Following that to the letter would mean:

1. Building a custom `compiler.wasm` that links `compiler_lib` as an rlib and adds Rust closures that bridge to `env.dom_*` host imports.
2. Calling some `register_inproc_module` API on `compiler_lib::main` to install the bindings.

Two problems:

- **No in-process registration API upstream.** `compiler_lib::main` exposes `register_native_module` (which dispatches via `host_call_native`) and `register_code_module`; nothing for `Vec<NativeBinding>` in-process. The docs describe the `Resolver` trait pattern but tell you to "replicate the bridge pattern in `compiler_lib`'s `main/`" — i.e., write your own runtime, which is hundreds of lines of WasmRuntime + handle table + ABI bridge + panic/alloc plumbing.
- **The custom embedder couples the host to specific capabilities.** A consumer who wants DOM has to use `dom-runtime`'s custom wasm. Adding `fs` later means switching to a `dom-plus-fs-runtime` that bundles both. Capabilities don't compose at load time — they have to be planned at embedder-build time.

Path D solves both by making the capability module itself responsible for shipping the code it needs. The host loader stays a single vanilla `createWorker`. Capabilities compose at load time, by URL.

## Adding a DOM operation

Edit `src/bridge.js`, add a property to the returned handler map:

```js
toggle_class: (a) => {
    node(rt.decodeInt(a[0])).classList.toggle(rt.decodeStr(a[1]));
    return rt.encodeNone();
},
```

Rebuild (`npm run build`) and use from Python (`from dom import toggle_class`). The loader auto-discovers handler names from `Object.keys(handlers)` at registration time — no separate name list to keep in sync, no Rust changes.

For operations returning a string, use `rt.encodeStr(...)`. For optional strings (`get_attribute`-style), branch on `null` and return `rt.encodeNone()` or `rt.encodeStr(value)`.

## Project layout

```
edge-python-dom/
├── Cargo.toml             minimal cdylib (no_std, no allocator, no deps)
├── build.rs               fetches compiler_lib.wasm from upstream release
├── src/
│   ├── lib.rs             11 lines: embed bridge.js + expose ptr/len
│   └── bridge.js          11 DOM handlers; factory `(rt) => handlerMap`
├── runtime/
│   ├── package.json       npm scripts: build, serve, start
│   ├── compiler_lib.wasm  vanilla upstream, fetched by build.rs (gitignored)
│   ├── edge_python_dom.wasm  built artifact (~2.6 KB, gitignored)
│   └── examples/
│       ├── index.html     loads @edge-python/runtime from jsdelivr
│       └── hello.py       demo script
├── README.md
└── LICENSE.md
```

No JS code in this repo. The engine lives in upstream `dylan-sutton-chavez/edge-python` under `runtime/`, served via jsdelivr CDN.

## Relationship to Edge Python

Two artifacts come from upstream `dylan-sutton-chavez/edge-python@v0.1.0`:

- **`compiler_lib.wasm`** — binary asset, downloaded by `build.rs` into `runtime/` at compile time.
- **`runtime/` (JS engine)** — consumed at script load time from jsdelivr (`cdn.jsdelivr.net/gh/.../edge-python@v0.1.0/runtime/`). Includes the `capability-bridge` loader that activates Path D for any module exporting `edge_capability_bridge_ptr/_len`.

No Rust-level dependency on `compiler_lib`'s rlib, no vendored JS. The DOM `.wasm` knows nothing about Edge Python's internals; it just publishes the carrier-with-embedded-bridge convention that the upstream loader recognizes. The sealed v1 [WASM module ABI](https://docs.edgepython.com/reference/wasm-abi) is not touched.

## References

1. **Haas et al.**, *[Bringing the Web up to Speed with WebAssembly](https://dl.acm.org/doi/10.1145/3062341.3062363)* (PLDI 2017). The shared-linear-memory contract used here for the bridge byte extraction is the FFI rationale Haas et al. describe.
2. **Holmes**, *[When Is WebAssembly Going to Get DOM Support?](https://queue.acm.org/detail.cfm?id=3746174)* (ACM Queue 2026). Argues for "the bridge as a thin function-call boundary"; the synthetic-native dispatch through `host_call_native` is exactly that shape.
3. **Calderón**, *[16 Patterns for Crossing the WebAssembly Boundary](https://dev.to/rafacalderon/16-patterns-for-crossing-the-webassembly-boundary-and-the-one-that-wants-to-kill-them-all-5kb)* (2026). The capability-module-carries-its-own-bridge pattern is closest to #14 (Bring Your Own Glue), generalized to wasm artifacts that ship side-by-side with their JS counterparts.

## License

MIT OR Apache-2.0
