/* Web Animations API; `finish_msg` arrives via `receive()` on completion. */

export default ({ node, animations }, { pushEvent }) => ({
    /* keyframes/options are JSON. `finish_msg` (optional) wakes `receive()` when the animation finishes. */
    animate: (h, keyframesJson, optionsJson, finish_msg) => {
        const target = node(h);
        const keyframes = JSON.parse(keyframesJson);
        const opts = optionsJson !== undefined ? JSON.parse(optionsJson || '{}') : {};
        /* Infinity doesn't survive JSON; accept "Infinity" as a string escape. */
        if (opts.iterations === 'Infinity') opts.iterations = Infinity;
        const anim = target.animate(keyframes, opts);
        if (finish_msg) {
            anim.finished.then(() => pushEvent(finish_msg)).catch(() => {});
        }
        animations.push(anim);
        return animations.length - 1;
    },

    animation_play: (h) => { if (animations[h]) animations[h].play(); },
    animation_pause: (h) => { if (animations[h]) animations[h].pause(); },
    animation_cancel: (h) => { if (animations[h]) animations[h].cancel(); },
    animation_finish: (h) => { if (animations[h]) animations[h].finish(); },
    animation_reverse: (h) => { if (animations[h]) animations[h].reverse(); },
});
