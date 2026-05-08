/* ===========================================================================
 * EDGE PYTHON  —  WASM MODULE ABI v1  (sealed contract)
 * ===========================================================================
 *
 *   ┌──────────────────────────────────────────────────────────────────┐
 *   │                  S E A L E D   C O N T R A C T                   │
 *   ├──────────────────────────────────────────────────────────────────┤
 *   │                                                                  │
 *   │  This module defines the Edge Python wasm-abi v1. The op codes,  │
 *   │  tag values, error kinds, and primitive ABI helpers below form   │
 *   │  the public contract every host that loads a guest `.wasm`       │
 *   │  module must honour, and every guest module already in the wild  │
 *   │  relies on.                                                      │
 *   │                                                                  │
 *   │  DO NOT MODIFY the numeric values, the function signatures, or   │
 *   │  the layout of `HandleTable` / `ErrorStash`.                     │
 *   │                                                                  │
 *   │  Bug fixes — correcting divergences from the contract — are the  │
 *   │  only acceptable maintenance. New capabilities arrive as new     │
 *   │  values inside the existing `Op` enum (consumed by `edge_op`),   │
 *   │  never as new imports or signature breaks.                       │
 *   │                                                                  │
 *   │  Lives in `compiler/src/abi.rs`. The orchestration that wires    │
 *   │  this module to the Edge Python parser/VM is in `main.rs`. The   │
 *   │  reference author-side SDK is the `edge-pdk` crate. See          │
 *   │  `documentation/reference/wasm-abi.md` for the user-facing spec  │
 *   │  and worked examples (Rust + Python).                            │
 *   │                                                                  │
 *   └──────────────────────────────────────────────────────────────────┘
 *
 * GUEST EXPORT SHAPE
 *
 *   extern "C" fn <name>(argv: *const u32, argc: u32, out: *mut u32) -> i32;
 *
 *     argv  : pointer (in guest linear memory) to `argc` opaque host-managed
 *             handles (u32) — one per positional argument.
 *     argc  : positional argument count.
 *     out   : pointer (in guest linear memory) where the guest writes ONE
 *             handle for the return value.
 *     return: 0 = success, 1 = error (host pulls via `edge_take_error`).
 *
 *   Plus the obligatory:
 *
 *   extern "C" fn __edge_alloc(size: u32) -> *mut u8;
 *
 *     Used by the host shim to stage argv arrays in the guest's linear
 *     memory before invoking each export.
 *
 * GUEST-SIDE IMPORTS  (from `env`)
 *
 *   fn edge_op(op, recv, name_ptr, name_len, argv_ptr, argc, out) -> i32;
 *   fn edge_encode(tag, ptr, len) -> u32;
 *   fn edge_decode(h, out_tag, dst, dst_max) -> i32;
 *   fn edge_release(h);
 *   fn edge_throw(kind, msg_ptr, msg_len);
 *   fn edge_take_error(out_kind, dst, dst_max) -> i32;
 *
 *   These six functions are the totality of the wire. Their full text is
 *   at `documentation/reference/wasm-abi.md`.
 *
 * THIS MODULE'S ROLE
 *
 *   `abi` is the host-internal, VM-agnostic half of the contract: it
 *   owns the sealed numeric values (Op / Tag / ErrorKind), the handle
 *   table (refcounted u32 → u64 Val bits), the error stash, and the
 *   primitive codec for None / Bool / Int / Float / Bytes.
 *
 *   `main.rs` is the WASM orchestration that injects this module as a
 *   dependency: it owns the WasmHostResolver, the parser/VM lifecycle,
 *   the JS imports (js_print / js_call_native / js_fetch_bytes), and
 *   the VM-coupled dispatch (Op::Call → method lookup, etc.). The split
 *   keeps the contract free of VM-specific churn — extending the
 *   parser, retiring opcodes from the VM, or swapping out the heap
 *   layout never requires touching this file.
 * =========================================================================== */

use alloc::{string::String, vec::Vec};

/* ---------- Op codes (sealed) --------------------------------------- */

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Op {
    Call      = 0,
    GetAttr   = 1,
    SetAttr   = 2,
    GetItem   = 3,
    SetItem   = 4,
    Len       = 5,
    Iter      = 6,
    IterNext  = 7,
}

impl Op {
    pub fn from_u32(op: u32) -> Option<Self> {
        match op {
            0 => Some(Self::Call),
            1 => Some(Self::GetAttr),
            2 => Some(Self::SetAttr),
            3 => Some(Self::GetItem),
            4 => Some(Self::SetItem),
            5 => Some(Self::Len),
            6 => Some(Self::Iter),
            7 => Some(Self::IterNext),
            _ => None,
        }
    }
}

