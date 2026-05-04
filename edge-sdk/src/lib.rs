//! SDK for writing Edge Python native modules in Rust.
//!
//! Use `edge_export!` to expose a Rust function to EdgePython scripts. The
//! macro wraps your function in the C ABI Edge Python's WASM loader expects:
//! arguments come in as `u64`-encoded `Val`s, the result is returned the same
//! way. You write idiomatic Rust; the macro handles the bit-twiddling.
//!
//! ```ignore
//! use edge_sdk::edge_export;
//!
//! edge_export! {
//!     pub fn add(a: i64, b: i64) -> i64 {
//!         a + b
//!     }
//! }
//! ```
//!
//! Then compile to WebAssembly:
//!
//! ```text
//! cargo build --release --target wasm32-unknown-unknown
//! ```
//!
//! The resulting `.wasm` is loadable by any Edge Python host that implements
//! the WASM loader pattern (see the loader in `compiler/tests/common.rs` for
//! the reference implementation).
//!
//! # Supported types (v1)
//!
//! Arguments and return: `i64` only. The encoding mirrors Edge Python's
//! NaN-boxed `Val::int`. Floats, strings, and heap types are coming.

#![no_std]

/* ─── Marshalling primitives ──────────────────────────────────────────────── */

/* NaN-boxed Val tag bits — must stay in sync with `vm::types::TAG_INT`. */
const QNAN: u64 = 0x7FFC_0000_0000_0000;
const SIGN: u64 = 0x8000_0000_0000_0000;
const TAG_INT: u64 = QNAN | SIGN;

/// Decode a `u64`-wire `Val` back into the contained `i64`. Sign-extends from
/// the 48-bit payload so negative values round-trip correctly.
#[inline(always)]
pub fn unpack_int(v: u64) -> i64 {
    let raw = (v & 0x0000_FFFF_FFFF_FFFF) as i64;
    (raw << 16) >> 16
}

/// Encode an `i64` into the wire `u64` format Edge Python uses. The result
/// matches `Val::int(i)` byte-for-byte.
#[inline(always)]
pub fn pack_int(i: i64) -> u64 {
    TAG_INT | (i as u64 & 0x0000_FFFF_FFFF_FFFF)
}

/* ─── The macro ───────────────────────────────────────────────────────────── */

/// Wrap a Rust function so it's callable from Edge Python via the WASM loader.
///
/// The wrapped function appears in the WASM module's exports under its given
/// name, with a C ABI signature that takes/returns `u64` for each argument
/// (the wire format for `Val`). The macro inserts the marshalling so the
/// inner function can use plain Rust types.
///
/// Currently supports `i64` arguments and `i64` return. Float and heap-type
/// support is planned.
#[macro_export]
macro_rules! edge_export {
    ($vis:vis fn $name:ident($($arg:ident: i64),* $(,)?) -> i64 $body:block) => {
        #[unsafe(no_mangle)]
        $vis extern "C" fn $name($($arg: u64),*) -> u64 {
            $(let $arg: i64 = $crate::unpack_int($arg);)*
            let __edge_result: i64 = { $body };
            $crate::pack_int(__edge_result)
        }
    };
}
