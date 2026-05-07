---
title: "Writing modules"
description: "How to write a native module for Edge Python in Rust."
---

Edge Python has no bundled stdlib. Modules are external artifacts — you write one in Rust, ship the `.wasm`, and any script can `from "<url>" import <names>`.

## What's in scope

Edge Python ships as a WebAssembly module. Imports come in two flavors and both load through the host's `Resolver`:

- **`.py` source modules** — multi-file projects with internal `import` chains. Source is parsed and the module's top level is spliced inline at compile time; the VM never sees a module concept.
- **Rust → `.wasm` modules via `edge-sdk`** — write a function in Rust, compile to `wasm32-unknown-unknown`, import from any script. The macro emits the C ABI Edge Python's loader expects.

Two transport features apply to both:

- **`http(s)://` URL imports** — the host fetches bytes at compile time. The reference browser shim (`demo/edge.js`) holds fetched bytes in an in-memory map for the duration of one `run()`, so the same URL referenced twice in a script fetches once. There is no persistent cache across runs in the reference shim — embedders that want one (IndexedDB, service worker, on-disk mirror) layer it on top of `fetch()`.
- **Integrity verification (`#sha256-<hex>`)** — append a digest to any URL spec; the compiler hashes the host's bytes and refuses to compile on mismatch. The check lives in the compiler, not the host, so the guarantee is identical across browser / WASI / embedder. See [Imports — Integrity verification](/reference/imports#integrity-verification).

There is no native dyn-lib path (`.so` / `.dylib` / `.dll`), by design — that would defeat the WASM sandbox's structural guarantees.

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

The host runtime loads the `.wasm` (browser shim via `WebAssembly.instantiateStreaming`, WASI host via its runtime API), registers each exported function as an Edge Python native, and dispatches your script's calls through the `CallExtern` opcode straight into the WASM instance.

## Reference example

The canonical reference module lives at [`edge-sdk/examples/reference.rs`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/edge-sdk/examples/reference.rs). The integration test in [`compiler/tests/packages.rs`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/compiler/tests/packages.rs) builds that exact file to a real `.wasm`, loads it through the test loader, and runs an Edge Python script that imports from it. **If the reference breaks, CI fails** — so the documentation here can never drift from what actually compiles and runs.

## Reference loader

A reference WASM loader lives in [`compiler/tests/loaders.rs`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/compiler/tests/loaders.rs) under `load_wasm_bindings`. It uses [`wasmtime`](https://wasmtime.dev/) to instantiate the module and walks every i64-typed export, wrapping each in a `NativeBinding` closure that dispatches into the wasmtime instance. `wasmtime` is a `[dev-dependencies]` entry — the production `compiler.wasm` doesn't bundle a WASM engine; loading is the host runtime's responsibility.

Production hosts implement the same shape against their own runtime: the [browser shim](https://github.com/dylan-sutton-chavez/edge-python/blob/main/demo/edge.js) does it via `WebAssembly.instantiateStreaming`; WASI embedders use their runtime's import API ([`wasmer`](https://wasmer.io/), [`wasmi`](https://crates.io/crates/wasmi), Cloudflare Workers, etc.).

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