/* ---------- Tags (sealed) ------------------------------------------- */

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Tag {
    None  = 0,
    Bool  = 1,
    Int   = 2,
    Float = 3,
    /// UTF-8 bytes; encoder builds a `str`, decoder of a `str` returns
    /// its bytes.
    Bytes = 4,
}

impl Tag {
    pub fn from_u32(t: u32) -> Option<Self> {
        match t {
            0 => Some(Self::None),
            1 => Some(Self::Bool),
            2 => Some(Self::Int),
            3 => Some(Self::Float),
            4 => Some(Self::Bytes),
            _ => None,
        }
    }
}

/// Sentinel tag returned by `edge_decode` for invalid handles or
/// non-primitive values (caller should use `edge_op` instead).
pub const TAG_INVALID: u32 = u32::MAX;

/* ---------- Error kinds (sealed) ------------------------------------ */

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ErrorKind {
    Type      = 0,
    Value     = 1,
    Runtime   = 2,
    Attribute = 3,
    Index     = 4,
    Key       = 5,
    Custom    = 6,
}

/* ---------- Handle table -------------------------------------------- */

/// One slot in the handle table. `rc=0` means the slot is on the free
/// list. The exposed handle is `index + 1` so handle `0` reserves
/// "invalid".
struct HandleSlot {
    /// Raw u64 representation of the host's `Val` type. The ABI module
    /// does not inspect it — encode and decode go through `classify_*`.
    val: u64,
    rc: u32,
}

/// Refcounted mapping `u32 → u64` (host-side Val bits). Process-wide;
/// the host typically holds a single instance for the lifetime of a
/// script run and clears it via `reset_modules()`.
pub struct HandleTable {
    slots: Vec<HandleSlot>,
    free_list: Vec<u32>,
}

impl Default for HandleTable {
    fn default() -> Self { Self::new() }
}

impl HandleTable {
    pub const fn new() -> Self {
        Self { slots: Vec::new(), free_list: Vec::new() }
    }

    /// Reset to empty state. Called by the host between runs.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.free_list.clear();
    }

    /// Register a value. Returns a fresh handle (rc=1).
    pub fn put(&mut self, val: u64) -> u32 {
        if let Some(idx) = self.free_list.pop() {
            self.slots[idx as usize] = HandleSlot { val, rc: 1 };
            idx + 1
        } else {
            self.slots.push(HandleSlot { val, rc: 1 });
            self.slots.len() as u32
        }
    }

    /// Look up a value by handle, or `None` if invalid / freed.
    pub fn get(&self, h: u32) -> Option<u64> {
        if h == 0 { return None; }
        self.slots.get((h - 1) as usize)
            .filter(|s| s.rc > 0)
            .map(|s| s.val)
    }

    /// Decrement refcount; free the slot when it reaches 0. Defensive
    /// against double-release.
    pub fn release(&mut self, h: u32) {
        if h == 0 { return; }
        let idx = (h - 1) as usize;
        if let Some(slot) = self.slots.get_mut(idx)
            && slot.rc > 0
        {
            slot.rc -= 1;
            if slot.rc == 0 { self.free_list.push(idx as u32); }
        }
    }
}

/* ---------- Error stash --------------------------------------------- */

/// Single-slot stash drained by `edge_take_error`. The host populates
/// it from its own dispatch failures and from `edge_throw` calls.
#[derive(Default)]
pub struct ErrorStash(Option<(u32, String)>);

impl ErrorStash {
    pub const fn new() -> Self { Self(None) }
    pub fn clear(&mut self) { self.0 = None; }

    /// Replace any pending error with `(kind, msg)`.
    pub fn set(&mut self, kind: u32, msg: String) {
        self.0 = Some((kind, msg));
    }

    /// Convenience: stash a typed error.
    pub fn set_typed(&mut self, kind: ErrorKind, msg: String) {
        self.0 = Some((kind as u32, msg));
    }

    /// Take the error if present.
    pub fn take(&mut self) -> Option<(u32, String)> { self.0.take() }

    /// Peek without consuming. Used by `edge_take_error`'s
    /// "buffer too small" path so the error stays pending for retry.
    pub fn peek(&self) -> Option<(u32, &str)> {
        self.0.as_ref().map(|(k, m)| (*k, m.as_str()))
    }
}

/* ---------- Primitive codec helpers --------------------------------- */

/// What `edge_encode` should do with the raw bytes the guest passed.
/// `Direct` is a primitive whose final Val bits are computed by this
/// module; `AllocStr` is a UTF-8 string the host must allocate on its
/// heap; `Invalid` is a malformed payload.
pub enum EncodeRequest<'a> {
    Direct(u64),
    AllocStr(&'a str),
    Invalid,
}

