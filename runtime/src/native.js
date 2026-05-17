/*
Native module loading + dispatch. `nativeTable` indexed by `baseId` from `register_native_module`; entries are wasmpdk fns or capability JS handlers, dispatched by `env.js`'s `host_call_native`.
*/

export const nativeTable = [];

export function resetNativeTable() {
    nativeTable.length = 0;
}

/* Build the 6 `env.edge_*` imports for wasm-pdk plugins; bridges guest ↔ compiler memory. */
export function makeGuestEnv(compilerExports) {
    const compMem = () => new Uint8Array(compilerExports.memory.buffer);
    const compView = () => new DataView(compilerExports.memory.buffer);

    return (guestExports) => {
        const gMem = () => new Uint8Array(guestExports.memory.buffer);
        const gView = () => new DataView(guestExports.memory.buffer);

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

/* Built-in Path A fallback: instantiate guest, walk exports, annotate each fn with its guest's `__edge_alloc` + `__edge_memory`. */
async function builtinWasmPdkLoader(module, ctx) {
    const envFactory = makeGuestEnv(ctx.compilerExports);
    let guest;
    const env = envFactory({ get memory() { return guest.exports.memory; } });
    // WebAssembly.instantiate(Module, ...) returns the Instance directly, not {module, instance}.
    const instance = await WebAssembly.instantiate(module, { env });
    guest = instance;

    if (typeof instance.exports.__edge_alloc !== 'function') {
        throw new Error(
            `native module missing '__edge_alloc(size: u32) -> *mut u8';` +
            ` see /reference/wasm-abi for the contract`
        );
    }

    const names = [];
    const fns = [];
    for (const [k, v] of Object.entries(instance.exports)) {
        if (k === 'memory' || k.startsWith('__') || typeof v !== 'function') continue;
        names.push(k);
        v.__edge_alloc = instance.exports.__edge_alloc;
        v.__edge_memory = instance.exports.memory;
        v.__edge_kind = 'wasmpdk';
        fns.push(v);
    }
    return { kind: 'wasmpdk', names, fns };
}

/* Try custom loaders first; built-in Path A is the implicit fallback. */
export async function loadNativeModule(spec, bytes, ctx) {
    const module = await WebAssembly.compile(bytes);

    for (const loader of ctx.loaders) {
        if (loader.match(module)) {
            const result = await loader.load(module, ctx);
            // Tag each fn with its dispatch kind so host_call_native picks the right path.
            for (const fn of result.fns) fn.__edge_kind = result.kind;
            return result;
        }
    }

    return await builtinWasmPdkLoader(module, ctx);
}
