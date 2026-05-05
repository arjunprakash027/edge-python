---
title: "Writing modules"
description: "How to write a native module for Edge Python in Rust today, plus the planned cross-language ABI."
---

Edge Python has no bundled stdlib. Modules are external artifacts — you write one, host it on a URL, and any script can `from "<url>" import <names>`. This page covers what's actually shipping today and the planned shape for other languages.

## What works today

| Module type | Status |
|---|---|
| `.py` source modules (multi-file projects) | ✅ implemented, works in CLI |
| Rust → `.wasm` modules via `edge-sdk` | ✅ implemented, works in CLI |
| `http(s)://` URL imports | ✅ implemented, works in CLI (no cache yet) |
| C / Zig / AS → `.wasm` modules | 🟡 same ABI works in theory; no SDK published yet, no test fixtures |
| Native dyn-libs (`.so`, `.dylib`, `.dll`) | ❌ ABI designed, loader not implemented |
| Integrity verification (`#sha256-...`) | ❌ planned |
| Multi-arch manifest selection | ❌ planned |

The rest of this page focuses on the Rust → WASM path, which is the only fully shipped non-`.py` flavor and the one the CLI loads natively.

## Quick start: Rust to WASM

### 1. Add the SDK

```toml
# Cargo.toml of your module crate
[package]
name = "my-edge-module"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
edge-sdk = { git = "https://github.com/dylan-sutton-chavez/edge-python", branch = "main" }

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

### 2. Write your module

```rust
#![no_std]
#![no_main]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

use edge_sdk::edge_export;

edge_export! {
    pub fn add(a: i64, b: i64) -> i64 {
        a + b
    }
}

edge_export! {
    pub fn square(x: i64) -> i64 {
        x * x
    }
}

edge_export! {
    pub fn area(r: f64) -> f64 {
        3.141592653589793 * r * r
    }
}

edge_export! {
    pub fn even(n: i64) -> bool {
        n % 2 == 0
    }
}
```

The `edge_export!` macro wraps your function in the C ABI Edge Python's loader expects: arguments come in as `u64`-encoded `Val`s, the result is returned the same way. You write idiomatic Rust; the macro handles the marshalling.

### 3. Build to WebAssembly

```bash
cargo build --release --target wasm32-unknown-unknown
# → target/wasm32-unknown-unknown/release/my_edge_module.wasm
```

### 4. Use it from a script

Drop the `.wasm` next to your script (or wherever, then point at it):

```text
my-app/
├── packages.json
├── main.py
└── vendor/
    └── my_edge_module.wasm
```

```json packages.json
{
  "imports": { "math": "./vendor/my_edge_module.wasm" }
}
```

```python main.py
from math import add, square

print(add(2, square(3)))   # 11
```

```bash
edge main.py
```

The CLI loads the `.wasm` via wasmtime, registers each exported function as an Edge Python native, and dispatches your script's calls through the `CallExtern` opcode straight into the WASM instance.

## Reference example

The canonical reference module lives at [`edge-sdk/examples/reference.rs`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/edge-sdk/examples/reference.rs). The integration test in [`compiler/tests/packages.rs`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/compiler/tests/packages.rs) builds that exact file to a real `.wasm`, loads it through the production loader, and runs an Edge Python script that imports from it. **If the reference breaks, CI fails** — so the documentation here can never drift from what actually compiles and runs.

## Reference loader

The reference WASM loader lives in [`compiler/src/modules/packages/wasm_loader.rs`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/compiler/src/modules/packages/wasm_loader.rs) under `load_wasm_bindings`. It uses [`wasmtime`](https://wasmtime.dev/) to instantiate the module and walks every i64-typed export, wrapping each in a `NativeBinding` closure that dispatches into the wasmtime instance.

The `edge` CLI ships with this loader, so end users don't write any Rust to use `.wasm` modules — they just point `packages.json` at the file. Embedders building on top of `compiler_lib` can either reuse `load_wasm_bindings` or substitute a different WebAssembly runtime ([`wasmer`](https://wasmer.io/), [`wasmi`](https://crates.io/crates/wasmi)) following the same `NativeBinding` shape.

The runtime isn't prescribed — what matters is that bindings get into the parser via the `Resolver` trait. See [Imports](/reference/imports) for how the Resolver is wired in.

## ABI details (v1)

What the SDK actually generates for you:

```
WASM module exports (per edge_export! invocation):
  $name: (i64, i64, ...) -> i64

Wire format:
  Each i64 argument is the bit-cast of an Edge Python Val::int.
    Val::int(i) = (TAG_INT | (i as u64 & 0x0000_FFFF_FFFF_FFFF))
  The return value is the same encoding.
```

The host's loader is responsible for the inverse marshalling: it reads `Val`s off the EdgePython stack, casts them to `u64`, calls into the WASM, casts the result back into a `Val`. The SDK and the loader agree on the encoding, so authors writing modules with the SDK and consumers loading those modules don't have to think about bit-twiddling.

### Supported types

| Rust type | EdgePython type | Encoding                           |
|-----------|-----------------|------------------------------------|
| `i64`     | `int`           | NaN-boxed sign-extended 47-bit     |
| `f64`     | `float`         | raw `f64::to_bits()`               |
| `bool`    | `bool`          | NaN-boxed True/False tag           |

The macro picks the right encode/decode per parameter and return via the `FromWire` / `IntoWire` traits — mix types freely. The wasm signature stays `(i64, ..., i64) -> i64` regardless of Rust types: the i64 always carries the NaN-boxed wire `Val`, and the macro decodes it back to the requested type at the call boundary.

Coming: strings (via shared linear memory), heap types (lists, dicts). For now, encode strings as host-side handles (an `i64` that the host reinterprets through a side channel) if you need them.

## What other languages need

The ABI is language-agnostic. To add support for a new source language, a contributor needs to:

1. Write the equivalent of `edge_export!` for that language (a macro, a code generator, or just a documented manual pattern).
2. Make sure the compiled output exports functions with the i64 wire format.
3. Compile to either `.wasm` (works with the existing loader) or to a dyn-lib (needs a new loader to be added).

There's no formal SDK for C, Zig, or AssemblyScript yet — but a `.wasm` produced by any of them with the right exports will load with the existing wasmtime-based loader. We accept SDK contributions for any language that targets `wasm32-unknown-unknown` cleanly.

## Hosting checklist

When you're ready to publish:

- HTTPS (HTTP triggers warnings in browser hosts)
- `Cache-Control: immutable` for hashed/versioned URLs
- CORS headers if the module will be loaded by browser scripts
- Source repo public (auditors will read your code before approving)
- Semver in the URL path (`@1.2.3`) for version pinning
- SHA256 hash on the import URL: `from "https://...wasm#sha256-..." import x`

## See also

- [Imports](/reference/imports) — script author's view of the import system.
- [Built-in functions](/reference/builtins) — what's in the language vs what's in modules.
- [Limits and errors](/reference/limits-and-errors) — sandbox semantics that apply to native code too.
