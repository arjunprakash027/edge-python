/* Sealed source-of-truth for the Edge Python wasm-abi v1 wire format.
   Both the compiler (host) and edge-pdk (guest) consume from here so the
   op codes, value tags, NaN-boxing layout, error kinds, and version
   number live in one place. Any change touches one site instead of
   three and forces a deliberate ABI version bump.

   no_std, zero deps. Re-exported by both `compiler::abi` and
   `edge_pdk::{op, tag, EDGE_ABI_VERSION}` so existing imports keep
   compiling. */

#![no_std]

/* Wire-format version. Bump on any breaking change to op codes, value
   tags, codec layout, or error kinds. Plugins export `__edge_abi_version`
   returning this constant; hosts MUST refuse instances with an
   unrecognised version. */
pub const EDGE_ABI_VERSION: u32 = 1;

/* NaN-boxing layout used to pack Val into 64 bits. */
pub mod nan_box {
    pub const QNAN: u64        = 0x7FFC_0000_0000_0000;
    pub const SIGN: u64        = 0x8000_0000_0000_0000;
    pub const TAG_UNDEF: u64   = QNAN;
    pub const TAG_NONE: u64    = QNAN | 1;
    pub const TAG_TRUE: u64    = QNAN | 2;
    pub const TAG_FALSE: u64   = QNAN | 3;
    pub const TAG_INT: u64     = QNAN | SIGN;
    pub const TAG_HEAP: u64    = QNAN | 4;
    /* 47-bit signed integer payload (two's-complement, sign bit at bit 47). */
    pub const INT_PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
}

/* Op codes for the universal `edge_op` dispatch primitive. */
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

/* Tags used by `edge_encode` / `edge_decode` for primitive transit. */
pub mod tag {
    pub const NONE: u32  = 0;
    pub const BOOL: u32  = 1;
    pub const INT: u32   = 2;
    pub const FLOAT: u32 = 3;
    /* UTF-8 bytes: encoder builds a str, decoder returns the str's bytes. */
    pub const BYTES: u32 = 4;
}

/* edge_decode sentinel: invalid handle or non-primitive — the caller
   should reach the value via `edge_op` instead. */
pub const TAG_INVALID: u32 = u32::MAX;

/* Error kinds drained by `edge_take_error` and produced by `edge_throw`. */
pub mod error_kind {
    pub const TYPE: u32      = 0;
    pub const VALUE: u32     = 1;
    pub const RUNTIME: u32   = 2;
    pub const ATTRIBUTE: u32 = 3;
    pub const INDEX: u32     = 4;
    pub const KEY: u32       = 5;
    pub const CUSTOM: u32    = 6;
}
