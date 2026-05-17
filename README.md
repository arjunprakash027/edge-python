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
    import * as engine from "https://cdn.edgepython.com/src/engine.js";

    const base = new URL('.', import.meta.url).href;
    await engine.load({
        wasmUrl: "https://cdn.edgepython.com/compiler_lib.wasm",
        imports: { dom: base + "edge_python_dom.wasm" },
        loaders: ["https://cdn.edgepython.com/loaders/capability-bridge.js"],
    });
    await engine.run({ src: await (await fetch("./script.py")).text() });
</script>
```

DOM imports `engine.js` directly instead of the usual `createWorker` because the bridge handlers call `document.*`, which is only available on the main thread.

## Quick start

Requires Rust (`wasm32-unknown-unknown` target), Python 3.7+, and a modern browser.

```bash
git clone https://github.com/dylan-sutton-chavez/edge-python-dom
cd edge-python-dom

cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/edge_python_dom.wasm web/

python3 -m http.server 8080 --directory web
```

Open <http://127.0.0.1:8080/>.

Any static HTTP server works in place of `python3 -m http.server` — the VS Code [Live Server](https://marketplace.visualstudio.com/items?itemName=ritwickdey.LiveServer) extension is a one-click alternative (right-click `web/index.html` → "Open with Live Server").

If the wasm32 target is missing: `rustup target add wasm32-unknown-unknown`.

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

Handles are opaque integers — pass them around, never compose or compare them numerically.

## How it works

`edge_python_dom.wasm` is a thin carrier: `src/lib.rs` is 11 lines whose only job is to embed `src/bridge.js` as bytes inside the wasm binary and expose two exports — `edge_capability_bridge_ptr` and `edge_capability_bridge_len`. No Rust DOM code, no allocator, no `wasm-pdk` plumbing.

At load time, upstream's [`capability-bridge`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/runtime/loaders/capability-bridge.js) loader detects those exports, reads the embedded JS source, evals it as a factory `(rt) => handlerMap`, and registers each handler as a native function the Python VM dispatches via `host_call_native`. The bridge runs in the page (main thread), so handlers can touch `document` directly.

Adding another capability (`fs`, `crypto`, …) is the same shape: one more `.wasm` carrying its own bridge, one more entry in `imports`. Capabilities compose at load time, by URL.

## Trade-offs

**Wire-ABI overhead per op.** Every DOM call pays several WASM↔JS crossings: dispatch entry, argument decode through `host_edge_decode` (or `host_edge_view` on newer builds), encode for the return. A canonical in-process Rust binding would read `HeapObj::Str` directly and skip all that. We accepted it because: `src/lib.rs` stays at 11 lines (no wasm-pdk dispatch surface, no allocator, no panic-stash plumbing), and the bridge logic is plain JS — easier to read and evolve than Rust-with-FFI. The cost is invisible for typical UI workloads (~50–200 DOM ops per frame); it shows up in tight loops of thousands of fine-grained ops per frame (procedural animation, drag-and-drop with continuous repainting).

**Requires CSP `'unsafe-eval'`.** The loader compiles the embedded bridge with `new Function(src)`, which strict Content-Security-Policy headers block. Affected: Chrome Extensions MV3, sandboxed iframes, some bank/gov/healthcare sites. We accepted it because most web apps ship permissive CSP and work out of the box. A sidecar variant — bridge as a `.bridge.js` file referenced by URL, loaded via dynamic `import()` instead of eval — is possible if needed; not implemented today.

## Why not canonical Path C

Edge Python's reference defines [Path C](https://docs.edgepython.com/reference/writing-modules#path-c-host-capability) as a Path B in-process binding shipped as part of a custom embedder. Following it literally would mean (a) building a custom `compiler.wasm` that links `compiler_lib` as an rlib plus Rust closures bridging to `env.dom_*` host imports, and (b) calling some `register_inproc_module` API to install them. Two problems:

- **No in-process registration API upstream.** `compiler_lib` exposes `register_native_module` (which dispatches via `host_call_native`) and `register_code_module`; nothing for in-process `Vec<NativeBinding>`. Building one means replicating its WasmRuntime, handle table, ABI bridge, and panic/alloc plumbing — hundreds of lines.
- **Custom embedders don't compose.** A user who wants DOM has to use `dom-runtime`'s custom wasm. Wanting `fs` later means switching to a `dom-plus-fs-runtime` that bundles both. Capabilities can't compose at load time — they have to be planned at embedder-build time.

Path D solves both by making the capability module ship the code it needs. The host stays on vanilla upstream code; capabilities compose by URL.

## Distribution

Only `edge_python_dom.wasm` is built and served from this repo. `compiler_lib.wasm` and the JS engine both come from `cdn.edgepython.com` at page load — no vendored copy of either lives here, and no build script reaches out to fetch them.

## License

MIT OR Apache-2.0
