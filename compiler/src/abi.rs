/* 
Sealed contract for modules.
  Op codes, tags, error kinds, HandleTable, ErrorStash, and primitive codec are frozen.
  Bug fixes only. New capabilities via new Op values — never new imports or signature changes.
  See documentation/reference/wasm-abi.md for the user-facing spec. 
*/

use alloc::{string::String, vec::Vec};

/* Source-of-truth NaN-boxing layout. Both the wire codec below and
   vm::types::Val import from here, so any change touches one site
   instead of three. Reserved for the `Sealed contract — v1` set: a
   layout change forces a wasm-abi version bump. */
pub mod nan_box {
    pub const QNAN: u64        = 0x7FFC_0000_0000_0000;
    pub const SIGN: u64        = 0x8000_0000_0000_0000;
    pub const TAG_UNDEF: u64   = QNAN;
    pub const TAG_NONE: u64    = QNAN | 1;
    pub const TAG_TRUE: u64    = QNAN | 2;
    pub const TAG_FALSE: u64   = QNAN | 3;
    pub const TAG_INT: u64     = QNAN | SIGN;
    pub const TAG_HEAP: u64    = QNAN | 4;
    /* 47-bit signed integer payload mask (two's-complement, sign bit at bit 47). */
    pub const INT_PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
}

/* Op codes (sealed) */

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Op {
    Call = 0,
    GetAttr = 1,
    SetAttr = 2,
    GetItem = 3,
    SetItem = 4,
    Len = 5,
    Iter = 6,
    IterNext = 7,
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

/* Tags (sealed) */

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Tag {
    None = 0,
    Bool = 1,
    Int = 2,
    Float = 3,
    // UTF-8 bytes: encoder builds a str, decoder returns its bytes.
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

// edge_decode sentinel: invalid handle or non-primitive; caller should use edge_op.
pub const TAG_INVALID: u32 = u32::MAX;

/* Error kinds (sealed) */

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ErrorKind {
    Type = 0,
    Value = 1,
    Runtime = 2,
    Attribute = 3,
    Index = 4,
    Key = 5,
    Custom = 6,
}

/* Handle table */

// Handle slot; rc=0 means free. Exposed handle = index+1 (0 reserved as invalid).
struct HandleSlot {
    // Raw Val bits; ABI never inspects — encode/decode go through classify_*.
    val: u64,
    rc: u32,
}

// Refcounted u32->u64 (Val bits) map; single instance per script run, cleared between runs.
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

    // Reset to empty state. Called by the host between runs.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.free_list.clear();
    }

    // Register a value. Returns a fresh handle (rc=1).
    pub fn put(&mut self, val: u64) -> u32 {
        if let Some(idx) = self.free_list.pop() {
            self.slots[idx as usize] = HandleSlot { val, rc: 1 };
            idx + 1
        } else {
            self.slots.push(HandleSlot { val, rc: 1 });
            self.slots.len() as u32
        }
    }

    // Look up a value by handle, or `None` if invalid / freed.
    pub fn get(&self, h: u32) -> Option<u64> {
        if h == 0 { return None; }
        self.slots.get((h - 1) as usize)
            .filter(|s| s.rc > 0)
            .map(|s| s.val)
    }

    // Decrements rc; frees slot at 0. Safe against double-release.
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

/* Error stash */

// Single-slot error stash drained by edge_take_error; populated by dispatch failures and edge_throw.
#[derive(Default)]
pub struct ErrorStash(Option<(u32, String)>);

impl ErrorStash {
    pub const fn new() -> Self { Self(None) }
    pub fn clear(&mut self) { self.0 = None; }

    // Replace any pending error with `(kind, msg)`.
    pub fn set(&mut self, kind: u32, msg: String) {
        self.0 = Some((kind, msg));
    }

    // Convenience: stash a typed error.
    pub fn set_typed(&mut self, kind: ErrorKind, msg: String) {
        self.0 = Some((kind as u32, msg));
    }

    // Take the error if present.
    pub fn take(&mut self) -> Option<(u32, String)> { self.0.take() }

    // Peeks without consuming; lets edge_take_error retry on buffer-too-small.
    pub fn peek(&self) -> Option<(u32, &str)> {
        self.0.as_ref().map(|(k, m)| (*k, m.as_str()))
    }
}

/* Primitive codec helpers */

// edge_encode decision: Direct=primitive Val bits, AllocStr=host heap alloc, Invalid=malformed.
pub enum EncodeRequest<'a> {
    Direct(u64),
    AllocStr(&'a str),
    Invalid,
}

// Maps (tag, bytes) to EncodeRequest. NaN-boxing layout is sealed in `nan_box`; changes require ABI bump.
pub fn classify_encode(tag: u32, bytes: &[u8]) -> EncodeRequest<'_> {
    use nan_box::*;

    match Tag::from_u32(tag) {
        Some(Tag::None) => EncodeRequest::Direct(TAG_NONE),
        Some(Tag::Bool) => {
            let b = !bytes.is_empty() && bytes[0] != 0;
            EncodeRequest::Direct(if b { TAG_TRUE } else { TAG_FALSE })
        }
        Some(Tag::Int) => {
            if bytes.len() != 8 { return EncodeRequest::Invalid; }
            let mut buf = [0u8; 8];
            buf.copy_from_slice(bytes);
            let i = i64::from_le_bytes(buf);
            EncodeRequest::Direct(TAG_INT | (i as u64 & INT_PAYLOAD_MASK))
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

// edge_decode decision: Primitive=ready bytes, Heap=host materializes, Invalid=malformed Val.
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

// Classifies Val bits into Primitive/Heap/Invalid; Heap means host must read from HeapPool.
pub fn classify_decode(val_bits: u64) -> DecodeBits {
    use nan_box::*;

    // Float: any non-QNAN-tagged pattern.
    if (val_bits & QNAN) != QNAN {
        return DecodeBits::Primitive {
            tag: Tag::Float as u32,
            bytes: PrimitiveBytes::Eight(f64::from_bits(val_bits).to_le_bytes()),
        };
    }
    // Int: QNAN|SIGN with payload.
    if (val_bits & (QNAN | SIGN)) == TAG_INT {
        let raw = (val_bits & INT_PAYLOAD_MASK) as i64;
        let sign_extended = (raw << 16) >> 16;
        return DecodeBits::Primitive {
            tag: Tag::Int as u32,
            bytes: PrimitiveBytes::Eight(sign_extended.to_le_bytes()),
        };
    }
    // Singletons and heap handles.
    let lower = val_bits & 0xF;
    if (val_bits & QNAN) == QNAN && (val_bits & SIGN) == 0 {
        if val_bits == TAG_NONE {
            return DecodeBits::Primitive {
                tag: Tag::None as u32, bytes: PrimitiveBytes::None,
            };
        }
        if val_bits == TAG_TRUE {
            return DecodeBits::Primitive {
                tag: Tag::Bool as u32, bytes: PrimitiveBytes::Bool(1),
            };
        }
        if val_bits == TAG_FALSE {
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
