/* Form value/checked, submit/reset, FormData, Validity, file pickers. */

export default ({ node, files }, { pushEvent }) => ({
    /* `value`/`checked` are live state; `get_attribute("value")` reads the initial-value attribute instead. */
    get_value: (h) => node(h).value ?? '',
    set_value: (h, v) => { node(h).value = v; },
    get_checked: (h) => !!node(h).checked,
    set_checked: (h, v) => { node(h).checked = v; },

    form_submit: (h) => { node(h).submit(); },
    form_reset: (h) => { node(h).reset(); },

    /* Values are arrays (uniform handling of multi-checkbox / multi-select). Files serialize as {__file__: true, ...}. */
    form_data: (h) => {
        const fd = new FormData(node(h));
        const out = {};
        for (const k of new Set(fd.keys())) {
            out[k] = fd.getAll(k).map(v => v instanceof File
                ? { __file__: true, name: v.name, size: v.size, type: v.type }
                : v
            );
        }
        return JSON.stringify(out);
    },

    is_valid: (h) => node(h).checkValidity(),

    validity: (h) => {
        const v = node(h).validity;
        return JSON.stringify({
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
        });
    },

    /* Also pops the browser's native validation tooltip near the invalid field. */
    report_validity: (h) => node(h).reportValidity(),
    /* Pass `""` to clear a previously set custom error. */
    set_custom_validity: (h, msg) => { node(h).setCustomValidity(msg); },
    validation_message: (h) => node(h).validationMessage || '',

    /* CSV of file handles. Python: [int(h) for h in get_files(inp).split(",") if h]. */
    get_files: (h) => {
        const fl = node(h).files;
        if (!fl || fl.length === 0) return '';
        const out = new Array(fl.length);
        for (let i = 0; i < fl.length; i++) {
            files.push(fl[i]);
            out[i] = files.length - 1;
        }
        return out.join(',');
    },

    file_info: (h) => {
        const f = files[h];
        if (!f) return null;
        return JSON.stringify({ name: f.name, size: f.size, type: f.type, last_modified: f.lastModified });
    },

    /* Async — result arrives via `receive()` as {msg, ok, text} or {msg, ok: false, error}. */
    file_read_text: (h, msg) => {
        const f = files[h];
        if (!f) return;
        const r = new FileReader();
        r.onload = () => pushEvent(JSON.stringify({ msg, ok: true, text: r.result }));
        r.onerror = () => pushEvent(JSON.stringify({ msg, ok: false, error: String(r.error) }));
        r.readAsText(f);
    },

    /* Async; result via `receive()` as {msg, ok, data_url}. Strip "data:<mime>;base64," and b64decode for raw bytes. */
    file_read_data_url: (h, msg) => {
        const f = files[h];
        if (!f) return;
        const r = new FileReader();
        r.onload = () => pushEvent(JSON.stringify({ msg, ok: true, data_url: r.result }));
        r.onerror = () => pushEvent(JSON.stringify({ msg, ok: false, error: String(r.error) }));
        r.readAsDataURL(f);
    },
});
