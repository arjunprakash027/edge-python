---
title: "WASM module ABI"
description: "The wire format a `.wasm` module must follow to be importable by Edge Python."
---

A `.wasm` module that an Edge Python script imports via `from "<url>" import <names>` must export functions following the wire format below. The contract is small (3 scalar types, one calling convention) and language-agnostic — Rust, C, Zig, AssemblyScript, anything that targets `wasm32` and exports C ABI functions can produce a compatible module.

The Edge Python project does not ship an SDK crate. This page is the spec; you write the boilerplate yourself or use any community-maintained wrapper.

## Wire format

Every exported function visible to Edge Python has signature:

```
(u64, u64, ..., u64) -> u64
```

Each `u64` carries one **NaN-boxed `Val`**, the same 64-bit tagged value Edge Python's VM uses internally. The host marshals scripts' arguments into u64s, calls your function, and decodes the returned u64.

### Tag bits

| Type | u64 encoding |
|---|---|
| `int` (i48 inline) | `0xFFFC_0000_0000_0000 \| (i & 0xFFFF_FFFF_FFFF)` — 48 signed bits, sign-extended on decode |
| `float` (f64) | `f.to_bits()` directly. Any NaN whose top bits collide with the `0x7FFC_*` tag pattern must be canonicalized to `0x7FF8_0000_0000_0000` to avoid ambiguity with tagged values |
| `bool` (`True`) | `0x7FFC_0000_0000_0002` |
| `bool` (`False`) | `0x7FFC_0000_0000_0003` |
| `None` | `0x7FFC_0000_0000_0001` |

Heap types (str, list, dict, etc.) are **not** representable in this wire format. A binding receives them as opaque heap-index Vals it can't dereference. If you need to pass strings or collections, encode them as ints (handles) and have the host interpret via a side channel — or use the in-process Rust embedder API (see [Writing modules](/reference/writing-modules)).

### Pack / unpack reference

Hand-rolled in any language. The Rust version:

```rust
const QNAN:    u64 = 0x7FFC_0000_0000_0000;
const TAG_INT: u64 = QNAN | 0x8000_0000_0000_0000;

#[inline] fn unpack_int(v: u64) -> i64 {
    let raw = (v & 0x0000_FFFF_FFFF_FFFF) as i64;
    (raw << 16) >> 16          // sign-extend from 48 bits
}
#[inline] fn pack_int(i: i64) -> u64 {
    debug_assert!(i >= -(1 << 47) && i < (1 << 47),
        "int outside inline 48-bit range");
    TAG_INT | (i as u64 & 0x0000_FFFF_FFFF_FFFF)
}

#[inline] fn unpack_float(v: u64) -> f64 { f64::from_bits(v) }
#[inline] fn pack_float(f: f64) -> u64 {
    let bits = f.to_bits();
    if (bits & QNAN) == QNAN { 0x7FF8_0000_0000_0000 } else { bits }
}

#[inline] fn unpack_bool(v: u64) -> bool { v == 0x7FFC_0000_0000_0002 }
#[inline] fn pack_bool(b: bool) -> u64 {
    if b { 0x7FFC_0000_0000_0002 } else { 0x7FFC_0000_0000_0003 }
}
```

C, Zig, and AssemblyScript ports are mechanical — same masks, same shifts.

## Minimal Rust example

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

Build:

```bash
cargo build --release --target wasm32-unknown-unknown
# → target/wasm32-unknown-unknown/release/my_edge_mod.wasm
```

Use from any Edge Python host:

```python
from "./my_edge_mod.wasm" import add
print(add(2, 3))   # → 5
```

## How the host loads it

When the host (browser shim, WASI runtime, Rust embedder) sees `from "<url>" import <names>` and the URL ends in `.wasm`, it:

1. Fetches the bytes.
2. Instantiates the module (`WebAssembly.instantiate` in the browser, `wasmtime::Module` server-side).
3. Walks the module's exports table and registers every function under the same name.
4. When a script invokes a binding, the host packs the args into u64s, calls the export, and unpacks the return.

The reference browser implementation is in [`demo/edge.js`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/demo/edge.js) (`_registerNativeModule` + `_handleNativeCall`). Other hosts mirror the same shape against their own runtime.

## Constraints and gotchas

- **Integers are i48, not i64.** Values outside ±2⁴⁷ silently truncate on the wire. If you need full i64 or BigInt, that's not representable here — work around it (split into limbs, etc.) or use the Rust embedder path.
- **NaN payloads can collide with tagged values.** `pack_float` must canonicalize any NaN whose top mantissa bits look like the QNAN tag pattern. Most NaNs from arithmetic don't collide; this only affects user-constructed NaN payloads.
- **No string / list / dict marshalling.** A `Val::Str` arrives as an opaque heap-index u64 your function can't read. Either encode handles you exchange via host channels, or use the in-process Rust API.
- **No exceptions across the boundary.** A binding returns one u64. To signal an error from a `.wasm` module, reserve a sentinel return value and document it. (The Rust embedder API does have proper `VmErr`; the WASM wire doesn't.)
- **Memory ownership.** The host doesn't read your module's linear memory; it just calls exports and reads/writes u64s. If your module allocates internally, those allocations are its private concern.

## Author conveniences (community-maintained)

The Edge Python project ships only this spec. Authors who want sugar (a `#[edge_export]` macro, `FromWire/IntoWire` traits, etc.) have two options:

- **Use a community SDK crate.** If anyone publishes one to crates.io with the `edge-python` keyword, you can depend on it.
- **Hand-roll the boilerplate.** It's ~25 lines per module crate (the example above is the entire boilerplate; per-function it's ~5 lines).

If you publish a SDK crate, ship it as a separate package — Edge Python's policy is to maintain only the wire spec, not author tooling.

## See also

- [Imports](/reference/imports) — how `from "..." import` resolves on the script side.
- [Writing modules](/reference/writing-modules) — the in-process Rust embedder path (full type coverage, no wire format).
