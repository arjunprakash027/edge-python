//! Edge Python Plugin Development Kit (PDK) — author-side runtime for
//! writing native `.wasm` modules importable from Edge Python scripts.
//!
//! This crate is the recommended Rust layer on top of the v1 wasm-abi
//! (see `documentation/reference/wasm-abi.md`). It provides:
//!
//!   * The six host imports (`edge_op`, `edge_encode`, `edge_decode`,
//!     `edge_release`, `edge_throw`, `edge_take_error`) declared as
//!     `extern "C"` and wrapped in safe helpers.
//!   * `Handle` — an opaque RAII wrapper around a host-managed handle.
//!     Drop releases the underlying refcount automatically.
//!   * `Value` — a typed enum over the bootstrap codec (Int, Float,
//!     Bool, None, Bytes/String).
//!   * `Error` / `Result` — the typed error channel; round-trips through
//!     `edge_throw` / `edge_take_error`.
//!   * `FromValue` / `IntoValue` traits with primitive impls (`i64`,
//!     `f64`, `bool`, `String`, `&str`, `Option<T>`, `Handle`).
//!   * The `__edge_alloc` export the host shim needs for argv staging
//!     (lives in the hidden `__internals` module so glob imports stay clean).
//!
//! Author code:
//!
//! ```ignore
//! use edge_pdk::*;
//!
//! #[plugin_fn]
//! fn slugify(s: String) -> String {
//!     s.to_lowercase().replace(' ', "-")
//! }
//! ```
//!
//! The `#[plugin_fn]` attribute lives in the internal `edge-pdk-macros`
//! sub-crate and is re-exported from here.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub use edge_pdk_macros::plugin_fn;

/* Curated public surface for plugin authors. Glob-importing the whole
   crate exposes #[doc(hidden)] symbols (`__edge_alloc`, `__internals`)
   which are part of the macro contract, not the user API. The prelude
   re-exports just what `#[plugin_fn]` expansion needs and what most
   plugins reach for: type wrappers, the attribute, the trait pair.
   Recommended: `use edge_pdk::prelude::*;`. */
pub mod prelude {
    pub use crate::{plugin_fn, Handle, Value, Error, Result, FromValue, IntoValue};
}

/* ---------- Plugin bootstrap ----------------------------------------- */

/* Re-exported under a hidden path so `module!` can name lol_alloc without
   forcing the plugin author to add it to their own Cargo.toml. */
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub use lol_alloc as __lol_alloc;

/* Emits the wasm32-only boilerplate every Edge Python plugin needs:
     - a #[global_allocator] backed by lol_alloc::LeakingPageAllocator
       (single-threaded bump allocator that matches the host model),
     - a #[panic_handler] that traps via wasm32::unreachable.

   The plugin author still writes #![no_std] / #![no_main] / extern crate
   alloc; at the crate root — those are crate-level attributes the macro
   cannot inject from inside an item position.

   Usage:
     edge_pdk::module!();

   On non-wasm targets (e.g. host-side unit tests for the plugin) the
   macro expands to nothing so cargo test still works. */
#[macro_export]
macro_rules! module {
    () => {
        #[cfg(target_arch = "wasm32")]
        #[global_allocator]
        static __EDGE_PDK_ALLOC: $crate::__lol_alloc::LeakingPageAllocator
            = $crate::__lol_alloc::LeakingPageAllocator;

        #[cfg(target_arch = "wasm32")]
        #[panic_handler]
        fn __edge_pdk_panic(_: &core::panic::PanicInfo) -> ! {
            core::arch::wasm32::unreachable()
        }
    };
}

use alloc::{string::String, vec::Vec};

/* ---------- Wire imports --------------------------------------------- */

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    pub fn edge_op(
        op: u32,
        recv: u32,
        name_ptr: *const u8,
        name_len: u32,
        argv_ptr: *const u32,
        argc: u32,
        out: *mut u32,
    ) -> i32;
    pub fn edge_encode(tag: u32, ptr: *const u8, len: u32) -> u32;
    pub fn edge_decode(
        h: u32,
        out_tag: *mut u32,
        dst: *mut u8,
        dst_max: u32,
    ) -> i32;
    pub fn edge_release(h: u32);
    pub fn edge_take_error(
        out_kind: *mut u32,
        dst: *mut u8,
        dst_max: u32,
    ) -> i32;
    pub fn edge_throw(kind: u32, msg_ptr: *const u8, msg_len: u32);
}

/* ---------- ABI version handshake ------------------------------------ */

/* Wire-format version this PDK targets. Bump on any breaking change to
   op codes, value tags, codec layout, or error kinds. The host loader
   reads `__edge_abi_version` and refuses to instantiate a plugin whose
   version it does not understand — without this, an evolved host would
   load an old plugin and decode garbage silently. */
