/* 
Sealed module contract: op codes / tags / error kinds / HandleTable / ErrorStash / primitive codec.
Wire-level constants live in `edge-abi`; the enums below mirror them byte-for-byte.
Add capabilities via new Op values, never new imports or signature changes.
See documentation/reference/wasm-abi.md for the spec. 
*/

use alloc::{string::String, vec::Vec};

pub use edge_abi::{nan_box, EDGE_ABI_VERSION, TAG_INVALID};

/* Op codes (sealed) — values mirror `edge_abi::op::*`. */

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Op {
    Call = edge_abi::op::CALL,
    GetAttr = edge_abi::op::GET_ATTR,
    SetAttr = edge_abi::op::SET_ATTR,
    GetItem = edge_abi::op::GET_ITEM,
    SetItem = edge_abi::op::SET_ITEM,
    Len = edge_abi::op::LEN,
    Iter = edge_abi::op::ITER,
    IterNext = edge_abi::op::ITER_NEXT,
}

impl Op {
    pub fn from_u32(op: u32) -> Option<Self> {
        match op {
            edge_abi::op::CALL => Some(Self::Call),
            edge_abi::op::GET_ATTR => Some(Self::GetAttr),
            edge_abi::op::SET_ATTR => Some(Self::SetAttr),
            edge_abi::op::GET_ITEM => Some(Self::GetItem),
            edge_abi::op::SET_ITEM => Some(Self::SetItem),
            edge_abi::op::LEN => Some(Self::Len),
            edge_abi::op::ITER => Some(Self::Iter),
            edge_abi::op::ITER_NEXT => Some(Self::IterNext),
            _ => None,
        }
    }
}

/* Tags (sealed) — values mirror `edge_abi::tag::*`. */

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Tag {
    None = edge_abi::tag::NONE,
    Bool = edge_abi::tag::BOOL,
    Int = edge_abi::tag::INT,
    Float = edge_abi::tag::FLOAT,
    // UTF-8 bytes: encoder builds a str, decoder returns its bytes.
    Bytes = edge_abi::tag::BYTES,
}

impl Tag {
    pub fn from_u32(t: u32) -> Option<Self> {
        match t {
            edge_abi::tag::NONE => Some(Self::None),
            edge_abi::tag::BOOL => Some(Self::Bool),
            edge_abi::tag::INT => Some(Self::Int),
            edge_abi::tag::FLOAT => Some(Self::Float),
            edge_abi::tag::BYTES => Some(Self::Bytes),
            _ => None,
        }
    }
}

/* Error kinds (sealed) — values mirror `edge_abi::error_kind::*`. */

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ErrorKind {
    Type = edge_abi::error_kind::TYPE,
    Value = edge_abi::error_kind::VALUE,
    Runtime = edge_abi::error_kind::RUNTIME,
    Attribute = edge_abi::error_kind::ATTRIBUTE,
    Index = edge_abi::error_kind::INDEX,
    Key = edge_abi::error_kind::KEY,
    Custom = edge_abi::error_kind::CUSTOM,
}

impl ErrorKind {
    pub fn from_u32(k: u32) -> Option<Self> {
        match k {
            edge_abi::error_kind::TYPE => Some(Self::Type),
            edge_abi::error_kind::VALUE => Some(Self::Value),
            edge_abi::error_kind::RUNTIME => Some(Self::Runtime),
            edge_abi::error_kind::ATTRIBUTE => Some(Self::Attribute),
            edge_abi::error_kind::INDEX => Some(Self::Index),
            edge_abi::error_kind::KEY => Some(Self::Key),
            edge_abi::error_kind::CUSTOM => Some(Self::Custom),
            _ => None,
        }
    }
}

/* Handle table */

// Handle slot; rc=0 = free. Exposed handle = index+1 (0 reserved as invalid).
struct HandleSlot {
    // Raw Val bits, opaque to the ABI; encode/decode go through classify_*.
    val: u64,
    rc: u32,
}

// Refcounted handle -> Val-bits map; cleared between script runs.
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

// Single-slot error stash; populated by dispatch failures / edge_throw, drained by edge_take_error.
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

// edge_encode outcome: Direct (Val bits), AllocStr (host alloc), or Invalid.
pub enum EncodeRequest<'a> {
    Direct(u64),
    AllocStr(&'a str),
    Invalid,
}

// Maps (tag, bytes) to EncodeRequest using the sealed `nan_box` layout.
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

// edge_decode outcome: Primitive (ready bytes), Heap (host materializes), or Invalid.
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

// Classifies Val bits; Heap routes the host to HeapPool.
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
