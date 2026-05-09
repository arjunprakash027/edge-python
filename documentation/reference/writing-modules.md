---
title: "Writing modules"
description: "Two paths to extend Edge Python: a `.wasm` module loaded by URL, or in-process Rust closures via the Resolver trait."
---

Edge Python has no bundled stdlib. There are **two ways** to add native functionality. Pick the one that fits your distribution model.

| Path | Distribution | Type coverage | Maintenance |
|---|---|---|---|
| **`.wasm` module via URL** ([WASM ABI](/reference/wasm-abi)) | Publish a `.wasm` to a CDN; any host loads it dynamically | Primitives only (None, bool, i64 truncated to 47-bit, f64, bytes/str) | You own the wire-format boilerplate, or use a community SDK |
| **In-process Rust binding** | Publish a Rust crate; embedders link it as an rlib | Full — any `HeapObj` (str, list, dict, set, tuple, instance, …) | You own a Rust crate; cargo handles distribution |

The `.wasm` path matches the marketplace pattern (`from "https://x.wasm" import f` works in any host). The in-process path matches the embedder pattern (compile your modules into your own `compiler.wasm`). Both are first-class.

## Path A: `.wasm` module by URL

The contract is the [WASM module ABI](/reference/wasm-abi) — short, language-agnostic, three scalar types. The Edge Python project does **not** ship an SDK crate; you write the boilerplate or use a community-maintained wrapper.

`Cargo.toml`:

```toml
[package]
name = "my-edge-mod"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[profile.release]
opt-level = "z"
lto = true
panic = "abort"
strip = true
```

`src/lib.rs`:

```rust
#![no_std]
#![no_main]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

const QNAN:    u64 = 0x7FFC_0000_0000_0000;
const TAG_INT: u64 = QNAN | 0x8000_0000_0000_0000;

#[inline] fn unpack_int(v: u64) -> i64 {
    let raw = (v & 0x0000_FFFF_FFFF_FFFF) as i64;
    (raw << 16) >> 16
}
#[inline] fn pack_int(i: i64) -> u64 {
    TAG_INT | (i as u64 & 0x0000_FFFF_FFFF_FFFF)
}

#[unsafe(no_mangle)]
pub extern "C" fn add(a: u64, b: u64) -> u64 {
    pack_int(unpack_int(a) + unpack_int(b))
}
```

Build and use:

```bash
cargo build --release --target wasm32-unknown-unknown
# → target/wasm32-unknown-unknown/release/my_edge_mod.wasm
```

```python
from "./my_edge_mod.wasm" import add
print(add(2, 3))   # → 5
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
use std::sync::Arc;
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
    vec![NativeBinding { name: "upper".into(), func: Arc::new(upper), pure: true }]
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
text         = { path = "../text" }
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

Plug `AppResolver` into your parser entry point (replicate `compiler_lib`'s `main.rs` pattern, or call `Parser::with_resolver` directly for native binaries).

Build and use:

```bash
cargo build --release --target wasm32-unknown-unknown -p embedder
# Serve embedder.wasm in place of compiler_lib.wasm.
```

```python
from text import upper
print(upper("hello"))   # → HELLO
```

### Type cookbook (in-process only)

The closure signature is `Fn(&mut HeapPool, &[Val]) -> Result<Val, VmErr>`. Inside it:

```rust
// Reading args
let n: i64    = if args[0].is_int()   { args[0].as_int()   } else { return Err(VmErr::Type("expected int")); };
let f: f64    = if args[0].is_float() { args[0].as_float() } else { return Err(VmErr::Type("expected float")); };
let b: bool   = if args[0].is_bool()  { args[0].as_bool()  } else { return Err(VmErr::Type("expected bool")); };
let s: String = match heap.get(args[0]) {
    HeapObj::Str(s) => s.clone(),
    _ => return Err(VmErr::Type("expected str")),
};
let items: Vec<Val> = match heap.get(args[0]) {
    HeapObj::List(rc) => rc.borrow().clone(),
    _ => return Err(VmErr::Type("expected list")),
};

// Returning values
Ok(Val::int(42))                                    // scalar — no allocation
Ok(Val::bool(true))
Ok(Val::none())
heap.alloc(HeapObj::Str("hi".into()))               // heap-allocated str
heap.alloc(HeapObj::List(std::rc::Rc::new(std::cell::RefCell::new(vec![Val::int(1), Val::int(2)]))))
heap.alloc(HeapObj::Tuple(vec![Val::int(1), Val::bool(true)]))

// Errors → surface in scripts as the corresponding Python exception class
Err(VmErr::Type("expected str"))                    // → TypeError
Err(VmErr::Value("empty separator"))                // → ValueError
Err(VmErr::Runtime("network unavailable"))          // → RuntimeError
Err(VmErr::TypeMsg(format!("got {:?}", v)))         // dynamically formatted
```

## Choosing between the two paths

| You want… | Use |
|---|---|
| Publish a module any Edge Python user can `from "<url>" import` without rebuilding | Path A (`.wasm` ABI) |
| Maximum speed and full type coverage (strings, lists, etc.) | Path B (in-process Rust) |
| Wrap a C/Zig/AS library | Path A (any wasm32-targeting language works) |
| Plug into a custom Rust app and expose its APIs | Path B |
| Both at once | Compose: your embedder's `Resolver` can return `Resolved::Native(...)` for in-process modules AND let `.wasm` URL imports flow through to the bridge |

## Pure vs impure

`pure: true` (in-process bindings only) lets the VM memoize the result — repeated calls with the same args skip execution. Mark functions pure when they depend only on their args. `.wasm`-loaded bindings default to `pure: false` since the host can't introspect their semantics.

## See also

- [WASM module ABI](/reference/wasm-abi) — the wire format spec for Path A.
- [Imports](/reference/imports) — script-side semantics, packages.json, integrity verification.
- [`compiler/src/modules/packages/mod.rs`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/compiler/src/modules/packages/mod.rs) — full `Resolver` trait + `NativeBinding` struct.
