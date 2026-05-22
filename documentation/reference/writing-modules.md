---
title: "Writing modules"
description: "Four paths to extend Edge Python: a `.wasm` module loaded by URL, in-process Rust closures via the Resolver trait, host packages bundled in a custom compiler, or a plain JS module that runs on the page's main thread."
---

Edge Python has no bundled stdlib. Four ways to add native functionality:

| Path | Distribution | Type coverage | Maintenance |
|---|---|---|---|
| **`.wasm` module via URL** ([WASM ABI](/reference/wasm-abi)) | Publish `.wasm` to a CDN; any host loads dynamically | Primitives only (None, bool, i64 truncated to 47-bit, f64, bytes/str) | Reference [`wasm-pdk`](https://github.com/dylan-sutton-chavez/edge-python/tree/main/wasm-pdk) (Rust), community PDKs, or hand-written wire boilerplate |
| **In-process Rust binding** | Rust crate linked as an rlib | Full — any `HeapObj` (str, list, dict, set, tuple, instance, …) | You own a crate; cargo distributes |
| **Host capability** | Custom `compiler.wasm` (Rust bindings linked in) + host runtime they bridge to | Full + host services (DOM, FS, fetch) through embedder host imports | You own embedder + host runtime; bindings travel together |
| **JS host module** | Plain ESM registered via `createWorker({ mainThreadModules })` | Primitives only (same as Path A) | Pure JS; no Rust, no `.wasm`, no build step |

`.wasm` matches the marketplace pattern (`from "https://x.wasm" import f` works in any host). In-process matches the embedder pattern (modules linked into your own `compiler.wasm`). Host-capability is in-process applied recursively — the embedder is a runtime distribution and its bindings are part of what it offers (the same pattern `print` and `input` use). JS host modules keep upstream `compiler_lib.wasm` untouched while exposing main-thread surface (DOM, dialogs, FileReader, observers, anything `window.*`). All four are first-class.

## Path A: `.wasm` module by URL

Contract: the [WASM module ABI](/reference/wasm-abi) — language-agnostic, three scalar types. Rust authors use the bundled [`wasm-pdk`](https://github.com/dylan-sutton-chavez/edge-python/tree/main/wasm-pdk) (`#[plugin_fn]`, typed `Handle` / `Value` / `Error`); other languages use community PDKs or hand-roll the boilerplate.

Worked examples (with and without the SDK), encoding tables, and language-specific snippets: [WASM module ABI](/reference/wasm-abi). Script side:

```python
from "./my_edge_mod.wasm" import add
print(add(2, 3))   # -> 5
```

## Path B: in-process Rust binding

For embedders linking `compiler_lib` as an rlib: native bindings are Rust closures handed to the parser via the `Resolver` trait. Closures get direct access to the VM heap (strings, lists, dicts, sets, tuples, instances, modules) — zero serialization, no wire format, no primitive-only ceiling.

Bindings declare a `pure` flag (referentially transparent, no side effects). Pure bindings can be memoised by the VM's template cache; impure always run.

### Module crate

`text/Cargo.toml`:

```toml
[package]
name = "text"
version = "0.1.0"
edition = "2024"

[dependencies]
compiler-lib = { git = "https://github.com/dylan-sutton-chavez/edge-python", branch = "main" }
```

`text/src/lib.rs`:

```rust
use compiler_lib::modules::packages::NativeBinding;
use compiler_lib::modules::vm::types::{HeapObj, HeapPool, Val, VmErr};

fn upper(heap: &mut HeapPool, args: &[Val]) -> Result<Val, VmErr> {
    let s = match heap.get(args[0]) {
        HeapObj::Str(s) => s.clone(),
        _ => return Err(VmErr::Type("upper: expected str")),
    };
    heap.alloc(HeapObj::Str(s.to_uppercase()))
}

pub fn module() -> Vec<NativeBinding> {
    vec![NativeBinding::from_fn("upper", upper, true)]
}
```

### Embedder crate

Your custom `compiler.wasm` (or native binary) links `compiler_lib` plus every module crate and implements `Resolver`.

`embedder/Cargo.toml`:

```toml
[package]
name = "embedder"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
compiler-lib = { git = "https://github.com/dylan-sutton-chavez/edge-python", branch = "main" }
text = { path = "../text" }
```

`embedder/src/lib.rs`:

```rust
use compiler_lib::modules::packages::{Resolved, Resolver};

pub struct AppResolver;

impl Resolver for AppResolver {
    fn resolve(&mut self, spec: &str) -> Result<Resolved, String> {
        match spec {
            "text" => Ok(Resolved::Native(text::module())),
            // Compose more modules: "json" => Ok(Resolved::Native(json::module())),
            _ => Err(format!("unknown module: {}", spec)),
        }
    }
}
```

Plug `AppResolver` into your parser entry point (replicate the bridge pattern in `compiler_lib`'s `main/`, or call `Parser::with_resolver` directly for native binaries). Build and use:

```bash
cargo build --release --target wasm32-unknown-unknown -p embedder
# Serve embedder.wasm in place of compiler_lib.wasm.
```

```python
from text import upper
print(upper("hello")) # -> HELLO
```

### Type cookbook (in-process only)

The closure signature is `Fn(&mut HeapPool, &[Val]) -> Result<Val, VmErr>`. Inside it:

```rust
// Reading args
let n: i64 = if args[0].is_int() { args[0].as_int() } else { return Err(VmErr::Type("expected int")); };
let f: f64 = if args[0].is_float() { args[0].as_float() } else { return Err(VmErr::Type("expected float")); };
let b: bool = if args[0].is_bool() { args[0].as_bool() } else { return Err(VmErr::Type("expected bool")); };
let s: String = match heap.get(args[0]) {
    HeapObj::Str(s) => s.clone(),
    _ => return Err(VmErr::Type("expected str")),
};
let items: Vec<Val> = match heap.get(args[0]) {
    HeapObj::List(rc) => rc.borrow().clone(),
    _ => return Err(VmErr::Type("expected list")),
};

// Returning values
Ok(Val::int(42)) // scalar — no allocation
Ok(Val::bool(true))
Ok(Val::none())
heap.alloc(HeapObj::Str("hi".into())) // heap-allocated str
heap.alloc(HeapObj::List(std::rc::Rc::new(std::cell::RefCell::new(vec![Val::int(1), Val::int(2)]))))
heap.alloc(HeapObj::Tuple(vec![Val::int(1), Val::bool(true)]))

// Errors -> surface in scripts as the corresponding Python exception class
Err(VmErr::Type("expected str")) // -> TypeError
Err(VmErr::Value("empty separator")) // -> ValueError
Err(VmErr::Runtime("network unavailable")) // -> RuntimeError
Err(VmErr::TypeMsg(format!("got {:?}", v))) // dynamically formatted
```

## Path C: host capability

Some native functionality can't live in a CDN-distributed `.wasm` (Path A) because the work happens outside the WASM sandbox — DOM mutation, WASI filesystem I/O, native crypto. Path A modules see only the sealed 6 `env.*` imports; they have no channel to the host runtime. Path C closes that gap.

A host capability is a Path B in-process binding shipped as part of a custom embedder. The Rust closure runs inside the embedder's `compiler.wasm` and bridges to the host runtime through additional host imports the embedder declares — not part of the sealed plugin ABI; the embedder's private contract with its host.

Precedent: `print(...)` calls the embedder's `host_print` import; `input()` drains a buffer the host fills via `set_input`. The same shape generalises — a browser-host distribution can register `dom` as a native module whose `query`, `set_text`, `append_child` closures bridge to JS through embedder-specific host imports. A WASI-host distribution can register `fs` against `wasi_snapshot_preview1`. Scripts see them as ordinary native modules:

```python
from dom import document, query     # browser host
from fs  import read_text, write    # WASI host
```

### What ships in a host-capability distribution

| Artifact | Role |
|---|---|
| Custom `compiler.wasm` | Vanilla `compiler_lib` + Path B bindings; declares additional host imports |
| Host runtime | Browser shim / WASI loader / native binary that provides those imports |
| Pure-Python wrappers (`.py`) (optional) | Ergonomic surface on top of raw bindings, shipped as a code module |

Users opt in by loading the custom `compiler.wasm` and matching host runtime together. Vanilla `compiler.wasm` keeps working for everyone else.

### Sketch

```rust
// dom-mod/src/lib.rs — Path B binding that bridges to JS
use compiler_lib::modules::packages::NativeBinding;
use compiler_lib::modules::vm::types::{HeapObj, HeapPool, Val, VmErr};

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn host_dom_op(opcode: u32, ptr: *const u8, len: u32) -> u32;
}

fn query(heap: &mut HeapPool, args: &[Val]) -> Result<Val, VmErr> {
    let sel = match heap.get(args[0]) {
        HeapObj::Str(s) => s.clone(),
        _ => return Err(VmErr::Type("query: expected str")),
    };
    let handle = unsafe { host_dom_op(OP_QUERY, sel.as_ptr(), sel.len() as u32) };
    Ok(heap.alloc(HeapObj::Instance(/* DOM element wrapper, holds `handle` */)))
}

pub fn module() -> Vec<NativeBinding> {
    vec![NativeBinding::from_fn("query", query, false), /* ... */]
}
```

The custom `compiler.wasm` declares `env.host_dom_op` alongside the standard `env.host_print` / `env.host_fetch_bytes` / `env.host_call_native`. The host runtime supplies the implementation.

### Why this is not a third module flavor

Scripts still see two flavors (code and native — see [Imports](/reference/imports)). Path C is a distribution pattern over Path B, not a new dispatch path: the compiler sees `Resolved::Native(bindings)` like any other; the bindings happen to bridge externally. Keeps the public language surface and the [WASM module ABI](/reference/wasm-abi) untouched.

## Path D: JS host module

Browsers run the engine in a Web Worker (no `document`, no `window`). Path D bridges: a capability ships as plain JavaScript, registers with `createWorker({ mainThreadModules })`, runs on the main thread. The runtime synthesises the native module registration so Python can `from <name> import ...`; each call is decoded in the Worker, shipped to main via `postMessage`, executed against `document`/`window`/etc., and the result encoded back. Python sees a synchronous call.

No `.wasm`, no Rust, no build step.

### Sketch

A module is a factory `(ctx) => handlers` (or `{name: handler}`). The factory receives `{ pushEvent }` so async callbacks (event listeners, observers, `FileReader`, animation `finished`) can wake a paused `receive()`.

```js
// dom.js
export const dom = ({ pushEvent }) => {
    const nodes = [];
    const alloc = (n) => { if (n == null) return -1; nodes.push(n); return nodes.length - 1; };
    const node = (h) => nodes[h];

    return {
        query: (sel) => alloc(document.querySelector(sel)),
        set_text: (h, txt) => { node(h).textContent = txt; },
        bind_event: (h, type, msg) => {
            node(h).addEventListener(type, (e) => {
                pushEvent(JSON.stringify({ msg, type: e.type, target_id: e.target.id }));
            });
        },
    };
};
```

```html
<script type="module">
    import { createWorker } from "https://runtime.edgepython.com/js/src/index.js";
    import { dom } from "./dom.js";

    const worker = await createWorker({
        wasmUrl: "https://runtime.edgepython.com/js/compiler_lib.wasm",
        mainThreadModules: { dom },
    });
    await worker.run(await (await fetch("./script.py")).text());
</script>
```

```python
from dom import query, set_text, bind_event
bind_event(query("#btn"), "click", "click")
async def main():
    while True:
        receive()
        set_text(query("#btn"), "clicked")
run(main())
```

Handlers take decoded JS values and return plain JS values. Supported tags: `None`, `bool`, `int` (i64, range-limited by JS Number), `float`, string bytes. Opaque object references (DOM nodes, files, observers) model as integer IDs into a main-thread registry the handlers own (the `alloc` / `node` pattern above).

### Trade-offs vs Path C

| | Path C | Path D |
|---|---|---|
| Compiler artifact | Custom per capability set | Vanilla upstream |
| Composition | Embed-time | Load-time, by import |
| Binding language | Rust closures, full `HeapObj` access | JavaScript, primitives only |
| Per-op overhead | Direct `Val` reads | `postMessage` round-trip (~0.1–0.4 ms) |
| Threading model | Wherever the embedder runs | Main thread (handlers reach `document`) |
| Build pipeline | `cargo` | None |

Pick Path D when the capability needs main-thread browser surface (DOM, dialogs, observers, FileReader) and per-op latency is acceptable — invisible for UI-rate workloads (~50–200 ops/frame). Reach for Path C when tight per-frame loops dominate or you need full `HeapObj` access.

Reference implementation: [`edge-python-host`](https://github.com/dylan-sutton-chavez/edge-python-host).

## Choosing between the four paths

| You want… | Use |
|---|---|
| Publish a module any Edge Python user can `from "<url>" import` without rebuilding | Path A (`.wasm` ABI) |
| Maximum speed and full type coverage (strings, lists, etc.) | Path B (in-process Rust) |
| Wrap a C/Zig/AS library | Path A (any wasm32-targeting language works) |
| Plug into a custom Rust app and expose its APIs | Path B |
| Expose host services (DOM, FS, native crypto) bundled into your own runtime distribution | Path C (host capability) |
| Expose browser-main-thread APIs (DOM, dialogs, observers) without shipping a custom embedder | Path D (JS host module) |
| Both at once | Compose: your embedder's `Resolver` can return `Resolved::Native(...)` for in-process modules AND let `.wasm` URL imports flow through to the bridge |

## Pure vs impure

`pure: true` (in-process only) lets the VM memoise — repeated calls with the same args skip execution. Mark functions pure when they depend only on their args. `.wasm`-loaded bindings default to `pure: false` (host can't introspect their semantics).

## See also

- [WASM module ABI](/reference/wasm-abi) — the wire format spec for Path A.
- [Imports](/reference/imports) — script-side semantics, packages.json, integrity verification.
- [`compiler/src/modules/packages/mod.rs`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/compiler/src/modules/packages/mod.rs) — full `Resolver` trait + `NativeBinding` struct.
