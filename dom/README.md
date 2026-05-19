# Edge Python DOM

DOM access for Edge Python, distributed as a single self-contained `.wasm`. The host loads vanilla `compiler_lib.wasm` and references `edge_python_dom.wasm` by URL; Python scripts see `dom` as an ordinary module.

```python
from dom import query, set_text, create_element, append_child, add_class

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
    import * as engine from "https://runtime.edgepython.com/js/src/engine.js";

    const base = new URL('.', import.meta.url).href;
    await engine.load({
        wasmUrl: "https://runtime.edgepython.com/js/compiler_lib.wasm",
        imports: { dom: base + "edge_python_dom.wasm" },
        loaders: ["https://runtime.edgepython.com/js/loaders/capability-bridge.js"],
    });
    await engine.run({ src: await (await fetch("./script.py")).text() });
</script>
```

DOM imports `engine.js` directly instead of the usual `createWorker` because the bridge handlers call `document.*`, which is only available on the main thread.

## Quick start

Requires Rust (`wasm32-unknown-unknown` target), Python 3.7+, and a modern browser.

```bash
git clone https://github.com/dylan-sutton-chavez/edge-python-capabilities
cd edge-python-capabilities

cargo build --release
python3 -m http.server 8080
```

Open <http://127.0.0.1:8080/dom/web/>.

The repo is a Cargo workspace — `cargo build --release` from the root builds every capability member and drops artifacts in the shared `target/` at the workspace root. `dom/web/index.html` references `../../target/wasm32-unknown-unknown/release/edge_python_dom.wasm` directly, so no copy step is needed — the static server has to run from the repo root, not from `dom/web/`.

If the wasm32 target is missing: `rustup target add wasm32-unknown-unknown`.

## Python API

```python
from dom import (
    body, query, create_element, append_child, insert_before, remove,
    get_text, set_text, get_attribute, set_attribute,
    add_class, remove_class, bind_event,
)
```

| Function                            | Signature                       | Notes                                   |
|-------------------------------------|---------------------------------|-----------------------------------------|
| `body()`                            | `() -> int`                     | Handle to `document.body`               |
| `query(selector)`                   | `(str) -> int \| None`          | `document.querySelector`                |
| `create_element(tag)`               | `(str) -> int`                  | Detached element; append it explicitly  |
| `append_child(parent, child)`       | `(int, int) -> None`            | `parent.appendChild(child)`             |
| `insert_before(new, ref)`           | `(int, int) -> None`            | `ref.parentNode.insertBefore(new, ref)` |
| `remove(node)`                      | `(int) -> None`                 | Removes from tree; handle invalid after |
| `get_text(node)`                    | `(int) -> str`                  | Reads `textContent`                     |
| `set_text(node, text)`              | `(int, str) -> None`            | Writes `textContent`                    |
| `get_attribute(node, name)`         | `(int, str) -> str \| None`     | `None` if attribute not set             |
| `set_attribute(node, name, val)`    | `(int, str, str) -> None`       |                                         |
| `add_class(node, name)`             | `(int, str) -> None`            | `classList.add`                         |
| `remove_class(node, name)`          | `(int, str) -> None`            | `classList.remove`                      |
| `bind_event(node, event, message)`  | `(int, str, str) -> None`       | `addEventListener` wrap; on fire, dispatches a `CustomEvent` the runtime routes to `receive()` |

Every entry is a 1-to-1 wrap of a native DOM method; behavioural composition is the consumer Python's job.

Handles are opaque integers — pass them around, never compose or compare them numerically.

## How it works

`edge_python_dom.wasm` is a thin carrier: `dom/src/lib.rs` is 11 lines whose only job is to embed `dom/src/bridge.js` as bytes inside the wasm binary and expose two exports — `edge_capability_bridge_ptr` and `edge_capability_bridge_len`. No Rust DOM code, no allocator, no `wasm-pdk` plumbing.

