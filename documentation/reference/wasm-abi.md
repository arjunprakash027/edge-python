---
title: "WASM module ABI"
description: "The wire format a `.wasm` module must follow to be importable by Edge Python."
---

> **Sealed contract — plugin ABI v1.** Every signature, op code, tag, and error kind here is the public contract for CDN-distributed `.wasm` plugin modules (Path A). New host packages arrive as new `Op` values, never new imports; a future wire-level break would ship as `env_v2.*` without removing v1. Distinct from the compiler↔host interface embedders declare (see [host packages](/reference/writing-modules#path-c-host-capability)) — embedders aren't bound by the 6-import limit here.

A `.wasm` module imported via `from "<url>" import <names>` follows the contract below. Handle-based API: the host owns all values, the guest sees only opaque `u32` handles, and one universal dispatch primitive (`edge_op`) covers every operation. New types, methods, and language features become available to existing modules with no ABI change.

## Guest export shape

Every function the script can call is exposed as:

```rust
extern "C" fn <name>(argv: *const u32, argc: u32, out: *mut u32) -> i32;
```

| Field | Meaning |
|---|---|
| `argv` | Pointer (in **guest** linear memory) to an array of `argc` host-managed handles, one per positional argument. |
| `argc` | Positional argument count. |
| `out`  | Pointer (in **guest** linear memory) where the guest writes ONE handle for the return value. |
| return | `0` = success, `1` = error (host pulls the error via `edge_take_error` immediately). |

`argv` handles are host-owned and live for the call. Handles the guest creates via `edge_encode` or `edge_op` are guest-owned until released — the guest must `edge_release` each before returning, except the one written into `*out`.

## Required guest exports

Every guest module MUST export, in addition to its user functions:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn __edge_alloc(size: u32) -> *mut u8;

#[unsafe(no_mangle)]
pub extern "C" fn __edge_abi_version() -> u32;
```

`__edge_alloc` lets the host stage `argv` arrays in guest linear memory before invoking each export.

`__edge_abi_version` returns the wire-format version (currently `1`). The host MUST read this once at instantiation and refuse unknown versions — otherwise a v2 host would silently decode garbage from a v1 module. (At v1 every loader targets 1 so the bundled `compiler.wasm` shim does not yet read the symbol; the check becomes load-bearing when v2 ships.)

The reference `wasm-pdk` crate emits both symbols automatically. `EDGE_ABI_VERSION` lives in the shared `wasm-abi` crate (no_std, zero deps) so host and every PDK read the same value.

## Host imports (6 functions)

Guest declares from `env`:

```rust
fn edge_op(
    op: u32,
    recv: u32,
    name_ptr: *const u8, name_len: u32,
    argv_ptr: *const u32, argc: u32,
    out: *mut u32,
) -> i32;

fn edge_encode(tag: u32, ptr: *const u8, len: u32) -> u32;

fn edge_decode(
    h: u32,
    out_tag: *mut u32,
    dst: *mut u8, dst_max: u32,
) -> i32;

fn edge_release(h: u32);

fn edge_take_error(
    out_kind: *mut u32,
    dst: *mut u8, dst_max: u32,
) -> i32;

fn edge_throw(kind: u32, msg_ptr: *const u8, msg_len: u32);
```

### `edge_op`

Universal dispatch. Returns `0` with a fresh handle in `*out` on success, `1` on error.

### `edge_encode`

Wraps a primitive in a fresh handle (rc=1; release when done). `ptr`/`len` describe bytes in guest memory; the host copies.

### `edge_decode`

Writes the value's tag at `*out_tag` and copies bytes into `dst[..dst_max]`. Returns bytes copied (`>= 0`), or `-bytes_needed` if the buffer was too small (re-allocate and retry). On invalid handle or non-primitive, returns `0` with `*out_tag = 0xFFFFFFFF` (composites go through `edge_op`).

### `edge_release`

Decrement refcount. No-op for handle `0` or already-released.

### `edge_take_error`

Drain the most recent error from a `1`-returning `edge_op`. Writes kind at `*out_kind`, copies the UTF-8 message into `dst[..dst_max]`. Returns bytes copied (`>= 0`), `-bytes_needed` if the buffer was too small (error stays pending), or `-1` if no error pending.

### `edge_throw`

Stash an error visible after the guest returns `1` — used when an error did not originate from a `1`-returning `edge_op` (e.g., a typed `Result::Err` from user code). Overwrites any pending error; the guest must immediately return `1`.

## Op codes

| Op | Value | Meaning |
|---|---|---|
| `Call` | 0 | `recv.<name>(args...)` -> handle |
| `GetAttr` | 1 | `recv.<name>` -> handle |
| `SetAttr` | 2 | `recv.<name> = args[0]` -> handle (None) |
| `GetItem` | 3 | `recv[args[0]]` -> handle |
| `SetItem` | 4 | `recv[args[0]] = args[1]` -> handle (None) |
| `Len` | 5 | `len(recv)` -> handle (Int) |
| `Iter` | 6 | `iter(recv)` -> handle (iterator List) |
| `IterNext` | 7 | `next(iter)` -> handle, or `1`+`StopIteration` on end |

All eight ops wired in v1. `Op::Iter` materialises the receiver into a List handle (set sorted via `vm.sort_set_items`; dict yields keys; str splits to single-char strings); `Op::IterNext` advances it. Values `8..u32::MAX` reserved — old hosts return `1` with `kind=Runtime`.

## Tags (for `edge_encode` / `edge_decode`)

| Tag | Value | Layout |
|---|---|---|
| None  | 0 | payload ignored |
| Bool  | 1 | 1 byte (0/1) |
| Int   | 2 | 8 bytes little-endian i64 |
| Float | 3 | 8 bytes IEEE 754 little-endian |
| Bytes | 4 | UTF-8 -> `str`; non-UTF-8 -> `bytes` |

Composites (list, dict, set, instance, callable, iterator) are not encodable. Construct via `edge_op(Call, type_handle, ...)` and operate via indexing ops.

## Error kinds (for `edge_take_error`)

| Kind | Value | Maps to |
|---|---|---|
| Type      | 0 | `TypeError` |
| Value     | 1 | `ValueError` |
| Runtime   | 2 | `RuntimeError` |
| Attribute | 3 | `AttributeError` |
| Index     | 4 | `IndexError` |
| Key       | 5 | `KeyError` |
| Custom    | 6 | the message carries the user-defined kind name |

## Worked example — recommended Rust path with `wasm-pdk`

The `wasm-pdk` crate provides the `#[plugin_fn]` proc macro that expands to wire-conformant exports. Authors write normal Rust:

```rust
// starter-module/src/lib.rs
#![no_std] #![no_main]
extern crate alloc;

use alloc::string::String;
use wasm_pdk::*;

wasm_pdk::module!();   // expands to #[global_allocator] + #[panic_handler]

#[plugin_fn]
fn slugify(s: String) -> String {
    s.to_lowercase().replace(' ', "-")
}

#[plugin_fn]
fn repeat_n(s: String, n: i64) -> Result<String> {
    if n < 0 { return Err(Error::Value("repeat count must be non-negative".into())); }
    Ok(s.repeat(n as usize))
}

#[plugin_fn]
fn sum_ints(items: Handle) -> Result<i64> {
    let n = items.len()?;
    let mut total: i64 = 0;
    for i in 0..n as u32 {
        total += i64::from_handle(items.get_item(i)?.raw())?;
    }
    Ok(total)
}
```

`Cargo.toml`:

```toml
[package]
name = "slugify-mod"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
wasm-pdk = { path = "../../wasm-pdk" }   # in-repo example; external authors use the git+tag form below
```

Build:

```bash
cargo build --release --target wasm32-unknown-unknown -p slugify-mod # -> target/wasm32-unknown-unknown/release/slugify_mod.wasm   (around 74 KB stripped)
```

### Consuming `wasm-pdk` from your own crate

Not on crates.io — depend from GitHub, pinned to a release tag:

```toml
[dependencies]
wasm-pdk = { git = "https://github.com/dylan-sutton-chavez/edge-python", tag = "v0.1.0" }
```

Cargo resolves `wasm-abi` and `wasm-pdk-macros` transitively. Pinning to a tag (vs `branch = "main"`) gives reproducible builds and a known wire-ABI version — your module compiled against `wasm-pdk vX.Y.Z` is binary-compatible with the `compiler_lib.wasm` of the same release. Bump `tag` + `cargo update -p wasm-pdk` to upgrade. Use `branch = "main"` only for unreleased iteration.

Use it from a script:

```python
from "./slugify_mod.wasm" import slugify, repeat_n, sum_ints

print(slugify("Hello World")) # -> hello-world
print(repeat_n("ha", 3)) # -> hahaha
print(sum_ints([1, 2, 3, 4])) # -> 10

try:
    print(repeat_n("nope", -1))
except ValueError as e:
    print("caught:", e) # -> caught: repeat count must be non-negative
```

## Worked example — raw, no SDK

Same module without the macro (for Zig / C / hand-written Rust):

### Rust source (`src/lib.rs`)

```rust
#![no_std] #![no_main]
extern crate alloc;
use alloc::{boxed::Box, vec};

#[global_allocator]
static A: lol_alloc::LeakingPageAllocator = lol_alloc::LeakingPageAllocator;
#[panic_handler] fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn edge_op(
        op: u32, recv: u32,
        name_ptr: *const u8, name_len: u32,
        argv_ptr: *const u32, argc: u32,
        out: *mut u32,
    ) -> i32;
    fn edge_encode(tag: u32, ptr: *const u8, len: u32) -> u32;
    fn edge_release(h: u32);
}

const OP_CALL: u32 = 0;
const TAG_BYTES: u32 = 4;

/// Required by the host shim for staging argv arrays.
#[unsafe(no_mangle)]
pub extern "C" fn __edge_alloc(size: u32) -> *mut u8 {
    Box::into_raw(vec![0u8; size as usize].into_boxed_slice()) as *mut u8
}

#[unsafe(no_mangle)]
pub extern "C" fn slugify(argv: *const u32, argc: u32, out: *mut u32) -> i32 {
    if argc != 1 { return 1; }
    let input = unsafe { *argv };

    // 1) input.lower()
    let mut lower: u32 = 0;
    if unsafe { edge_op(OP_CALL, input, b"lower".as_ptr(), 5, core::ptr::null(), 0, &mut lower) } != 0 {
        return 1;
    }

    // 2) lower.replace(" ", "-")
    let space = unsafe { edge_encode(TAG_BYTES, b" ".as_ptr(), 1) };
    let dash  = unsafe { edge_encode(TAG_BYTES, b"-".as_ptr(), 1) };
    let argv2 = [space, dash];
    let r = unsafe { edge_op(OP_CALL, lower, b"replace".as_ptr(), 7, argv2.as_ptr(), 2, out) };

    // 3) Cleanup intermediate handles. The result handle in *out
    //    transfers to the host.
    unsafe { edge_release(space); edge_release(dash); edge_release(lower); }
    r
}
```

Same `Cargo.toml` as the `wasm-pdk` example (drop `wasm-pdk`, add `lol_alloc = "0.4"`); imported from scripts the same way.

## How the host loads it

For `from "<url>" import <names>` with a `.wasm` URL: the host fetches bytes (verifying any `#sha256-...` fragment), instantiates with the 6 host imports, walks the export table, marshals args as handles, propagates results. Reference browser shim: [`runtime/worker/worker.js`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/runtime/worker/worker.js); WASI hosts and Rust embedders mirror the shape.

## Constraints and caveats

* **Refcounted handles.** Guest releases every handle it creates via `edge_encode` / `edge_op` except the one returned through `*out`. Host releases argv.
* **`edge_decode` is primitives-only.** For `list`, `dict`, `set`, instances, use `edge_op` (e.g. `Call recv "items"`, `GetItem recv idx`).
* **Reentrance supported.** A guest's `edge_op` runs while the VM is paused on the script's `CallExtern`. Method dispatch routes through the same `vm/handlers/builtin_methods/` descriptor table the language uses internally — adding a method there makes it visible to existing modules with no recompile.
* **Error-as-status, not panic.** Returning `1` does NOT abort the host — the host pulls the error and raises it as a typed Python exception.
* **Memory ownership.** Host only reads guest linear memory at well-defined copy points. Guest-internal allocations stay private.

## Author conveniences

The `wasm-pdk` crate (Plugin Development Kit) — bundled in this repo, publishable independently of `compiler.wasm` — provides:

* `#[plugin_fn]` — typed Rust function → wire-conformant export.
* `module!()` — expands to `#[global_allocator]` + `#[panic_handler]`.
* `FromValue` / `IntoValue` with primitive impls (`i64`, `f64`, `bool`, `String`, `&str`, `Option<T>`, `Handle`).
* `Handle` / `Value` / `Error` with `Drop`-driven release.
* `__edge_alloc` + `__edge_abi_version` emitted automatically.

The macro emits the worked-example boilerplate; manual is ~25 lines for the first function, ~5 per additional.

Community PDKs (uncoordinated releases, each tracking the sealed wire spec): Zig (`wasm-pdk-zig`), AssemblyScript (`wasm-pdk-as`), C (`wasm-pdk.h`).

## See also

- [Imports](/reference/imports) — how `from "..." import` resolves on the script side, including walk-up packages.json and integrity verification.
- [Writing modules](/reference/writing-modules) — the in-process Rust embedder path (full type coverage, no wire format).
