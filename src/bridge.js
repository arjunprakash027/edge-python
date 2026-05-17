/*
DOM capability bridge. Embedded into edge_python_dom.wasm at compile time
and extracted by the loader (runtime/src/edge.js) at module-registration time.

Pure JS factory: `(rt) => handlerMap`. The runtime `rt` exposes compiler_lib's
bootstrap codec (decodeStr / encodeInt / etc.), so handlers never have to
touch raw WASM memory or NaN-boxing themselves.

Each handler receives an array of u32 handles (one per Python arg) and returns
a single u32 handle as its result. Throw to surface a Python exception — the
loader converts it into a stashed RuntimeError.
*/

(rt) => {
    const nodes = [];
    const HANDLE_NONE = -1;

    const alloc = (node) => {
        if (node === null || node === undefined) return HANDLE_NONE;
        nodes.push(node);
        return nodes.length - 1;
    };

    const node = (h) => {
        if (h < 0 || h >= nodes.length || nodes[h] === null) {
            throw new Error("invalid DOM node handle: " + h);
        }
        return nodes[h];
    };

    return {
        query: (a) => rt.encodeInt(alloc(document.querySelector(rt.decodeStr(a[0])))),

        body: () => rt.encodeInt(alloc(document.body)),

        create_element: (a) => rt.encodeInt(alloc(document.createElement(rt.decodeStr(a[0])))),

        append_child: (a) => {
            node(rt.decodeInt(a[0])).appendChild(node(rt.decodeInt(a[1])));
            return rt.encodeNone();
        },

        remove: (a) => {
            const h = rt.decodeInt(a[0]);
            node(h).remove();
            nodes[h] = null;
            return rt.encodeNone();
        },

        get_text: (a) => rt.encodeStr(node(rt.decodeInt(a[0])).textContent || ""),

        set_text: (a) => {
            node(rt.decodeInt(a[0])).textContent = rt.decodeStr(a[1]);
            return rt.encodeNone();
        },

        get_attribute: (a) => {
            const v = node(rt.decodeInt(a[0])).getAttribute(rt.decodeStr(a[1]));
            return v === null ? rt.encodeNone() : rt.encodeStr(v);
        },

        set_attribute: (a) => {
            node(rt.decodeInt(a[0])).setAttribute(rt.decodeStr(a[1]), rt.decodeStr(a[2]));
            return rt.encodeNone();
        },

        add_class: (a) => {
            node(rt.decodeInt(a[0])).classList.add(rt.decodeStr(a[1]));
            return rt.encodeNone();
        },

        remove_class: (a) => {
            node(rt.decodeInt(a[0])).classList.remove(rt.decodeStr(a[1]));
            return rt.encodeNone();
        },
    };
}
