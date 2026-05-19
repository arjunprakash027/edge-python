    const style = {
        set_style: (a) => {
            node(rt.decodeInt(a[0])).style[rt.decodeStr(a[1])] = rt.decodeStr(a[2]);
            return rt.encodeNone();
        },
        get_style: (a) => rt.encodeStr(node(rt.decodeInt(a[0])).style[rt.decodeStr(a[1])] || ""),
        get_computed_style: (a) => rt.encodeStr(
            getComputedStyle(node(rt.decodeInt(a[0])))[rt.decodeStr(a[1])] || ""
        ),

        // Returns JSON {x, y, w, h, top, right, bottom, left}.
        rect: (a) => {
            const r = node(rt.decodeInt(a[0])).getBoundingClientRect();
            return rt.encodeStr(JSON.stringify({
                x: r.x, y: r.y, w: r.width, h: r.height,
                top: r.top, right: r.right, bottom: r.bottom, left: r.left,
            }));
        },

        offset_width: (a) => rt.encodeInt(node(rt.decodeInt(a[0])).offsetWidth),
        offset_height: (a) => rt.encodeInt(node(rt.decodeInt(a[0])).offsetHeight),
        client_width: (a) => rt.encodeInt(node(rt.decodeInt(a[0])).clientWidth),
        client_height: (a) => rt.encodeInt(node(rt.decodeInt(a[0])).clientHeight),

        scroll_top: (a) => rt.encodeInt(node(rt.decodeInt(a[0])).scrollTop),
        set_scroll_top: (a) => {
            node(rt.decodeInt(a[0])).scrollTop = rt.decodeInt(a[1]);
            return rt.encodeNone();
        },
        scroll_into_view: (a) => {
            node(rt.decodeInt(a[0])).scrollIntoView();
            return rt.encodeNone();
        },

        focus: (a) => { node(rt.decodeInt(a[0])).focus(); return rt.encodeNone(); },
        blur: (a) => { node(rt.decodeInt(a[0])).blur(); return rt.encodeNone(); },
    };
