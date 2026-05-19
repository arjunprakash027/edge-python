    const events = {
        // Listener fires `CustomEvent("edge-python-event")` with a JSON detail; Python reads it via `receive()`.
        bind_event: (a) => {
            const target = node(rt.decodeInt(a[0]));
            const type = rt.decodeStr(a[1]);
            const msg = rt.decodeStr(a[2]);
            const opts = a[3] !== undefined ? JSON.parse(rt.decodeStr(a[3]) || "{}") : {};
            const listener = (e) => {
                if (opts.prevent_default) e.preventDefault();
                if (opts.stop_propagation) e.stopPropagation();

                // Drag/drop: dropped files become bridge handles; text/plain payload comes through `drop_text`.
                let drop_files, drop_text;
                if (e.dataTransfer) {
                    if (e.dataTransfer.files && e.dataTransfer.files.length) {
                        drop_files = [];
                        for (const f of e.dataTransfer.files) {
                            files.push(f);
                            drop_files.push(files.length - 1);
                        }
                    }
                    if (e.type === "drop") {
                        const t = e.dataTransfer.getData("text/plain");
                        if (t) drop_text = t;
                    }
                }

                // Clipboard text on copy/paste/cut; pasted file/image items become file handles.
                let clipboard_text, clipboard_files;
                if (e.clipboardData) {
                    const t = e.clipboardData.getData("text/plain");
                    if (t) clipboard_text = t;
                    if (e.type === "paste" && e.clipboardData.items) {
                        for (const item of e.clipboardData.items) {
                            if (item.kind === "file") {
                                const f = item.getAsFile();
                                if (f) {
                                    if (!clipboard_files) clipboard_files = [];
                                    files.push(f);
                                    clipboard_files.push(files.length - 1);
                                }
                            }
                        }
                    }
                }

                // Single touch is covered by clientX/Y; emit `touches` only for multi-finger.
                let touches;
                if (e.touches && e.touches.length > 1) {
                    touches = new Array(e.touches.length);
                    for (let i = 0; i < e.touches.length; i++) {
                        const t = e.touches[i];
                        touches[i] = { x: t.clientX, y: t.clientY, force: t.force };
                    }
                }

                const detail = JSON.stringify({
                    msg,
                    type: e.type,
                    target_id: e.target && e.target.id ? e.target.id : undefined,
                    target_tag: e.target && e.target.tagName ? e.target.tagName.toLowerCase() : undefined,
                    value: e.target && "value" in e.target ? e.target.value : undefined,
                    checked: e.target && "checked" in e.target ? e.target.checked : undefined,
                    key: e.key,
                    code: e.code,
                    button: e.button,
                    x: e.clientX,
                    y: e.clientY,
                    movement_x: e.movementX,
                    movement_y: e.movementY,
                    alt: e.altKey,
                    ctrl: e.ctrlKey,
                    shift: e.shiftKey,
                    meta: e.metaKey,
                    drop_files,
                    drop_text,
                    clipboard_text,
                    clipboard_files,
                    touches,
                });
                window.dispatchEvent(new CustomEvent("edge-python-event", { detail }));
            };
            const listenerOpts = { capture: !!opts.capture, passive: !!opts.passive, once: !!opts.once };
            target.addEventListener(type, listener, listenerOpts);
            bindings.push({ target, type, listener, capture: !!opts.capture });
            return rt.encodeInt(bindings.length - 1);
        },

        // Idempotent: double-unbind is a no-op.
        unbind_event: (a) => {
            const h = rt.decodeInt(a[0]);
            const b = bindings[h];
            if (!b) return rt.encodeNone();
            b.target.removeEventListener(b.type, b.listener, { capture: b.capture });
            bindings[h] = null;
            return rt.encodeNone();
        },

        dispatch_event: (a) => {
            const target = node(rt.decodeInt(a[0]));
            const type = rt.decodeStr(a[1]);
            const detail = a[2] !== undefined ? rt.decodeStr(a[2]) : "";
            target.dispatchEvent(new CustomEvent(type, { detail, bubbles: true, cancelable: true }));
            return rt.encodeNone();
        },

        // Synthetic native click — triggers default behaviors (file picker, link nav). CustomEvent("click") wouldn't.
        click: (a) => { node(rt.decodeInt(a[0])).click(); return rt.encodeNone(); },
    };