At load time, upstream's [`capability-bridge`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/runtime/loaders/capability-bridge.js) loader detects those exports, reads the embedded JS source, evals it as a factory `(rt) => handlerMap`, and registers each handler as a native function the Python VM dispatches via `host_call_native`. The bridge runs in the page (main thread), so handlers can touch `document` directly.

Adding another capability (`fs`, `crypto`, …) is the same shape: one more `.wasm` carrying its own bridge, one more entry in `imports`. Capabilities compose at load time, by URL.

## Performance

### What is a DOM node

A **node** is any element in the document tree — a `<div>`, a `<span>`, a text run, an attribute. When this README says "create 500 nodes", it means 500 `create_element` operations, each one followed by some attribute / text / parent wiring. As reference, here is the node footprint of common pages:

| Page                              | Approx node count |
|-----------------------------------|-------------------|
| Static landing                    | 100–500           |
| SPA dashboard / feed              | 3.000–10.000      |
| Twitter timeline, Gmail open      | 10.000–20.000     |
| Google Maps with UI loaded        | 20.000+           |

What matters for 60fps is **not** the total node count of the document — that is one-time work at page load. What matters is **how many nodes your code touches per frame** (every 16.67ms): create, mutate, remove, or read.

### Current pipeline (per `engine.run` call)

For a single Python script that creates `N` nodes via this bridge, measured on a **debug** build of `compiler_lib.wasm`. The release artifact served from `runtime.edgepython.com` (built with `wasm-opt -O3`) runs the bridge ~3–5× faster — see the multiplier note at the end of this section.

```
                                       cost (approx)
  ┌─────────────────────────────────┐
  │ 1. JS → wasm: write source      │   < 0.1 ms     (cheap memcpy)
  │ 2. WebAssembly.instantiate       │   ~ 0.5–1 ms   (per run, fresh VM)
  │ 3. Python parse + compile        │   ~ 1–2 ms     (script-size dependent)
  │ 4. BFS prefetch + module resolve │   < 0.5 ms     (no extras for `dom` only)
  │ 5. Python execution              │   N × 4 × 2.75 µs    ← bridge dominates
  │    (4 bridge calls per node:                                    │
  │     create_element, set_attribute,                              │
  │     set_text, append_child)                                     │
  │ 6. Return path: JS gets control  │   < 0.1 ms                   │
  └─────────────────────────────────┘
  ─── after engine.run returns ────
  │ 7. Browser layout + paint        │   ~ 1–3 ms     (N-dependent, optimized)
```

Concretely, for the palette demo (`N = 6`):
- Steps 1–4: ~3 ms (one-time per run)
- Step 5: 6 × 4 × 2.75 µs ≈ 0.07 ms (negligible)
- Step 7: <1 ms
- **Total wall time: ~4 ms** — well under one frame at 60fps.

For a heavier scenario (`N = 500`):
- Steps 1–4: ~3 ms
- Step 5: 500 × 4 × 2.75 µs ≈ 5.5 ms
- Step 7: ~2 ms
- **Total wall time: ~10 ms** — still inside one frame.

### FPS budget math

The number you care about is **nodes touched per frame** at a target framerate. Each frame must fit:

```
T_frame ≥ T_script + T_paint

T_script   = T_fixed_overhead + N × bridge_calls_per_node × bridge_cost
           ≈ 3 ms + N × 4 × 2.75 µs
           ≈ 3 ms + N × 0.011 ms

T_paint    ≈ 1 ms + N × 0.005 ms      (rough; depends on layout complexity)

T_frame    ≤ 16.67 ms      (60fps)
           ≤ 33.33 ms      (30fps)
```

Solving for `N` at 60fps with a `2 ms` paint reserve and a `3 ms` engine.run overhead:

```
N_max(60fps) = (16.67 − 2 − 3) / 0.011 ≈ 1.060 nodes/frame
N_max(30fps) = (33.33 − 4 − 3) / 0.011 ≈ 2.400 nodes/frame
```