pub const EDGE_ABI_VERSION: u32 = 1;

#[unsafe(no_mangle)]
pub extern "C" fn __edge_abi_version() -> u32 { EDGE_ABI_VERSION }

/* ---------- Op codes & tags (must match bridge.rs spec) -------------- */

#[allow(non_camel_case_types)]
pub mod op {
    pub const CALL: u32      = 0;
    pub const GET_ATTR: u32  = 1;
    pub const SET_ATTR: u32  = 2;
    pub const GET_ITEM: u32  = 3;
    pub const SET_ITEM: u32  = 4;
    pub const LEN: u32       = 5;
    pub const ITER: u32      = 6;
    pub const ITER_NEXT: u32 = 7;
}

#[allow(non_camel_case_types)]
pub mod tag {
    pub const NONE: u32  = 0;
    pub const BOOL: u32  = 1;
    pub const INT: u32   = 2;
    pub const FLOAT: u32 = 3;
    pub const BYTES: u32 = 4;
}

/* ---------- Internals — macro contract surface, not user API --------- */

/* Sub-module so `use edge_pdk::*;` cannot pull these into a plugin
   author's namespace. The `#[plugin_fn]` expansion qualifies the path
   explicitly (`::edge_pdk::__internals::stash_error`), and `__edge_alloc`
   stays a no_mangle WASM export regardless of Rust module nesting. */
#[doc(hidden)]
pub mod __internals {
    use super::Error;
    use alloc::string::ToString;

    /* Used by #[plugin_fn] expansion when a user fn returns Err(_). */
    pub fn stash_error(e: Error) {
        let kind = e.kind();
        let msg = e.message().to_string();
        unsafe { super::edge_throw(kind, msg.as_ptr(), msg.len() as u32); }
    }

    /* Host-side argv stager. The shim allocates space in this module's
       linear memory before invoking each export; the layout is
       [u32; argc] for argv and a single u32 for `out`. We use a leak-free
       bump scheme — every call lives entirely on the heap, so the leak is
       reclaimed when the WASM instance is torn down. */
    #[unsafe(no_mangle)]
    pub extern "C" fn __edge_alloc(size: u32) -> *mut u8 {
        let v = alloc::vec![0u8; size as usize];
        alloc::boxed::Box::into_raw(v.into_boxed_slice()) as *mut u8
    }
}

/* ---------- Errors --------------------------------------------------- */

#[derive(Debug, Clone)]
pub enum Error {
    Type(String),
    Value(String),
    Runtime(String),
    Attribute(String),
    Index(String),
    Key(String),
    Custom { kind: u32, message: String },
}

