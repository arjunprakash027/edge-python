/*
Handle-codec helpers wrapping `host_edge_decode` / `host_edge_encode`; passed to capability loaders so handlers skip NaN-boxing.
*/

const TD = new TextDecoder();
const TE = new TextEncoder();

// wasm-abi/src/lib.rs `pub mod tag`.
const TAG_NONE = 0;
const TAG_BOOL = 1;
const TAG_INT = 2;
const TAG_FLOAT = 3;
const TAG_BYTES = 4;

export function makeRt(getExports) {
    return {
        decodeStr: (h) => decodeStr(getExports(), h),
        decodeInt: (h) => decodeInt(getExports(), h),
        decodeBool: (h) => decodeBool(getExports(), h),
        decodeFloat: (h) => decodeFloat(getExports(), h),
        encodeStr: (s) => encodeStr(getExports(), s),
        encodeInt: (n) => encodeInt(getExports(), n),
        encodeBool: (b) => encodeBool(getExports(), b),
        encodeFloat: (f) => encodeFloat(getExports(), f),
        encodeNone: () => getExports().host_edge_encode(TAG_NONE, 0, 0),
    };
}

function decodeBytes(exps, handle, expectedTag) {
    const tagPtr = exps.wasm_alloc(4);
    let cap = 256;
    let dstPtr = exps.wasm_alloc(cap);
    let n = exps.host_edge_decode(handle, tagPtr, dstPtr, cap);
    if (n < 0) {
        const needed = -n;
        exps.wasm_free(dstPtr, cap);
        cap = needed;
        dstPtr = exps.wasm_alloc(cap);
        n = exps.host_edge_decode(handle, tagPtr, dstPtr, cap);
    }
    const tag = new DataView(exps.memory.buffer).getUint32(tagPtr, true);
    exps.wasm_free(tagPtr, 4);
    if (tag !== expectedTag) {
        exps.wasm_free(dstPtr, cap);
        throw new Error(`expected tag ${expectedTag}, got ${tag}`);
    }
    const out = new Uint8Array(exps.memory.buffer, dstPtr, n).slice();
    exps.wasm_free(dstPtr, cap);
    return out;
}

const decodeStr = (exps, h) => TD.decode(decodeBytes(exps, h, TAG_BYTES));
const decodeInt = (exps, h) => {
    const b = decodeBytes(exps, h, TAG_INT);
    return Number(new DataView(b.buffer, b.byteOffset, 8).getBigInt64(0, true));
};
const decodeBool = (exps, h) => decodeBytes(exps, h, TAG_BOOL)[0] !== 0;
const decodeFloat = (exps, h) => {
    const b = decodeBytes(exps, h, TAG_FLOAT);
    return new DataView(b.buffer, b.byteOffset, 8).getFloat64(0, true);
};

function encodeStr(exps, s) {
    const bytes = TE.encode(s);
    const ptr = exps.wasm_alloc(bytes.length);
    new Uint8Array(exps.memory.buffer, ptr, bytes.length).set(bytes);
    const h = exps.host_edge_encode(TAG_BYTES, ptr, bytes.length);
    exps.wasm_free(ptr, bytes.length);
    return h;
}
function encodeInt(exps, n) {
    const buf = exps.wasm_alloc(8);
    new DataView(exps.memory.buffer).setBigInt64(buf, BigInt(n), true);
    const h = exps.host_edge_encode(TAG_INT, buf, 8);
    exps.wasm_free(buf, 8);
    return h;
}
function encodeBool(exps, b) {
    const buf = exps.wasm_alloc(1);
    new Uint8Array(exps.memory.buffer, buf, 1)[0] = b ? 1 : 0;
    const h = exps.host_edge_encode(TAG_BOOL, buf, 1);
    exps.wasm_free(buf, 1);
    return h;
}
function encodeFloat(exps, f) {
    const buf = exps.wasm_alloc(8);
    new DataView(exps.memory.buffer).setFloat64(buf, f, true);
    const h = exps.host_edge_encode(TAG_FLOAT, buf, 8);
    exps.wasm_free(buf, 8);
    return h;
}