If you eliminate the `engine.run` overhead by running everything inside a single long-lived script (no per-frame Python re-parse), the formula loses the `3 ms` constant:

```
N_max(60fps, long-lived) = (16.67 − 2) / 0.011 ≈ 1.330 nodes/frame
```

The CDN-served `compiler_lib.wasm` is a `release` + `wasm-opt -O3` build that reduces `bridge_cost` by ~3–5×. With `bridge_cost ≈ 0.7 µs` and 4 calls per node, the per-node cost drops to `~3 µs` and the budget triples.

### What this is good for

| Workload                                                  | Verdict          |
|-----------------------------------------------------------|------------------|
| Static + interactive UIs (forms, dialogs, dashboards)     | trivial 60fps    |
| Reactive lists / feeds touching ~200 nodes per update     | comfortable 60fps|
| Real-time charts updating ~500 data points per frame      | clean 60fps      |
| Drag-and-drop with live repositioning                     | 60fps if you avoid full rebuilds; 30fps otherwise |
| Procedural animations (rebuild ~1.000 nodes/frame)        | 30–60fps         |
| Particle systems / canvas-style renders via DOM nodes     | **don't** — use `<canvas>` |

Rule of thumb: **if a frame touches fewer than ~1.000 nodes** in steady state on a debug build (or ~3.000 on release), Python-via-DOM is indistinguishable from native JS for the user.

## Trade-offs

**Wire-ABI overhead per op.** Every DOM call pays several WASM↔JS crossings: dispatch entry, argument decode through `host_edge_decode` (or `host_edge_view` on newer builds), encode for the return. A canonical in-process Rust binding would read `HeapObj::Str` directly and skip all that. We accepted it because: `src/lib.rs` stays at 11 lines (no wasm-pdk dispatch surface, no allocator, no panic-stash plumbing), and the bridge logic is plain JS — easier to read and evolve than Rust-with-FFI. The cost is invisible for typical UI workloads (~50–200 DOM ops per frame); it shows up in tight loops of thousands of fine-grained ops per frame (procedural animation, drag-and-drop with continuous repainting).

**Requires CSP `'unsafe-eval'`.** The loader compiles the embedded bridge with `new Function(src)`, which strict Content-Security-Policy headers block. Affected: Chrome Extensions MV3, sandboxed iframes, some bank/gov/healthcare sites. We accepted it because most web apps ship permissive CSP and work out of the box. A sidecar variant — bridge as a `.bridge.js` file referenced by URL, loaded via dynamic `import()` instead of eval — is possible if needed; not implemented today.

## Why not canonical Path C

Edge Python's reference defines [Path C](https://docs.edgepython.com/reference/writing-modules#path-c-host-capability) as a Path B in-process binding shipped as part of a custom embedder. Following it literally would mean (a) building a custom `compiler.wasm` that links `compiler_lib` as an rlib plus Rust closures bridging to `env.dom_*` host imports, and (b) calling some `register_inproc_module` API to install them. Two problems:

- **No in-process registration API upstream.** `compiler_lib` exposes `register_native_module` (which dispatches via `host_call_native`) and `register_code_module`; nothing for in-process `Vec<NativeBinding>`. Building one means replicating its WasmRuntime, handle table, ABI bridge, and panic/alloc plumbing — hundreds of lines.
- **Custom embedders don't compose.** A user who wants DOM has to use `dom-runtime`'s custom wasm. Wanting `fs` later means switching to a `dom-plus-fs-runtime` that bundles both. Capabilities can't compose at load time — they have to be planned at embedder-build time.

Path D solves both by making the capability module ship the code it needs. The host stays on vanilla upstream code; capabilities compose by URL.

## Distribution

Only `edge_python_dom.wasm` is built and served from this repo. `compiler_lib.wasm` and the JS engine both come from `runtime.edgepython.com` at page load — no vendored copy of either lives here, and no build script reaches out to fetch them.

## License

MIT OR Apache-2.0
