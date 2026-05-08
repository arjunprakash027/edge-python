---
title: "WASM module ABI"
description: "The wire format a `.wasm` module must follow to be importable by Edge Python."
---

> **Sealed contract — v1.** Every signature, op code, tag, and error kind on this page is part of the public contract. New capabilities arrive as new `Op` values consumed by `edge_op`, never as new imports. Bug fixes that align an implementation with this page are the only acceptable maintenance. A future wire-level break would ship under a different module name (`env_v2.*`) without removing v1.

A `.wasm` module that an Edge Python script imports via `from "<url>" import <names>` follows the contract below. The shape is a small **handle-based** API: the host owns all values, the guest sees only opaque `u32` handles, and one universal dispatch primitive (`edge_op`) covers every operation on those values. This means new types, methods, and language features added to Edge Python become available to existing modules with **no ABI change**.

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

Handles in `argv` are owned by the host and live for the duration of the call. Handles the guest creates via `edge_encode` or `edge_op` are owned by the guest until released — the guest must call `edge_release` on each before returning, **except** for the one written into `*out`.

## Required guest export

In addition to the user functions, every guest module MUST export:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn __edge_alloc(size: u32) -> *mut u8;
```

The host calls this to stage `argv` arrays in the guest's linear memory before invoking each export. The reference `edge-pdk` crate emits it automatically.

## Host imports (6 functions)

A guest module declares these from the `env` module:

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

Bootstrap encoder. Wraps a primitive in a fresh handle (rc=1; release when done). The `ptr`/`len` describe bytes in **guest** memory; the host copies as needed.

### `edge_decode`

Bootstrap decoder. Writes the value's tag at `*out_tag` and copies its bytes into the guest-provided buffer `dst[..dst_max]`. Returns the number of bytes copied (`>= 0`), or `-bytes_needed` (`< 0`) if the buffer was too small — re-allocate and retry. On invalid handle or non-primitive type, returns `0` with `*out_tag = 0xFFFFFFFF` (composites must be operated on via `edge_op`).

### `edge_release`

Decrement refcount on a handle. No-op for handle `0` or already-released handles.

### `edge_take_error`

Drain the most recent error stashed by a `1`-returning `edge_op`. Writes the kind at `*out_kind` and copies the UTF-8 message into `dst[..dst_max]`. Returns the byte count copied (`>= 0`), `-bytes_needed` if the buffer was too small (the error stays pending — retry with a bigger buffer), or `-1` if no error was pending.

### `edge_throw`

Stash an error so the host sees it after the guest returns `1` from its export. Used when the guest produces an error that did NOT originate from a `1`-returning `edge_op` (e.g., a typed `Result::Err` from user code). Calling this overwrites any pending error; the guest must immediately return `1`.

## Op codes

| Op | Value | Meaning |
|---|---|---|
| `Call` | 0 | `recv.<name>(args...)` → handle |
| `GetAttr` | 1 | `recv.<name>` → handle (planned) |
| `SetAttr` | 2 | `recv.<name> = args[0]` → handle (None) (planned) |
| `GetItem` | 3 | `recv[args[0]]` → handle (planned) |
| `SetItem` | 4 | `recv[args[0]] = args[1]` → handle (None) (planned) |
| `Len` | 5 | `len(recv)` → handle (Int) |
| `Iter` | 6 | `iter(recv)` → handle (iterator) (planned) |
| `IterNext` | 7 | `next(iter)` → handle, or `1`+`StopIteration` on end (planned) |

Values `8..u32::MAX` are reserved for future ops. Old hosts return `1` with `kind=Runtime` for unknown ops.

`v1` ships `Call` and `Len` fully. The rest of the table is reserved and will be enabled in subsequent point releases without ABI changes.

## Tags (for `edge_encode` / `edge_decode`)

| Tag | Value | Layout |
|---|---|---|
| None  | 0 | payload ignored |
| Bool  | 1 | 1 byte (0/1) |
| Int   | 2 | 8 bytes little-endian i64 |
| Float | 3 | 8 bytes IEEE 754 little-endian |
| Bytes | 4 | UTF-8 → `str`; non-UTF-8 → `bytes` |

Composite values (list, dict, set, instance, callable, iterator) are **not** encodable. Construct them through `edge_op(Call, type_handle, ...)` and operate via the indexing ops.

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

## Worked example — recommended Rust path with `edge-pdk`

The `edge-pdk` crate (in this repo at `edge-pdk/`) ships the
`#[plugin_fn]` proc macro that expands to wire-conformant exports.
Authors write normal Rust:

```rust
// examples/slugify-mod/src/lib.rs
#![no_std] #![no_main]
extern crate alloc;

use alloc::string::String;
use edge_pdk::*;

#[global_allocator]
static A: lol_alloc::LeakingPageAllocator = lol_alloc::LeakingPageAllocator;
#[panic_handler] fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

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
edge-pdk = { path = "../../edge-pdk" }   # or = "0.1" once published
lol_alloc = "0.4"
```

Build:

```bash
cargo build --release --target wasm32-unknown-unknown -p slugify-mod
# → target/wasm32-unknown-unknown/release/slugify_mod.wasm   (~74 KB stripped)
```

Use it from a script:

