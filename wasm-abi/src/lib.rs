/* 
Edge Python wasm-abi wire format. Shared by compiler (host) and wasm-pdk (guest). no_std, zero deps.
*/

#![no_std]

/* Bump on any breaking change. Plugins export `__edge_abi_version`; hosts MUST refuse unrecognised versions. */
pub const EDGE_ABI_VERSION: u32 = 1;

/* NaN-boxing layout that packs Val into 64 bits. */
pub mod nan_box {
    pub const QNAN: u64 = 0x7FFC_0000_0000_0000;
    pub const SIGN: u64 = 0x8000_0000_0000_0000;
    pub const TAG_UNDEF: u64 = QNAN;
    pub const TAG_NONE: u64 = QNAN | 1;
    pub const TAG_TRUE: u64 = QNAN | 2;
    pub const TAG_FALSE: u64 = QNAN | 3;
    pub const TAG_INT: u64 = QNAN | SIGN;
    pub const TAG_HEAP: u64 = QNAN | 4;
    /* 47-bit signed payload, sign bit at bit 47. */
    pub const INT_PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
}

/* Op codes for the `edge_op` dispatch primitive. */
pub mod op {
    pub const CALL: u32 = 0;
    pub const GET_ATTR: u32 = 1;
    pub const SET_ATTR: u32 = 2;
    pub const GET_ITEM: u32 = 3;
    pub const SET_ITEM: u32 = 4;
    pub const LEN: u32 = 5;
    pub const ITER: u32 = 6;
    pub const ITER_NEXT: u32 = 7;
    pub const NEW_DICT: u32 = 8;
    pub const NEW_LIST: u32 = 9;
    pub const TYPE_OF: u32 = 10;
    // Construct composites from argv items in one call.
    pub const NEW_TUPLE: u32 = 11;
    pub const NEW_SET: u32 = 12;
    pub const NEW_FROZENSET: u32 = 13;
}

/* Tags for `edge_encode` / `edge_decode` primitive transit. */
pub mod tag {
    pub const NONE: u32 = 0;
    pub const BOOL: u32 = 1;
    pub const INT: u32 = 2;
    pub const FLOAT: u32 = 3;
    /* UTF-8 bytes: encoder builds a str, decoder returns its bytes. */
    pub const BYTES: u32 = 4;
    // Opaque bytes: skips UTF-8 validation, maps to Python `bytes`.
    pub const RAW: u32 = 5;
}

/* Sentinel from `edge_decode` for invalid handles or non-primitives, callers should reach the value via `edge_op` instead. */
pub const TAG_INVALID: u32 = u32::MAX;

/* Error kinds for `edge_take_error` and `edge_throw`. */
pub mod error_kind {
    pub const TYPE: u32 = 0;
    pub const VALUE: u32 = 1;
    pub const RUNTIME: u32 = 2;
    pub const ATTRIBUTE: u32 = 3;
    pub const INDEX: u32 = 4;
    pub const KEY: u32 = 5;
    pub const CUSTOM: u32 = 6;
}
