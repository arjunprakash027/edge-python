    const forms = {
        // `value`/`checked` are live state; `get_attribute("value")` reads the initial-value attribute instead.
        get_value: (a) => rt.encodeStr(node(rt.decodeInt(a[0])).value ?? ""),
        set_value: (a) => {
            node(rt.decodeInt(a[0])).value = rt.decodeStr(a[1]);
            return rt.encodeNone();
        },
        get_checked: (a) => rt.encodeBool(!!node(rt.decodeInt(a[0])).checked),
        set_checked: (a) => {
            node(rt.decodeInt(a[0])).checked = rt.decodeBool(a[1]);
            return rt.encodeNone();
        },

        form_submit: (a) => { node(rt.decodeInt(a[0])).submit(); return rt.encodeNone(); },
        form_reset: (a) => { node(rt.decodeInt(a[0])).reset(); return rt.encodeNone(); },

        // Values are arrays (uniform handling of multi-checkbox / multi-select). Files serialize as {__file__: true, name, size, type}.
        form_data: (a) => {
            const fd = new FormData(node(rt.decodeInt(a[0])));
            const out = {};
            for (const k of new Set(fd.keys())) {
                out[k] = fd.getAll(k).map(v => v instanceof File
                    ? { __file__: true, name: v.name, size: v.size, type: v.type }
                    : v
                );
            }
            return rt.encodeStr(JSON.stringify(out));
        },

        is_valid: (a) => rt.encodeBool(node(rt.decodeInt(a[0])).checkValidity()),

        validity: (a) => {
            const v = node(rt.decodeInt(a[0])).validity;
            return rt.encodeStr(JSON.stringify({
                valid: v.valid,
                value_missing: v.valueMissing,
                type_mismatch: v.typeMismatch,
                pattern_mismatch: v.patternMismatch,
                too_long: v.tooLong,
                too_short: v.tooShort,
                range_underflow: v.rangeUnderflow,
                range_overflow: v.rangeOverflow,
                step_mismatch: v.stepMismatch,
                bad_input: v.badInput,
                custom_error: v.customError,
            }));
        },

        // Also pops the browser's native validation tooltip near the invalid field.
        report_validity: (a) => rt.encodeBool(node(rt.decodeInt(a[0])).reportValidity()),

        // Pass `""` to clear a previously set custom error.
        set_custom_validity: (a) => {
            node(rt.decodeInt(a[0])).setCustomValidity(rt.decodeStr(a[1]));
            return rt.encodeNone();
        },

        validation_message: (a) => rt.encodeStr(node(rt.decodeInt(a[0])).validationMessage || ""),

        // CSV of file handles. Python: [int(h) for h in get_files(inp).split(",") if h].
        get_files: (a) => {
            const fl = node(rt.decodeInt(a[0])).files;
            if (!fl || fl.length === 0) return rt.encodeStr("");
            const out = new Array(fl.length);
            for (let i = 0; i < fl.length; i++) {
                files.push(fl[i]);
                out[i] = files.length - 1;
            }
            return rt.encodeStr(out.join(","));
        },

        file_info: (a) => {
            const f = files[rt.decodeInt(a[0])];
            if (!f) return rt.encodeNone();
            return rt.encodeStr(JSON.stringify({
                name: f.name,
                size: f.size,
                type: f.type,
                last_modified: f.lastModified,
            }));
        },

        // Async — result arrives via `receive()` as {msg, ok, text} or {msg, ok: false, error}.
        file_read_text: (a) => {
            const f = files[rt.decodeInt(a[0])];
            const msg = rt.decodeStr(a[1]);
            if (!f) return rt.encodeNone();
            const r = new FileReader();
            r.onload = () => window.dispatchEvent(new CustomEvent("edge-python-event", {
                detail: JSON.stringify({ msg, ok: true, text: r.result })
            }));
            r.onerror = () => window.dispatchEvent(new CustomEvent("edge-python-event", {
                detail: JSON.stringify({ msg, ok: false, error: String(r.error) })
            }));
            r.readAsText(f);
            return rt.encodeNone();
        },

        // Async; result via `receive()` as {msg, ok, data_url}. Strip "data:<mime>;base64," and b64decode for raw bytes.
        file_read_data_url: (a) => {
            const f = files[rt.decodeInt(a[0])];
            const msg = rt.decodeStr(a[1]);
            if (!f) return rt.encodeNone();
            const r = new FileReader();
            r.onload = () => window.dispatchEvent(new CustomEvent("edge-python-event", {
                detail: JSON.stringify({ msg, ok: true, data_url: r.result })
            }));
            r.onerror = () => window.dispatchEvent(new CustomEvent("edge-python-event", {
                detail: JSON.stringify({ msg, ok: false, error: String(r.error) })
            }));
            r.readAsDataURL(f);
            return rt.encodeNone();
        },
    };
