    const tree = {
        query: (a) => rt.encodeInt(alloc(document.querySelector(rt.decodeStr(a[0])))),
        query_all: (a) => rt.encodeStr(allocList(document.querySelectorAll(rt.decodeStr(a[0])))),
        closest: (a) => rt.encodeInt(alloc(node(rt.decodeInt(a[0])).closest(rt.decodeStr(a[1])))),
        matches: (a) => rt.encodeBool(node(rt.decodeInt(a[0])).matches(rt.decodeStr(a[1]))),
        body: () => rt.encodeInt(alloc(document.body)),
        active_element: () => rt.encodeInt(alloc(document.activeElement)),

        parent: (a) => rt.encodeInt(alloc(node(rt.decodeInt(a[0])).parentElement)),
        children: (a) => rt.encodeStr(allocList(node(rt.decodeInt(a[0])).children)),
        first_child: (a) => rt.encodeInt(alloc(node(rt.decodeInt(a[0])).firstElementChild)),
        last_child: (a) => rt.encodeInt(alloc(node(rt.decodeInt(a[0])).lastElementChild)),
        next_sibling: (a) => rt.encodeInt(alloc(node(rt.decodeInt(a[0])).nextElementSibling)),
        prev_sibling: (a) => rt.encodeInt(alloc(node(rt.decodeInt(a[0])).previousElementSibling)),
        tag_name: (a) => rt.encodeStr((node(rt.decodeInt(a[0])).tagName || "").toLowerCase()),

        create_element: (a) => rt.encodeInt(alloc(document.createElement(rt.decodeStr(a[0])))),

        // SVG and MathML need a namespace; `createElement("svg")` returns an HTMLUnknownElement that doesn't render.
        create_element_ns: (a) => rt.encodeInt(alloc(
            document.createElementNS(rt.decodeStr(a[0]), rt.decodeStr(a[1]))
        )),

        append_child: (a) => {
            node(rt.decodeInt(a[0])).appendChild(node(rt.decodeInt(a[1])));
            return rt.encodeNone();
        },

        // Sibling positioning without needing the parent handle.
        insert_before: (a) => {
            const newNode = node(rt.decodeInt(a[0]));
            const refNode = node(rt.decodeInt(a[1]));
            refNode.parentNode.insertBefore(newNode, refNode);
            return rt.encodeNone();
        },

        // Nulls the slot rather than splicing — splicing would shift every later handle.
        remove: (a) => {
            const h = rt.decodeInt(a[0]);
            node(h).remove();
            nodes[h] = null;
            return rt.encodeNone();
        },

        // Variadic: first arg is the parent, rest are child handles. Pass only the parent to clear.
        replace_children: (a) => {
            const parent = node(rt.decodeInt(a[0]));
            const kids = new Array(a.length - 1);
            for (let i = 1; i < a.length; i++) kids[i - 1] = node(rt.decodeInt(a[i]));
            parent.replaceChildren(...kids);
            return rt.encodeNone();
        },

        // `deep` defaults to true (descendants included).
        clone_node: (a) => {
            const deep = a[1] !== undefined ? rt.decodeBool(a[1]) : true;
            return rt.encodeInt(alloc(node(rt.decodeInt(a[0])).cloneNode(deep)));
        },

        get_text: (a) => rt.encodeStr(node(rt.decodeInt(a[0])).textContent || ""),
        set_text: (a) => {
            node(rt.decodeInt(a[0])).textContent = rt.decodeStr(a[1]);
            return rt.encodeNone();
        },

        get_html: (a) => rt.encodeStr(node(rt.decodeInt(a[0])).innerHTML || ""),
        set_html: (a) => {
            node(rt.decodeInt(a[0])).innerHTML = rt.decodeStr(a[1]);
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
        remove_attribute: (a) => {
            node(rt.decodeInt(a[0])).removeAttribute(rt.decodeStr(a[1]));
            return rt.encodeNone();
        },

        add_class: (a) => { node(rt.decodeInt(a[0])).classList.add(rt.decodeStr(a[1])); return rt.encodeNone(); },
        remove_class: (a) => { node(rt.decodeInt(a[0])).classList.remove(rt.decodeStr(a[1])); return rt.encodeNone(); },
        toggle_class: (a) => rt.encodeBool(node(rt.decodeInt(a[0])).classList.toggle(rt.decodeStr(a[1]))),
        has_class: (a) => rt.encodeBool(node(rt.decodeInt(a[0])).classList.contains(rt.decodeStr(a[1]))),

        get_data: (a) => {
            const v = node(rt.decodeInt(a[0])).dataset[rt.decodeStr(a[1])];
            return v === undefined ? rt.encodeNone() : rt.encodeStr(v);
        },
        set_data: (a) => {
            node(rt.decodeInt(a[0])).dataset[rt.decodeStr(a[1])] = rt.decodeStr(a[2]);
            return rt.encodeNone();
        },
    };
