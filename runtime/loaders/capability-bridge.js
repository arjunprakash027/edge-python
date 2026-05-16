/*
Path D loader: extracts embedded JS bridge from capability `.wasm`, evals as `(rt) => handlerMap`, registers as JS handlers. Opt-in; needs CSP `'unsafe-eval'`.
*/

const TD = new TextDecoder();

export default {
    match(module) {
        const names = WebAssembly.Module.exports(module).map(e => e.name);
        return names.includes('edge_capability_bridge_ptr')
            && names.includes('edge_capability_bridge_len');
    },

    async load(module, ctx) {
        const { instance } = await WebAssembly.instantiate(module, { env: {} });

        const ptr = instance.exports.edge_capability_bridge_ptr();
        const len = instance.exports.edge_capability_bridge_len();
        const src = TD.decode(new Uint8Array(instance.exports.memory.buffer, ptr, len));

        const factory = new Function(`return (${src});`)();
        const handlers = factory(ctx.rt);

        return {
            kind: 'capability',
            names: Object.keys(handlers),
            fns: Object.values(handlers),
        };
    },
};