impl Error {
    pub fn message(&self) -> &str {
        match self {
            Self::Type(s) | Self::Value(s) | Self::Runtime(s)
            | Self::Attribute(s) | Self::Index(s) | Self::Key(s) => s,
            Self::Custom { message, .. } => message,
        }
    }
    pub fn kind(&self) -> u32 {
        match self {
            Self::Type(_)      => 0,
            Self::Value(_)     => 1,
            Self::Runtime(_)   => 2,
            Self::Attribute(_) => 3,
            Self::Index(_)     => 4,
            Self::Key(_)       => 5,
            Self::Custom { kind, .. } => *kind,
        }
    }
    pub fn from_kind(kind: u32, message: String) -> Self {
        match kind {
            0 => Self::Type(message),
            1 => Self::Value(message),
            2 => Self::Runtime(message),
            3 => Self::Attribute(message),
            4 => Self::Index(message),
            5 => Self::Key(message),
            _ => Self::Custom { kind, message },
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;

/// Drain the host's stashed error after a 1-returning edge_op. Always
/// returns Some — a bare 1 status without a stashed error is an ABI
/// violation; we surface it as a Runtime error.
pub fn last_error() -> Error {
    let mut kind: u32 = 2;
    let mut buf = alloc::vec![0u8; 256];
    loop {
        let r = unsafe {
            edge_take_error(&mut kind as *mut u32, buf.as_mut_ptr(), buf.len() as u32)
        };
        if r >= 0 {
            buf.truncate(r as usize);
            let msg = String::from_utf8_lossy(&buf).into_owned();
            return Error::from_kind(kind, msg);
        }
        if r == -1 { return Error::Runtime(String::from("native error: no message")); }
        // Negative = -needed. Re-allocate and retry.
        buf.resize((-r) as usize, 0);
    }
}

/* ---------- Handle (RAII) -------------------------------------------- */

/// Opaque host-managed reference to an Edge Python value. Carries an
/// `owned` flag distinguishing handles the guest must release on drop
/// (created by `edge_encode` or `edge_op`) from handles the host owns
/// for the duration of a call (argv slots).
pub struct Handle { raw: u32, owned: bool }

impl Handle {
    /// Wrap an argv handle WITHOUT releasing on drop. The host owns
    /// argv handles for the duration of the call.
    pub fn borrow(raw: u32) -> Self { Self { raw, owned: false } }
    /// Take ownership of a raw handle. Drop will release the refcount.
    pub fn from_raw(raw: u32) -> Self { Self { raw, owned: true } }
    /// Surrender the handle without running its Drop — used when the
    /// wrapper transfers ownership back to the host (writing to *out).
    pub fn into_raw(self) -> u32 {
        let r = self.raw;
        core::mem::forget(self);
        r
    }
    pub fn raw(&self) -> u32 { self.raw }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if self.owned && self.raw != 0 { unsafe { edge_release(self.raw) } }
    }
}

impl FromValue for Handle {
    fn from_handle(h: u32) -> Result<Self> { Ok(Handle::borrow(h)) }
}
impl IntoValue for Handle {
    fn into_handle(self) -> Result<Handle> { Ok(self) }
}

/* ---------- Bootstrap codec ----------------------------------------- */

/// Decode a handle into a typed Value. Returns Err if the handle names
/// a composite (list/dict/etc.) — those should be operated on via
/// `Handle::call` etc.
pub fn decode(h: u32) -> Result<Value> {
    let mut tag: u32 = 0;
    let mut buf = alloc::vec![0u8; 256];
    loop {
        let r = unsafe {
            edge_decode(h, &mut tag as *mut u32, buf.as_mut_ptr(), buf.len() as u32)
        };
        if r >= 0 {
            if tag == u32::MAX {
                return Err(Error::Type(alloc::string::String::from(
                    "value is not a primitive (use Handle::call for composites)")));
            }
            buf.truncate(r as usize);
            return Ok(match tag {
                0 => Value::None,
                1 => Value::Bool(buf[0] != 0),
                2 => {
                    let mut a = [0u8; 8];
                    a.copy_from_slice(&buf[..8]);
                    Value::Int(i64::from_le_bytes(a))
                }
                3 => {
                    let mut a = [0u8; 8];
                    a.copy_from_slice(&buf[..8]);
                    Value::Float(f64::from_le_bytes(a))
                }
                4 => Value::Bytes(buf),
                _ => return Err(Error::Type(alloc::string::String::from("unknown tag"))),
            });
        }
        // Negative = -needed.
        buf.resize((-r) as usize, 0);
    }
}

/// Encode a primitive into a handle.
pub fn encode(v: Value) -> Result<Handle> {
    let raw = match v {
        Value::None => unsafe { edge_encode(tag::NONE, core::ptr::null(), 0) },
        Value::Bool(b) => {
            let buf = [if b { 1u8 } else { 0u8 }];
            unsafe { edge_encode(tag::BOOL, buf.as_ptr(), 1) }
        }
        Value::Int(i) => {
            let buf = i.to_le_bytes();
            unsafe { edge_encode(tag::INT, buf.as_ptr(), 8) }
        }
        Value::Float(f) => {
            let buf = f.to_le_bytes();
            unsafe { edge_encode(tag::FLOAT, buf.as_ptr(), 8) }
        }
        Value::Bytes(b) => unsafe { edge_encode(tag::BYTES, b.as_ptr(), b.len() as u32) },
    };
    if raw == 0 { Err(Error::Runtime(alloc::string::String::from("encode failed"))) }
    else { Ok(Handle::from_raw(raw)) }
}

/// Typed primitive value — the bootstrap codec's basis. Composite types
/// (list, dict, set, instances) are accessed through `Handle::call`.
#[derive(Debug, Clone)]
pub enum Value {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// UTF-8 bytes when produced from a `str`; raw bytes otherwise.
    Bytes(Vec<u8>),
}

/* ---------- FromValue / IntoValue ------------------------------------ */

pub trait FromValue: Sized {
    fn from_handle(h: u32) -> Result<Self>;
}

pub trait IntoValue {
    fn into_handle(self) -> Result<Handle>;
}

impl FromValue for () {
    fn from_handle(h: u32) -> Result<Self> {
        match decode(h)? {
            Value::None => Ok(()),
            v => Err(Error::Type(alloc::format!("expected None, got {:?}", v))),
        }
    }
}
impl IntoValue for () {
    fn into_handle(self) -> Result<Handle> { encode(Value::None) }
}

impl FromValue for bool {
    fn from_handle(h: u32) -> Result<Self> {
        match decode(h)? {
            Value::Bool(b) => Ok(b),
            v => Err(Error::Type(alloc::format!("expected bool, got {:?}", v))),
        }
    }
}
impl IntoValue for bool {
    fn into_handle(self) -> Result<Handle> { encode(Value::Bool(self)) }
}

impl FromValue for i64 {
    fn from_handle(h: u32) -> Result<Self> {
        match decode(h)? {
            Value::Int(i) => Ok(i),
            v => Err(Error::Type(alloc::format!("expected int, got {:?}", v))),
        }
    }
}
impl IntoValue for i64 {
    fn into_handle(self) -> Result<Handle> { encode(Value::Int(self)) }
}

impl FromValue for f64 {
    fn from_handle(h: u32) -> Result<Self> {
        match decode(h)? {
            Value::Float(f) => Ok(f),
            Value::Int(i) => Ok(i as f64),
            v => Err(Error::Type(alloc::format!("expected float, got {:?}", v))),
        }
    }
}
impl IntoValue for f64 {
    fn into_handle(self) -> Result<Handle> { encode(Value::Float(self)) }
}

impl FromValue for String {
    fn from_handle(h: u32) -> Result<Self> {
        match decode(h)? {
            Value::Bytes(b) => String::from_utf8(b)
                .map_err(|e| Error::Value(alloc::format!("invalid utf-8: {}", e))),
            v => Err(Error::Type(alloc::format!("expected str, got {:?}", v))),
        }
    }
}
impl IntoValue for String {
    fn into_handle(self) -> Result<Handle> { encode(Value::Bytes(self.into_bytes())) }
}
impl IntoValue for &str {
    fn into_handle(self) -> Result<Handle> {
        encode(Value::Bytes(self.as_bytes().to_vec()))
    }
}
impl IntoValue for alloc::borrow::Cow<'_, str> {
    fn into_handle(self) -> Result<Handle> {
        encode(Value::Bytes(self.into_owned().into_bytes()))
    }
}

impl<T: FromValue> FromValue for Option<T> {
    fn from_handle(h: u32) -> Result<Self> {
        // Decode peeks at the tag without consuming; if None, return.
        let mut tag: u32 = 0;
        let r = unsafe { edge_decode(h, &mut tag as *mut u32, core::ptr::null_mut(), 0) };
        if r >= 0 && tag == 0 { return Ok(None); }
        T::from_handle(h).map(Some)
    }
}
impl<T: IntoValue> IntoValue for Option<T> {
    fn into_handle(self) -> Result<Handle> {
        match self {
            None => encode(Value::None),
            Some(v) => v.into_handle(),
        }
    }
}

/* ---------- Universal dispatch via Handle ---------------------------- */

impl Handle {
    /// Invoke `recv.<name>(args)`. The args become a transient argv
    /// array; their handles are NOT released by this call (caller still
    /// owns them).
    pub fn call(&self, name: &str, args: &[u32]) -> Result<Handle> {
        let mut out: u32 = 0;
        let r = unsafe {
            edge_op(
                op::CALL, self.raw,
                name.as_ptr(), name.len() as u32,
                args.as_ptr(), args.len() as u32,
                &mut out as *mut u32,
            )
        };
        if r != 0 { return Err(last_error()); }
        Ok(Handle::from_raw(out))
    }

