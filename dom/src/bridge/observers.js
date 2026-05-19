    const observers = {
        // Options: {root_handle?, rootMargin?, threshold?}. Event detail: {msg, intersecting, ratio, x, y, w, h}.
        observe_intersection: (a) => {
            const target = node(rt.decodeInt(a[0]));
            const msg = rt.decodeStr(a[1]);
            const opts = a[2] !== undefined ? JSON.parse(rt.decodeStr(a[2]) || "{}") : {};
            if (opts.root_handle !== undefined) {
                opts.root = node(opts.root_handle);
                delete opts.root_handle;
            }
            const observer = new IntersectionObserver((entries) => {
                for (const e of entries) {
                    const r = e.boundingClientRect;
                    window.dispatchEvent(new CustomEvent("edge-python-event", {
                        detail: JSON.stringify({
                            msg,
                            intersecting: e.isIntersecting,
                            ratio: e.intersectionRatio,
                            x: r.x, y: r.y, w: r.width, h: r.height,
                        })
                    }));
                }
            }, opts);
            observer.observe(target);
            intersectionObservers.push(observer);
            return rt.encodeInt(intersectionObservers.length - 1);
        },

        unobserve_intersection: (a) => {
            const h = rt.decodeInt(a[0]);
            const o = intersectionObservers[h];
            if (!o) return rt.encodeNone();
            o.disconnect();
            intersectionObservers[h] = null;
            return rt.encodeNone();
        },

        // Fires when target's box changes (any layout reflow), not just on window resize. Event detail: {msg, w, h, x, y}.
        observe_resize: (a) => {
            const target = node(rt.decodeInt(a[0]));
            const msg = rt.decodeStr(a[1]);
            const observer = new ResizeObserver((entries) => {
                for (const e of entries) {
                    const r = e.contentRect;
                    window.dispatchEvent(new CustomEvent("edge-python-event", {
                        detail: JSON.stringify({ msg, w: r.width, h: r.height, x: r.x, y: r.y })
                    }));
                }
            });
            observer.observe(target);
            resizeObservers.push(observer);
            return rt.encodeInt(resizeObservers.length - 1);
        },

        unobserve_resize: (a) => {
            const h = rt.decodeInt(a[0]);
            const o = resizeObservers[h];
            if (!o) return rt.encodeNone();
            o.disconnect();
            resizeObservers[h] = null;
            return rt.encodeNone();
        },

        // Options follow MutationObserverInit. Added/removed report counts only — re-query for the actual new nodes.
        observe_mutations: (a) => {
            const target = node(rt.decodeInt(a[0]));
            const msg = rt.decodeStr(a[1]);
            const opts = a[2] !== undefined ? JSON.parse(rt.decodeStr(a[2]) || "{}") : {};
            // Spec requires at least one of childList/attributes/characterData — default to the most common.
            if (!opts.childList && !opts.attributes && !opts.characterData) {
                opts.childList = true;
            }
            const observer = new MutationObserver((mutations) => {
                for (const m of mutations) {
                    window.dispatchEvent(new CustomEvent("edge-python-event", {
                        detail: JSON.stringify({
                            msg,
                            type: m.type,
                            target_tag: m.target && m.target.tagName ? m.target.tagName.toLowerCase() : undefined,
                            target_id: m.target && m.target.id ? m.target.id : undefined,
                            attribute_name: m.attributeName || undefined,
                            attribute_old: m.oldValue || undefined,
                            added: m.addedNodes.length,
                            removed: m.removedNodes.length,
                        })
                    }));
                }
            });
            observer.observe(target, opts);
            mutationObservers.push(observer);
            return rt.encodeInt(mutationObservers.length - 1);
        },

        unobserve_mutations: (a) => {
            const h = rt.decodeInt(a[0]);
            const o = mutationObservers[h];
            if (!o) return rt.encodeNone();
            o.disconnect();
            mutationObservers[h] = null;
            return rt.encodeNone();
        },
    };
