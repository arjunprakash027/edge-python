/*  Native module ABI plumbing: `nativeTable` holds guest exports by u32 id; `host_call_native` dispatches via the universal handle ABI. */
export const nativeTable = [];

/* Build the guest module's env imports: translate between guest linear memory and compiler memory + handle table. */
export function makeGuestEnv(compilerExports) {
    /* View factories: `wasm_alloc` and `host_edge_*` may grow memory and detach cached ArrayBuffer views; fresh per use. */
    const compMem = () => new Uint8Array(compilerExports.memory.buffer);
    const compView = () => new DataView(compilerExports.memory.buffer);

    return (guestExports) => {
        const gMem = () => new Uint8Array(guestExports.memory.buffer);
        const gView = () => new DataView(guestExports.memory.buffer);

        // Alloc compiler scratch and copy `len` bytes from guest memory; stages name/buf/msg before a `host_edge_*` call.
        const stage = (ptr, len) => {
            const c = compilerExports.wasm_alloc(Math.max(1, len));
            if (len) compMem().set(gMem().subarray(ptr, ptr + len), c);
            return c;
        };

        return {
            edge_op: (op, recv, name_ptr, name_len, argv_ptr, argc, out) => {
                const cName = stage(name_ptr, name_len);
                const cArgv = compilerExports.wasm_alloc(Math.max(4, argc * 4));
                const cOut  = compilerExports.wasm_alloc(4);
                for (let i = 0; i < argc; i++) {
                    compView().setUint32(cArgv + i * 4, gView().getUint32(argv_ptr + i * 4, true), true);
                }
                const ret = compilerExports.host_edge_op(op, recv, cName, name_len, cArgv, argc, cOut);
                if (ret === 0 && out) gView().setUint32(out, compView().getUint32(cOut, true), true);
                return ret;
            },

            edge_encode: (tag, ptr, len) =>
                compilerExports.host_edge_encode(tag, stage(ptr, len), len),

            edge_decode: (h, out_tag, dst, dst_max) => {
                const cTag = compilerExports.wasm_alloc(4);
                const cBuf = compilerExports.wasm_alloc(Math.max(1, dst_max));
                const ret = compilerExports.host_edge_decode(h, cTag, cBuf, dst_max);
                // Always write the tag back (caller may inspect it on err too).
                gView().setUint32(out_tag, compView().getUint32(cTag, true), true);
                if (ret > 0) gMem().set(compMem().subarray(cBuf, cBuf + ret), dst);
                return ret;
            },

            edge_release: (h) => compilerExports.host_edge_release(h),

            edge_throw: (kind, msg_ptr, msg_len) => {
                compilerExports.host_edge_throw(kind, stage(msg_ptr, msg_len), msg_len);
            },

            edge_take_error: (out_kind, dst, dst_max) => {
                const cKind = compilerExports.wasm_alloc(4);
                const cBuf  = compilerExports.wasm_alloc(Math.max(1, dst_max));
                const ret = compilerExports.host_edge_take_error(cKind, cBuf, dst_max);
                if (ret >= 0) {
                    gView().setUint32(out_kind, compView().getUint32(cKind, true), true);
                    if (ret > 0) gMem().set(compMem().subarray(cBuf, cBuf + ret), dst);
                }
                return ret;
            },
        };
    };
}

/* Instantiate guest .wasm against universal ABI. Returns callable export names; guest must export `__edge_alloc(size) -> ptr`. */
export async function instantiateNativeModule(spec, bytes, compilerExports) {
    const envFactory = makeGuestEnv(compilerExports);
    let guest;
    const env = envFactory({ get memory() { return guest.exports.memory; } });
    const { instance } = await WebAssembly.instantiate(bytes, { env });
    guest = instance;

    if (typeof instance.exports.__edge_alloc !== 'function') {
        throw new Error(
            `native module '${spec}' must export '__edge_alloc(size: u32) -> *mut u8';` +
            ` see /reference/wasm-abi for the contract`
        );
    }

    // Discover callable exports. Skip the ABI plumbing (memory) and toolchain post-link helpers (anything prefixed `__`, including __edge_alloc).
    const names = [];
    const fns = [];
    for (const [k, v] of Object.entries(instance.exports)) {
        if (k === 'memory' || k.startsWith('__') || typeof v !== 'function') continue;
        names.push(k);
        // Annotate the function with its guest's allocator so `host_call_native` stages argv without re-resolving the instance.
        v.__edge_alloc = instance.exports.__edge_alloc;
        v.__edge_memory = instance.exports.memory;
        fns.push(v);
    }
    return { names, fns };
}
