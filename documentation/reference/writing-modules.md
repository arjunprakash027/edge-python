---
title: "Writing modules"
description: "Three paths to extend Edge Python: a `.wasm` module loaded by URL, in-process Rust closures via the Resolver trait, or host capabilities the runtime ships as part of itself."
---

Edge Python has no bundled stdlib. There are **three ways** to add native functionality. Pick the one that fits your distribution model.

| Path | Distribution | Type coverage | Maintenance |
|---|---|---|---|
| **`.wasm` module via URL** ([WASM ABI](/reference/wasm-abi)) | Publish a `.wasm` to a CDN; any host loads it dynamically | Primitives only (None, bool, i64 truncated to 47-bit, f64, bytes/str) | Use the reference [`wasm-pdk`](https://github.com/dylan-sutton-chavez/edge-python/tree/main/wasm-pdk) crate (Rust), a community PDK for your language, or hand-write the wire-format boilerplate |
| **In-process Rust binding** | Publish a Rust crate; embedders link it as an rlib | Full — any `HeapObj` (str, list, dict, set, tuple, instance, …) | You own a Rust crate; cargo handles distribution |
| **Host capability** | Ship a custom `compiler.wasm` (in-process Rust bindings linked in) plus the host-side runtime they bridge to | Full — same as in-process bindings, plus access to host services (DOM, FS, fetch) through the embedder's host imports | You own the custom embedder and its host runtime; the bindings travel together |

The `.wasm` path matches the marketplace pattern (`from "https://x.wasm" import f` works in any host). The in-process path matches the embedder pattern (compile your modules into your own `compiler.wasm`). The host-capability path is the in-process path applied recursively: the embedder *is* a runtime distribution, and the bindings it ships are part of what that runtime offers — exactly the way `print` and `input` already work. All three are first-class.

## Path A: `.wasm` module by URL

The contract is the [WASM module ABI](/reference/wasm-abi) — short, language-agnostic, three scalar types. For Rust, the bundled [`wasm-pdk`](https://github.com/dylan-sutton-chavez/edge-python/tree/main/wasm-pdk) crate is the reference author-side layer (`#[plugin_fn]`, typed `Handle` / `Value` / `Error`). For other languages, use a community PDK or write the boilerplate by hand — the minimal raw version is below.

`Cargo.toml`:

```toml
[package]
name = "my-edge-mod"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
lol_alloc = "0.4"

[profile.release]
opt-level = "z"
lto = true
panic = "abort"
strip = true
```

`src/lib.rs` — every export follows the wire ABI signature `(argv: *const u32, argc: u32, out: *mut u32) -> i32`, where `argv` carries host-owned handles and the result is written back as a handle:

```rust
#![no_std]
#![no_main]
extern crate alloc;
use alloc::{boxed::Box, vec};

#[global_allocator]
static A: lol_alloc::LeakingPageAllocator = lol_alloc::LeakingPageAllocator;
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn edge_encode(tag: u32, ptr: *const u8, len: u32) -> u32;
    fn edge_decode(h: u32, out_tag: *mut u32, dst: *mut u8, dst_max: u32) -> i32;
}

const TAG_INT: u32 = 2;

#[unsafe(no_mangle)]
pub extern "C" fn __edge_alloc(size: u32) -> *mut u8 {
    Box::into_raw(vec![0u8; size as usize].into_boxed_slice()) as *mut u8
}

#[unsafe(no_mangle)]
pub extern "C" fn __edge_abi_version() -> u32 { 1 }

#[unsafe(no_mangle)]
pub extern "C" fn add(argv: *const u32, argc: u32, out: *mut u32) -> i32 {
    if argc != 2 { return 1; }
    let mut tag: u32 = 0;
    let mut buf = [0u8; 8];
    let mut read_int = |h: u32| -> Option<i64> {
        let r = unsafe { edge_decode(h, &mut tag, buf.as_mut_ptr(), 8) };
        if r < 0 || tag != TAG_INT { return None; }
        Some(i64::from_le_bytes(buf))
    };
    let a = match read_int(unsafe { *argv }) { Some(v) => v, None => return 1 };
    let b = match read_int(unsafe { *argv.add(1) }) { Some(v) => v, None => return 1 };
    let sum = (a + b).to_le_bytes();
    let h = unsafe { edge_encode(TAG_INT, sum.as_ptr(), 8) };
    unsafe { *out = h; }
    0
}
```

For anything but trivial scalar examples, prefer the `wasm-pdk` crate (`#[plugin_fn]`) — see the [WASM module ABI](/reference/wasm-abi) worked example. Build and use:

```bash
cargo build --release --target wasm32-unknown-unknown
# -> target/wasm32-unknown-unknown/release/my_edge_mod.wasm
```

```python
from "./my_edge_mod.wasm" import add
print(add(2, 3))   # -> 5
```

Full encoding tables and language-specific snippets (C, Zig, AssemblyScript) live in [WASM module ABI](/reference/wasm-abi).

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

## Choosing between the three paths

| You want… | Use |
|---|---|
| Publish a module any Edge Python user can `from "<url>" import` without rebuilding | Path A (`.wasm` ABI) |
| Maximum speed and full type coverage (strings, lists, etc.) | Path B (in-process Rust) |
| Wrap a C/Zig/AS library | Path A (any wasm32-targeting language works) |
| Plug into a custom Rust app and expose its APIs | Path B |
| Expose host services (DOM, FS, native crypto) that Path A's sandboxed `.wasm` can't reach | Path C (host capability) |
| Both at once | Compose: your embedder's `Resolver` can return `Resolved::Native(...)` for in-process modules AND let `.wasm` URL imports flow through to the bridge |

## Pure vs impure

`pure: true` (in-process bindings only) lets the VM memoize the result — repeated calls with the same args skip execution. Mark functions pure when they depend only on their args. `.wasm`-loaded bindings default to `pure: false` since the host can't introspect their semantics.

## See also

- [WASM module ABI](/reference/wasm-abi) — the wire format spec for Path A.
- [Imports](/reference/imports) — script-side semantics, packages.json, integrity verification.
- [`compiler/src/modules/packages/mod.rs`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/compiler/src/modules/packages/mod.rs) — full `Resolver` trait + `NativeBinding` struct.