    /// `recv.<name>` — read attribute / bind builtin method.
    pub fn get_attr(&self, name: &str) -> Result<Handle> {
        let mut out: u32 = 0;
        let r = unsafe {
            edge_op(
                op::GET_ATTR, self.raw,
                name.as_ptr(), name.len() as u32,
                core::ptr::null(), 0,
                &mut out as *mut u32,
            )
        };
        if r != 0 { return Err(last_error()); }
        Ok(Handle::from_raw(out))
    }

    /// `recv[index]`.
    pub fn get_item(&self, index: u32) -> Result<Handle> {
        let argv = [index];
        let mut out: u32 = 0;
        let r = unsafe {
            edge_op(
                op::GET_ITEM, self.raw,
                core::ptr::null(), 0,
                argv.as_ptr(), 1,
                &mut out as *mut u32,
            )
        };
        if r != 0 { return Err(last_error()); }
        Ok(Handle::from_raw(out))
    }

    /// `len(recv)`.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> Result<i64> {
        let mut out: u32 = 0;
        let r = unsafe {
            edge_op(
                op::LEN, self.raw,
                core::ptr::null(), 0,
                core::ptr::null(), 0,
                &mut out as *mut u32,
            )
        };
        if r != 0 { return Err(last_error()); }
        /* Wrap into a Handle so Drop releases on every exit path, including
           the `?` from a future from_handle that fails between decode and release. */
        let h = Handle::from_raw(out);
        i64::from_handle(h.raw())
    }
}