/// Inspect the bytes a guest passed to `edge_encode` and decide how
/// the host should materialize the value. The NaN-boxing layout for
/// None / Bool / Int / Float lives here (and only here): changing it
/// requires bumping the wasm-abi version.
pub fn classify_encode(tag: u32, bytes: &[u8]) -> EncodeRequest<'_> {
    /* NaN-boxing constants — must match the host's `Val` impl. */
    const QNAN:           u64 = 0x7FFC_0000_0000_0000;
    const TAG_NONE_BITS:  u64 = QNAN | 1;
    const TAG_TRUE_BITS:  u64 = QNAN | 2;
    const TAG_FALSE_BITS: u64 = QNAN | 3;
    const TAG_INT_BITS:   u64 = QNAN | 0x8000_0000_0000_0000;

    match Tag::from_u32(tag) {
        Some(Tag::None) => EncodeRequest::Direct(TAG_NONE_BITS),
        Some(Tag::Bool) => {
            let b = !bytes.is_empty() && bytes[0] != 0;
            EncodeRequest::Direct(if b { TAG_TRUE_BITS } else { TAG_FALSE_BITS })
        }
        Some(Tag::Int) => {
            if bytes.len() != 8 { return EncodeRequest::Invalid; }
            let mut buf = [0u8; 8];
            buf.copy_from_slice(bytes);
            let i = i64::from_le_bytes(buf);
            EncodeRequest::Direct(TAG_INT_BITS | (i as u64 & 0x0000_FFFF_FFFF_FFFF))
        }
        Some(Tag::Float) => {
            if bytes.len() != 8 { return EncodeRequest::Invalid; }
            let mut buf = [0u8; 8];
            buf.copy_from_slice(bytes);
            EncodeRequest::Direct(f64::from_le_bytes(buf).to_bits())
        }
        Some(Tag::Bytes) => match core::str::from_utf8(bytes) {
            Ok(s) => EncodeRequest::AllocStr(s),
            Err(_) => EncodeRequest::Invalid,
        },
        None => EncodeRequest::Invalid,
    }
}

/// What `edge_decode` should do with a Val u64. `Primitive` returns the
/// bytes ready to copy into the guest's buffer; `Heap` defers to the
/// host (the value is heap-resident — the host materializes its bytes,
/// e.g. UTF-8 for a `str`); `Invalid` is a malformed Val.
pub enum DecodeBits {
    Primitive { tag: u32, bytes: PrimitiveBytes },
    Heap,
    Invalid,
}

pub enum PrimitiveBytes {
    None,
    Bool(u8),
    Eight([u8; 8]),
}

impl PrimitiveBytes {
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::None => &[],
            Self::Bool(b) => core::slice::from_ref(b),
            Self::Eight(a) => a.as_slice(),
        }
    }
}

/// Inspect raw Val bits to extract the primitive kind. Returns
/// `DecodeBits::Heap` for QNAN-tagged heap handles — the host must
/// consult its own heap to materialize the value (e.g. read the bytes
/// of a `Str` from `HeapPool`).
pub fn classify_decode(val_bits: u64) -> DecodeBits {
    /* Same NaN-boxing constants as `classify_encode`. */
    const QNAN: u64 = 0x7FFC_0000_0000_0000;
    const SIGN: u64 = 0x8000_0000_0000_0000;
    const TAG_INT: u64 = QNAN | SIGN;

    // Float: any pattern that ISN'T QNAN-tagged.
    if (val_bits & QNAN) != QNAN {
        return DecodeBits::Primitive {
            tag: Tag::Float as u32,
            bytes: PrimitiveBytes::Eight(f64::from_bits(val_bits).to_le_bytes()),
        };
    }
    // Int: QNAN | SIGN with payload.
    if (val_bits & (QNAN | SIGN)) == TAG_INT {
        let raw = (val_bits & 0x0000_FFFF_FFFF_FFFF) as i64;
        let sign_extended = (raw << 16) >> 16;
        return DecodeBits::Primitive {
            tag: Tag::Int as u32,
            bytes: PrimitiveBytes::Eight(sign_extended.to_le_bytes()),
        };
    }
    // Singletons (None / True / False) and heap handles.
    let lower = val_bits & 0xF;
    if (val_bits & QNAN) == QNAN && (val_bits & SIGN) == 0 {
        if val_bits == QNAN | 1 {
            return DecodeBits::Primitive {
                tag: Tag::None as u32, bytes: PrimitiveBytes::None,
            };
        }
        if val_bits == QNAN | 2 {
            return DecodeBits::Primitive {
                tag: Tag::Bool as u32, bytes: PrimitiveBytes::Bool(1),
            };
        }
        if val_bits == QNAN | 3 {
            return DecodeBits::Primitive {
                tag: Tag::Bool as u32, bytes: PrimitiveBytes::Bool(0),
            };
        }
        if lower >= 4 {
            return DecodeBits::Heap;
        }
    }
    DecodeBits::Invalid
}
