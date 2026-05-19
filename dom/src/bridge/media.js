    const media = {
        // play() may reject under user-gesture policies; we swallow.
        media_play: (a) => {
            const p = node(rt.decodeInt(a[0])).play();
            if (p && p.catch) p.catch(() => {});
            return rt.encodeNone();
        },
        media_pause: (a) => { node(rt.decodeInt(a[0])).pause(); return rt.encodeNone(); },

        get_current_time: (a) => rt.encodeFloat(node(rt.decodeInt(a[0])).currentTime || 0),
        set_current_time: (a) => {
            node(rt.decodeInt(a[0])).currentTime = rt.decodeFloat(a[1]);
            return rt.encodeNone();
        },

        // NaN until metadata loads — we coerce to 0 so Python's float math stays clean.
        get_duration: (a) => {
            const d = node(rt.decodeInt(a[0])).duration;
            return rt.encodeFloat(isFinite(d) ? d : 0);
        },

        get_paused: (a) => rt.encodeBool(node(rt.decodeInt(a[0])).paused),

        set_volume: (a) => {
            node(rt.decodeInt(a[0])).volume = rt.decodeFloat(a[1]);
            return rt.encodeNone();
        },
        set_playback_rate: (a) => {
            node(rt.decodeInt(a[0])).playbackRate = rt.decodeFloat(a[1]);
            return rt.encodeNone();
        },
    };
