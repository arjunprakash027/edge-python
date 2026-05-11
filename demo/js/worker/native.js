/* Native module ABI plumbing.

   `nativeTable`: JS owns the table; compiler.wasm refers to entries by their
   u32 id (assigned at register_native_module time). Each entry holds a guest
   export function. host_call_native looks up by id and routes the call
   through the universal handle ABI. */
export const nativeTable = [];

/* Build the env imports a guest module declares. They translate the guest's
   view (its own pointers in its own linear memory) into the compiler's view
   (compiler memory + handle table) and back. */
export function makeGuestEnv(compilerExports) {
    const compMem = () => new Uint8Array(compilerExports.memory.buffer);
    const compView = () => new DataView(compilerExports.memory.buffer);

    return (guestExports) => ({
        edge_op: (op, recv, name_ptr, name_len, argv_ptr, argc, out) => {
            const guestMem = new Uint8Array(guestExports.memory.buffer);
            const guestView = new DataView(guestExports.memory.buffer);
            // Stage name and argv in compiler memory.
            const cName = compilerExports.wasm_alloc(Math.max(1, name_len));
            const cArgv = compilerExports.wasm_alloc(Math.max(4, argc * 4));
            const cOut  = compilerExports.wasm_alloc(4);
            if (name_len) compMem().set(guestMem.subarray(name_ptr, name_ptr + name_len), cName);
            for (let i = 0; i < argc; i++) {
                const h = guestView.getUint32(argv_ptr + i * 4, true);
                compView().setUint32(cArgv + i * 4, h, true);
            }
            const ret = compilerExports.host_edge_op(op, recv, cName, name_len, cArgv, argc, cOut);
            if (ret === 0 && out) {
                const h = compView().getUint32(cOut, true);
                guestView.setUint32(out, h, true);
            }
            return ret;
        },

        edge_encode: (tag, ptr, len) => {
            const guestMem = new Uint8Array(guestExports.memory.buffer);
            const cPtr = compilerExports.wasm_alloc(Math.max(1, len));
            if (len) compMem().set(guestMem.subarray(ptr, ptr + len), cPtr);
            return compilerExports.host_edge_encode(tag, cPtr, len);
        },

        edge_decode: (h, out_tag, dst, dst_max) => {
            const guestView = new DataView(guestExports.memory.buffer);
            const cTag = compilerExports.wasm_alloc(4);
            const cBuf = compilerExports.wasm_alloc(Math.max(1, dst_max));
            const ret = compilerExports.host_edge_decode(h, cTag, cBuf, dst_max);
            // Always write the tag back (caller may inspect it on err too).
            const tag = compView().getUint32(cTag, true);
            guestView.setUint32(out_tag, tag, true);
            if (ret > 0) {
                const guestMem = new Uint8Array(guestExports.memory.buffer);
                guestMem.set(compMem().subarray(cBuf, cBuf + ret), dst);
            }
            return ret;
        },

        edge_release: (h) => compilerExports.host_edge_release(h),

        edge_throw: (kind, msg_ptr, msg_len) => {
            const guestMem = new Uint8Array(guestExports.memory.buffer);
            const cMsg = compilerExports.wasm_alloc(Math.max(1, msg_len));
            if (msg_len) compMem().set(guestMem.subarray(msg_ptr, msg_ptr + msg_len), cMsg);
            compilerExports.host_edge_throw(kind, cMsg, msg_len);
        },

        edge_take_error: (out_kind, dst, dst_max) => {
            const guestView = new DataView(guestExports.memory.buffer);
            const cKind = compilerExports.wasm_alloc(4);
            const cBuf = compilerExports.wasm_alloc(Math.max(1, dst_max));
            const ret = compilerExports.host_edge_take_error(cKind, cBuf, dst_max);
            if (ret >= 0) {
                guestView.setUint32(out_kind, compView().getUint32(cKind, true), true);
                if (ret > 0) {
                    const guestMem = new Uint8Array(guestExports.memory.buffer);
                    guestMem.set(compMem().subarray(cBuf, cBuf + ret), dst);
                }
            }
            return ret;
        },
    });
}

/* Instantiate a guest .wasm module against the universal ABI. Returns
   the list of callable export names so the host can register them with
   compiler.wasm. The guest is required to export `__edge_alloc(size) ->
   ptr` for the host to stage argv arrays in guest memory. */
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

    // Discover callable exports. Skip the ABI plumbing (memory,
    // __edge_alloc) and toolchain post-link helpers (__data_end, etc.).
    const SKIP = new Set(['memory', '__edge_alloc']);
    const names = [];
    const fns = [];
    for (const k of Object.keys(instance.exports)) {
        if (SKIP.has(k) || k.startsWith('__')) continue;
        if (typeof instance.exports[k] !== 'function') continue;
        names.push(k);
        const fn = instance.exports[k];
        // Annotate the function with its guest's allocator so host_call_native
        // can stage argv without re-resolving the instance every call.
        fn.__edge_alloc = instance.exports.__edge_alloc;
        fn.__edge_memory = instance.exports.memory;
        fns.push(fn);
    }
    return { names, fns };
}
