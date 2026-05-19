    const animationsApi = {
        // keyframes/options are JSON. `finish_msg` (optional) is dispatched via receive() when the animation finishes.
        animate: (a) => {
            const target = node(rt.decodeInt(a[0]));
            const keyframes = JSON.parse(rt.decodeStr(a[1]));
            const opts = a[2] !== undefined ? JSON.parse(rt.decodeStr(a[2]) || "{}") : {};
            const finish_msg = a[3] !== undefined ? rt.decodeStr(a[3]) : null;
            // Infinity doesn't survive JSON; accept "Infinity" as a string escape.
            if (opts.iterations === "Infinity") opts.iterations = Infinity;
            const anim = target.animate(keyframes, opts);
            if (finish_msg) {
                anim.finished.then(() => {
                    window.dispatchEvent(new CustomEvent("edge-python-event", { detail: finish_msg }));
                }).catch(() => {});
            }
            animations.push(anim);
            return rt.encodeInt(animations.length - 1);
        },

        animation_play: (a) => { const h = rt.decodeInt(a[0]); if (animations[h]) animations[h].play(); return rt.encodeNone(); },
        animation_pause: (a) => { const h = rt.decodeInt(a[0]); if (animations[h]) animations[h].pause(); return rt.encodeNone(); },
        animation_cancel: (a) => { const h = rt.decodeInt(a[0]); if (animations[h]) animations[h].cancel(); return rt.encodeNone(); },
        animation_finish: (a) => { const h = rt.decodeInt(a[0]); if (animations[h]) animations[h].finish(); return rt.encodeNone(); },
        animation_reverse: (a) => { const h = rt.decodeInt(a[0]); if (animations[h]) animations[h].reverse(); return rt.encodeNone(); },
    };
