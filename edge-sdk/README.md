# edge-sdk

SDK for writing Edge Python native modules in Rust. Provides the `edge_export!` macro that wraps a plain Rust function in the C ABI Edge Python's WASM loader expects — no manual marshalling, no FFI boilerplate.

## Quick start

`Cargo.toml`:

```toml
[package]
name = "my-edge-mod"
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

`src/lib.rs`:

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
```

Build:

```bash
cargo build --release --target wasm32-unknown-unknown
# → target/wasm32-unknown-unknown/release/my_edge_mod.wasm
```

Use from any Edge Python host:

```python
from "./my_edge_mod.wasm" import add
print(add(2, 3))   # 5
```

## What the macro does

`edge_export!` takes a function with `i64` parameters and an `i64` return type and generates the equivalent `extern "C"` wrapper that takes/returns `u64` (the wire format for `Val::int`). The wrapper unpacks each argument, calls the inner function, and packs the result.

Without the macro:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn add(a: u64, b: u64) -> u64 {
    let a = (a & 0x0000_FFFF_FFFF_FFFF) as i64;
    let a = (a << 16) >> 16;   // sign-extend from 48 bits
    let b = (b & 0x0000_FFFF_FFFF_FFFF) as i64;
    let b = (b << 16) >> 16;
    let r = a + b;
    0x7FFC_0000_8000_0000 | (r as u64 & 0x0000_FFFF_FFFF_FFFF)
}
```

With the macro:

```rust
edge_export! {
    pub fn add(a: i64, b: i64) -> i64 {
        a + b
    }
}
```

## Supported types (v1)

Arguments and return: `i64` only. The encoding mirrors Edge Python's NaN-boxed `Val::int`. Floats, strings, and heap types are coming.

For values outside `i64`, drop down to manual marshalling using `pack_int` and `unpack_int` from this crate, plus your own bit work for other Val variants.

## Reference module

[`examples/reference.rs`](examples/reference.rs) is the canonical module the project uses to verify the SDK end-to-end. The compiler's test suite builds it to wasm32 and loads it through the production loader on every test run — if the SDK changes break the reference, CI fails.

Build the reference:

```bash
cargo build --release --target wasm32-unknown-unknown --example reference
# → target/wasm32-unknown-unknown/release/examples/reference.wasm
```

## ABI contract

The compiled `.wasm` module exports each `edge_export!`-decorated function with a `(i64, i64, ...) -> i64` signature. Edge Python's loader walks every i64-typed export and registers it as a native binding callable from scripts.

For full details on the loader side, see [`compiler/src/modules/packages/wasm_loader.rs`](../compiler/src/modules/packages/wasm_loader.rs).

## License

MIT OR Apache-2.0