```python
from "./slugify_mod.wasm" import slugify, repeat_n, sum_ints

print(slugify("Hello World"))     # → hello-world
print(repeat_n("ha", 3))          # → hahaha
print(sum_ints([1, 2, 3, 4]))     # → 10

try:
    print(repeat_n("nope", -1))
except ValueError as e:
    print("caught:", e)           # → caught: repeat count must be non-negative
```

## Worked example — raw, no SDK

For Zig / C / hand-written Rust, the same module without the macro:

### The Rust source (`src/lib.rs`)

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
    if unsafe { edge_op(OP_CALL, input, b"lower".as_ptr(), 5,
                        core::ptr::null(), 0, &mut lower) } != 0 {
        return 1;
    }

    // 2) lower.replace(" ", "-")
    let space = unsafe { edge_encode(TAG_BYTES, b" ".as_ptr(), 1) };
    let dash  = unsafe { edge_encode(TAG_BYTES, b"-".as_ptr(), 1) };
    let argv2 = [space, dash];
    let r = unsafe { edge_op(OP_CALL, lower, b"replace".as_ptr(), 7,
                             argv2.as_ptr(), 2, out) };

    // 3) Cleanup intermediate handles. The result handle in *out
    //    transfers to the host.
    unsafe { edge_release(space); edge_release(dash); edge_release(lower); }
    r
}
```

### `Cargo.toml`

```toml
[package]
name = "slugify-mod"
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

### Compile

```bash
cargo build --release --target wasm32-unknown-unknown
# → target/wasm32-unknown-unknown/release/slugify_mod.wasm   (~1-2 KB)
```

### Use it from an Edge Python script

```python
from "https://example.com/slugify.wasm" import slugify

print(slugify("Hello World"))    # → hello-world
print(slugify("ABC 123"))        # → abc-123
```

Or via `packages.json`:

```json
{
  "imports": {
    "slug": "https://example.com/slugify.wasm"
  }
}
```

```python
from slug import slugify
print(slugify("Hello World"))
```

## How the host loads it

When the host (browser shim, WASI runtime, Rust embedder) sees `from "<url>" import <names>` and the URL ends in `.wasm`:

1. **Fetch** the bytes (browser: `fetch()`; CLI: filesystem or HTTP).
2. **Verify integrity** if a `#sha256-...` fragment is present (parser-side, before any code runs).
3. **Instantiate** the module with the 5 host imports wired to compiler.wasm exports (`host_edge_op`, `host_edge_encode`, `host_edge_decode`, `host_edge_release`, `host_edge_take_error`).
4. **Walk the export table** and register every callable function under its name.
5. **At each script call site**, the host registers the args as handles, invokes the matching guest export, reads the result handle, and propagates the `Val`. Errors stashed via `edge_take_error` raise the corresponding Python exception.

The reference browser shim is `demo/worker.js`. WASI hosts and Rust embedders mirror the same shape against their own runtime.

## Constraints and gotchas

- **Refcounted handles.** The guest must release every handle it creates via `edge_encode` or `edge_op` except the one it returns through `*out`. Argv handles are released by the host.
- **`edge_decode` only handles primitives.** For `list`, `dict`, `set`, instances, etc., use `edge_op` (e.g. `Call recv "items"`, `GetItem recv idx`).
- **Reentrance is supported.** A guest's `edge_op` runs while the Edge Python VM is paused on the script's `CallExtern`. Method dispatch routes through the same `methods.rs` table the language uses internally — adding a method there makes it visible to existing modules without recompiling them.
- **Error-as-status, not panic.** Returning `1` from a guest function does NOT abort the host. The host pulls the error and raises it as a typed Python exception in the script.
- **Memory ownership.** The host doesn't read the guest's linear memory except to copy in/out at well-defined points. Anything the guest allocates internally (its own pools, caches, embedded blobs) is private; the host never touches it.

## Author conveniences (community-maintained)

The Edge Python project ships only this spec. The reference Rust author layer is the **`edge-pdk`** crate (Plugin Development Kit), bundled in this repo at `edge-pdk/` and intended to be published independently. It provides:

- `#[plugin_fn]` proc macro that turns a typed Rust function into a wire-conformant export.
- `FromValue` / `IntoValue` traits with primitive impls (`i64`, `f64`, `bool`, `String`, `&str`, `Option<T>`, `Handle`).
- `Handle` / `Value` / `Error` types wrapping handles with `Drop`-driven release.
- The required `__edge_alloc` export emitted automatically.

A typical author-side function with the macro:

```rust
use edge_pdk::*;

#[plugin_fn]
fn slugify(s: String) -> String {
    s.to_lowercase().replace(' ', "-")
}
```

The macro emits the boilerplate seen in the worked example above. Authors who don't want a toolchain hand-roll the boilerplate (~25 lines for the first function, ~5 lines per additional).

Per the project's policy, similar community PDKs exist for Zig (`edge-pdk-zig`), AssemblyScript (`edge-pdk-as`), and C (`edge-pdk.h`) without coordinated releases — each tracks the sealed wire spec on its own cadence.

## See also

- [Imports](/reference/imports) — how `from "..." import` resolves on the script side, including walk-up packages.json and integrity verification.
- [Writing modules](/reference/writing-modules) — the in-process Rust embedder path (full type coverage, no wire format).
