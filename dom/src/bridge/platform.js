    const platform = {
        // Native modal with backdrop + focus trap. Bind "close" event to detect dismissal.
        show_modal: (a) => { node(rt.decodeInt(a[0])).showModal(); return rt.encodeNone(); },
        dialog_close: (a) => {
            const n = node(rt.decodeInt(a[0]));
            if (a[1] !== undefined) n.close(rt.decodeStr(a[1])); else n.close();
            return rt.encodeNone();
        },

        // May reject without a user gesture; we swallow. Bind "fullscreenchange" on documentElement for state.
        request_fullscreen: (a) => {
            const p = node(rt.decodeInt(a[0])).requestFullscreen();
            if (p && p.catch) p.catch(() => {});
            return rt.encodeNone();
        },
        exit_fullscreen: () => {
            const p = document.exitFullscreen();
            if (p && p.catch) p.catch(() => {});
            return rt.encodeNone();
        },
        fullscreen_element: () => rt.encodeInt(alloc(document.fullscreenElement)),

        // While locked, clientX/Y freezes but movementX/Y keeps firing (see events.js payload).
        request_pointer_lock: (a) => {
            const p = node(rt.decodeInt(a[0])).requestPointerLock();
            if (p && p.catch) p.catch(() => {});
            return rt.encodeNone();
        },
        exit_pointer_lock: () => { document.exitPointerLock(); return rt.encodeNone(); },

        // User-space units (different from `rect()` which returns viewport pixels).
        bbox: (a) => {
            const r = node(rt.decodeInt(a[0])).getBBox();
            return rt.encodeStr(JSON.stringify({ x: r.x, y: r.y, w: r.width, h: r.height }));
        },

        // Total length of an SVGPathElement — needed for stroke-dasharray "drawing" animations.
        path_length: (a) => rt.encodeFloat(node(rt.decodeInt(a[0])).getTotalLength()),

        // {x, y} at distance `dist` along the path — for animating an object along a curve.
        point_at_length: (a) => {
            const p = node(rt.decodeInt(a[0])).getPointAtLength(rt.decodeFloat(a[1]));
            return rt.encodeStr(JSON.stringify({ x: p.x, y: p.y }));
        },
    };
