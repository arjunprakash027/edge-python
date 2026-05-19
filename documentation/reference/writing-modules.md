---
title: "Writing modules"
description: "Four paths to extend Edge Python: a `.wasm` module loaded by URL, in-process Rust closures via the Resolver trait, host capabilities the runtime ships as part of itself, or a self-contained capability `.wasm` that carries its own JS bridge."
---

Edge Python has no bundled stdlib. There are **four ways** to add native functionality. Pick the one that fits your distribution model.

| Path | Distribution | Type coverage | Maintenance |
|---|---|---|---|
| **`.wasm` module via URL** ([WASM ABI](/reference/wasm-abi)) | Publish a `.wasm` to a CDN; any host loads it dynamically | Primitives only (None, bool, i64 truncated to 47-bit, f64, bytes/str) | Use the reference [`wasm-pdk`](https://github.com/dylan-sutton-chavez/edge-python/tree/main/wasm-pdk) crate (Rust), a community PDK for your language, or hand-write the wire-format boilerplate |
| **In-process Rust binding** | Publish a Rust crate; embedders link it as an rlib | Full — any `HeapObj` (str, list, dict, set, tuple, instance, …) | You own a Rust crate; cargo handles distribution |
| **Host capability** | Ship a custom `compiler.wasm` (in-process Rust bindings linked in) plus the host-side runtime they bridge to | Full — same as in-process bindings, plus access to host services (DOM, FS, fetch) through the embedder's host imports | You own the custom embedder and its host runtime; the bindings travel together |
| **Self-contained capability `.wasm`** | Publish a single `.wasm` that embeds its own JS bridge as bytes; the vanilla upstream `compiler_lib.wasm` loads it by URL alongside any other capability | Primitives only (same as Path A) — handlers go through `host_call_native` | You own one `.wasm` per capability; capabilities compose by URL at load time |

The `.wasm` path matches the marketplace pattern (`from "https://x.wasm" import f` works in any host). The in-process path matches the embedder pattern (compile your modules into your own `compiler.wasm`). The host-capability path is the in-process path applied recursively: the embedder *is* a runtime distribution, and the bindings it ships are part of what that runtime offers — exactly the way `print` and `input` already work. The self-contained-capability path keeps the upstream `compiler_lib.wasm` untouched and lets capabilities compose by URL instead of by recompilation. All four are first-class.

## Path A: `.wasm` module by URL

The contract is the [WASM module ABI](/reference/wasm-abi) — short, language-agnostic, three scalar types. For Rust, the bundled [`wasm-pdk`](https://github.com/dylan-sutton-chavez/edge-python/tree/main/wasm-pdk) crate is the reference author-side layer (`#[plugin_fn]`, typed `Handle` / `Value` / `Error`); for other languages, use a community PDK or write the boilerplate by hand.

Complete worked examples (with and without the SDK), encoding tables, and language-specific snippets live in [WASM module ABI](/reference/wasm-abi). The script side is just:

```python
from "./my_edge_mod.wasm" import add
print(add(2, 3))   # -> 5
```

## Path B: in-process Rust binding

For embedders that link `compiler_lib` as an rlib, native bindings are Rust closures the host hands the parser through the `Resolver` trait. Closures get **direct access to the VM heap** — strings, lists, dicts, sets, tuples, instances, modules — with zero serialization. There is no wire format, no marshalling overhead, no primitive-only ceiling.

Bindings declare a `pure` flag (true if the function is referentially transparent — same args produce same result, no side effects). Pure bindings can be memoised by the VM's template cache; impure bindings always run.

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

The embedder is your custom `compiler.wasm` (or native binary). It links `compiler_lib` plus every module crate you want available, and implements `Resolver`.

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

Plug `AppResolver` into your parser entry point (replicate the bridge pattern in `compiler_lib`'s `main/`, or call `Parser::with_resolver` directly for native binaries).

Build and use:

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

Some native functionality cannot live in a CDN-distributed `.wasm` (Path A) because the work happens **outside** the WASM sandbox — DOM mutation in a browser, filesystem I/O on WASI, native crypto on a Rust host. Path A `.wasm` modules only see the sealed 6 `env.*` imports; they have no channel to the host runtime. Path C closes that gap.

A **host capability** is a Path B in-process binding shipped as part of a custom embedder. The Rust closure runs inside the embedder's `compiler.wasm` and bridges to the host runtime through additional host imports that the embedder itself declares — these imports are **not** part of the sealed plugin ABI; they are the embedder's private contract with its host.

Precedent already in the language:

- `print(...)` is a built-in that calls the embedder's `host_print` import. The host runtime (browser shim, WASI runtime, native binary) implements `host_print` against its native output channel.
- `input()` drains a buffer the host fills via `set_input`.

The same shape generalises. A browser-host distribution can register `dom` as a native module whose `query`, `set_text`, `append_child` closures bridge to a JS-side runtime through embedder-specific host imports. A WASI-host distribution can register `fs` the same way against `wasi_snapshot_preview1`. Scripts see them as ordinary native modules:

```python
from dom import document, query     # browser host
from fs  import read_text, write    # WASI host
```

### What ships in a host-capability distribution

| Artifact | Role |
|---|---|
| Custom `compiler.wasm` | Vanilla `compiler_lib` plus the Path B bindings linked in; declares the additional host imports the bindings need |
| Host runtime | The browser shim / WASI loader / native binary that provides those host imports |
| (Optional) Pure-Python wrappers (`.py`) | Ergonomic surface on top of the raw bindings, distributed as a code module |

Users opt in by loading the custom `compiler.wasm` and matching host runtime together (typically as a single package). Vanilla `compiler.wasm` keeps working for everyone who doesn't need the capability.

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

The custom `compiler.wasm` declares `env.host_dom_op` alongside the standard `env.host_print` / `env.host_fetch_bytes` / `env.host_call_native`. The host runtime supplies its implementation.

### Why this is not a third module flavor

From the script's perspective there are still **two flavors** (code and native — see [Imports](/reference/imports)). Path C is a distribution pattern over Path B, not a new dispatch path. The compiler sees a `Resolved::Native(bindings)` like any other; the bindings happen to bridge externally. This keeps the public language surface and the [WASM module ABI](/reference/wasm-abi) untouched.

## Path D: self-contained capability `.wasm`

Path C needs a **custom `compiler.wasm`**: every capability mix is a separate embedder build. Path D removes that step. The capability ships as a **single `.wasm`** that the vanilla upstream `compiler_lib.wasm` loads by URL, exactly like a Path A module. Capabilities compose at load time by listing them in the runtime's `imports` map.

The `.wasm` is a thin carrier — it embeds plain JavaScript inside its linear memory and exports two pointers to it: `edge_capability_bridge_ptr` and `edge_capability_bridge_len`. The [`capability-bridge`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/runtime/loaders/capability-bridge.js) loader (opted in at `engine.load()`) detects those exports, evals the source as a factory `(rt) => handlerMap`, and registers every handler as a native binding dispatched via `host_call_native` — the same path Path A natives use, so no new ABI.

Handlers run on the page's **main thread** (the bridge loader is wired into `engine.js` directly, not the worker), giving them access to `document`, `localStorage`, `crypto.subtle`, and anything else only the main thread sees.

```html
<script type="module">
    import * as engine from "https://runtime.edgepython.com/js/src/engine.js";

    await engine.load({
        wasmUrl: "https://runtime.edgepython.com/js/compiler_lib.wasm",
        imports: { dom: "https://cdn.example.com/edge_python_dom.wasm" },
        loaders: ["https://runtime.edgepython.com/js/loaders/capability-bridge.js"],
    });
    await engine.run({ src: await (await fetch("./script.py")).text() });
</script>
```

```python
from dom import query, set_text
set_text(query("#app"), "hello from Python")
```

### Sketch

`src/lib.rs` is all the Rust there is:

```rust
#![no_std]
#![no_main]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

static BRIDGE: &[u8] = include_bytes!("bridge.js");

#[unsafe(no_mangle)] pub extern "C" fn edge_capability_bridge_ptr() -> *const u8 { BRIDGE.as_ptr() }
#[unsafe(no_mangle)] pub extern "C" fn edge_capability_bridge_len() -> u32 { BRIDGE.len() as u32 }
```

`src/bridge.js` holds the actual logic — a factory returning one JS function per Python entry point:

```js
(rt) => {
    const handles = new Map();
    let next = 1;
    const wrap = (n) => { const id = next++; handles.set(id, n); return id; };

    return {
        query: (sel) => {
            const el = document.querySelector(rt.readStr(sel));
            return el ? wrap(el) : null;
        },
        set_text: (h, t) => { handles.get(rt.readInt(h)).textContent = rt.readStr(t); },
    };
}
```

`rt` exposes the helpers (`readStr`, `readInt`, `alloc`, …) for crossing the JS↔compiler memory boundary. The compiler doesn't know — or care — that the implementation is JS rather than a wasm-pdk `.wasm`.

### Trade-offs vs Path C

| | Path C | Path D |
|---|---|---|
| Compiler artifact | Custom per capability set | Vanilla upstream |
| Composition | Embed-time | Load-time, by URL |
| Binding language | Rust closures, full `HeapObj` access | JS handlers via wire ABI (primitives only) |
| Per-op overhead | Direct `Val` reads | Encode/decode per arg |
| Threading model | Wherever the embedder runs | Main thread (handlers reach `document`) |
| CSP requirement | None beyond standard | Requires `'unsafe-eval'` (loader uses `new Function`) |

Path D is the right pick when load-time composition and main-thread host access matter more than per-op throughput — invisible for typical UI work (~50–200 ops/frame), visible only in tight loops of thousands of fine-grained ops per frame. Reach for Path C when those loops dominate, or when strict CSP rules out `'unsafe-eval'`.

A reference implementation lives in [`edge-python-capabilities`](https://github.com/dylan-sutton-chavez/edge-python-capabilities) — currently exposing `dom`. Adding a capability is one more workspace member and one more entry in the consumer's `imports` map; no changes to upstream `compiler_lib` or the runtime.

## Choosing between the four paths

| You want… | Use |
|---|---|
| Publish a module any Edge Python user can `from "<url>" import` without rebuilding | Path A (`.wasm` ABI) |
| Maximum speed and full type coverage (strings, lists, etc.) | Path B (in-process Rust) |
| Wrap a C/Zig/AS library | Path A (any wasm32-targeting language works) |
| Plug into a custom Rust app and expose its APIs | Path B |
| Expose host services (DOM, FS, native crypto) bundled into your own runtime distribution | Path C (host capability) |
| Expose host services as a CDN-distributed `.wasm` users can mix and match without a custom embedder | Path D (self-contained capability `.wasm`) |
| Both at once | Compose: your embedder's `Resolver` can return `Resolved::Native(...)` for in-process modules AND let `.wasm` URL imports flow through to the bridge |

## Pure vs impure

`pure: true` (in-process bindings only) lets the VM memoize the result — repeated calls with the same args skip execution. Mark functions pure when they depend only on their args. `.wasm`-loaded bindings default to `pure: false` since the host can't introspect their semantics.

## See also

- [WASM module ABI](/reference/wasm-abi) — the wire format spec for Path A.
- [Imports](/reference/imports) — script-side semantics, packages.json, integrity verification.
- [`compiler/src/modules/packages/mod.rs`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/compiler/src/modules/packages/mod.rs) — full `Resolver` trait + `NativeBinding` struct.
