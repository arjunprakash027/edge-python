//! SDK for writing Edge Python native modules in Rust.
//!
//! Use [`edge_export!`] to expose a Rust function to EdgePython scripts. The
//! macro wraps your function in the C ABI Edge Python's WASM loader expects:
//! arguments come in as `u64`-encoded `Val`s, the result is returned the same
//! way. You write idiomatic Rust; the macro handles the bit-twiddling.
//!
//! ```ignore
//! use edge_sdk::edge_export;
//!
//! edge_export! {
//!     pub fn add(a: i64, b: i64) -> i64 { a + b }
//! }
//!
//! edge_export! {
//!     pub fn area(r: f64) -> f64 { 3.14159 * r * r }
//! }
//!
//! edge_export! {
//!     pub fn even(n: i64) -> bool { n % 2 == 0 }
//! }
//! ```
//!
//! Build for wasm:
//!
//! ```text
//! cargo build --release --target wasm32-unknown-unknown
//! ```
//!
//! The resulting `.wasm` is loadable by any Edge Python host that implements
//! the WASM loader pattern (see `compiler/tests/loaders.rs` for the reference
//! implementation used by the integration test suite).
//!
//! # Supported types
//!
//! | Rust type | EdgePython type | Encoding                      |
//! |-----------|-----------------|-------------------------------|
//! | `i64`     | `int`           | NaN-boxed sign-extended 47-bit |
//! | `f64`     | `float`         | raw `f64::to_bits()`          |
//! | `bool`    | `bool`          | NaN-boxed True/False tag      |
//!
//! Strings, lists, and other heap types require a buffer protocol with
//! linear-memory cooperation and are deliberately not in v1 of this SDK —
//! a host wanting to round-trip them today should expose them as ints
//! (handle-to-host-heap) or as multiple scalar args.

#![no_std]

/* ─── Wire format ─────────────────────────────────────────────────────────── */

/* Tag bits — must stay in sync with `vm::types`. */
const QNAN:      u64 = 0x7FFC_0000_0000_0000;
const SIGN:      u64 = 0x8000_0000_0000_0000;
const TAG_INT:   u64 = QNAN | SIGN;
const TAG_TRUE:  u64 = QNAN | 2;
const TAG_FALSE: u64 = QNAN | 3;
/* Canonical NaN, identical to `Val::CANON_NAN`, used so any NaN we emit
   doesn't collide with the QNAN-tag pattern that marks non-float values. */
const CANON_NAN: u64 = 0x7FF8_0000_0000_0000;

/// Decode a wire `u64` as `i64`. Sign-extends the 48-bit payload.
#[inline(always)]
pub fn unpack_int(v: u64) -> i64 {
    let raw = (v & 0x0000_FFFF_FFFF_FFFF) as i64;
    (raw << 16) >> 16
}

/// Encode an `i64` as the wire `u64`. Matches `Val::int(i)`.
#[inline(always)]
pub fn pack_int(i: i64) -> u64 {
    TAG_INT | (i as u64 & 0x0000_FFFF_FFFF_FFFF)
}

/// Decode a wire `u64` as `f64`. Non-NaN floats round-trip via `from_bits`.
#[inline(always)]
pub fn unpack_float(v: u64) -> f64 { f64::from_bits(v) }

/// Encode an `f64` as the wire `u64`. Canonicalises NaN so the result never
/// collides with a tagged non-float Val.
#[inline(always)]
pub fn pack_float(f: f64) -> u64 {
    let bits = f.to_bits();
    if (bits & QNAN) == QNAN { CANON_NAN } else { bits }
}

/// Decode a wire `u64` as `bool`. Anything that isn't `True` is treated as
/// `False`, matching Edge Python's `truthy` semantics for non-bool inputs.
#[inline(always)]
pub fn unpack_bool(v: u64) -> bool { v == TAG_TRUE }

/// Encode a `bool` as the wire `u64`.
#[inline(always)]
pub fn pack_bool(b: bool) -> u64 { if b { TAG_TRUE } else { TAG_FALSE } }

/* ─── Trait-based dispatch ────────────────────────────────────────────────── */

/// Implemented by every Rust type the SDK can decode from the wire.
pub trait FromWire: Sized { fn from_wire(u: u64) -> Self; }
impl FromWire for i64  { #[inline] fn from_wire(u: u64) -> Self { unpack_int(u) } }
impl FromWire for f64  { #[inline] fn from_wire(u: u64) -> Self { unpack_float(u) } }
impl FromWire for bool { #[inline] fn from_wire(u: u64) -> Self { unpack_bool(u) } }

/// Implemented by every Rust type the SDK can encode onto the wire.
pub trait IntoWire { fn into_wire(self) -> u64; }
impl IntoWire for i64  { #[inline] fn into_wire(self) -> u64 { pack_int(self) } }
impl IntoWire for f64  { #[inline] fn into_wire(self) -> u64 { pack_float(self) } }
impl IntoWire for bool { #[inline] fn into_wire(self) -> u64 { pack_bool(self) } }

/* ─── The macro ───────────────────────────────────────────────────────────── */

/// Wrap a Rust function so it's callable from Edge Python via the WASM loader.
///
/// The wrapped function appears in the WASM module's exports under its given
/// name, with a C ABI signature that takes/returns `u64` for each argument
/// (the wire format for `Val`). The macro inserts the type-specific decode
/// for each parameter and the encode for the return value.
///
/// Supported parameter and return types: any combination of `i64`, `f64`,
/// `bool`. Adding a new scalar type means one `impl FromWire` and one
/// `impl IntoWire` — the macro is type-agnostic.
#[macro_export]
macro_rules! edge_export {
    ($vis:vis fn $name:ident($($arg:ident : $ty:ty),* $(,)?) -> $ret:ty $body:block) => {
        #[unsafe(no_mangle)]
        $vis extern "C" fn $name($($arg: u64),*) -> u64 {
            $(let $arg: $ty = <$ty as $crate::FromWire>::from_wire($arg);)*
            let __edge_result: $ret = { $body };
            <$ret as $crate::IntoWire>::into_wire(__edge_result)
        }
    };
}
