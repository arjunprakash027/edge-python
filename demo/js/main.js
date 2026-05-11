import { CodeJar } from 'https://esm.sh/codejar@4';

// Config

const MAX_LINES = 99;
const TAB_SIZE = 4;
const ENTRY_PATH = "runtime/perceptron.py";
const ENTRY_DIR = ENTRY_PATH.includes('/')
    ? ENTRY_PATH.slice(0, ENTRY_PATH.lastIndexOf('/') + 1)
    : '';
const DEV = !['demo.edgepython.com'].includes(location.hostname);
const FETCH_OPTS = DEV ? { cache: 'no-store' } : undefined;

/* Resolve the versioned WASM URL as soon as main.js parses, in parallel
   with the worker boot. In PROD we also kick the WASM fetch itself so the
   bytes live in the HTTP cache by the time the worker asks for them; the
   worker's `compileStreaming(fetch(url))` then reuses that response. In
   DEV the `no-store` opts would make an eager fetch turn into a second
   round-trip, so we just resolve the URL and let the worker fetch. */
const WASM_URL_PROMISE = (async () => {
    const ver = await fetch('./version.json', { cache: 'no-store' })
        .then(r => r.ok ? r.json() : {}).catch(() => ({}));
    const bust = ver.v ? `?v=${ver.v}` : '';
    const path = DEV
        ? `https://demo.edgepython.com/compiler_lib.wasm${bust}`
        : `./compiler_lib.wasm${bust}`;
    const url = new URL(path, location.href).toString();
    if (!DEV) fetch(url);
    return url;
})();

const DEFAULT_CODE = `"""\nFunctional pipeline using lambdas, closures and list comprehensions.\nReference: Backus, J. (1978).\n"""\n\ndouble = lambda n: n * 2\nsquare = lambda n: n * n\n\ndef compose(*fns):\n    def piped(x):\n        for f in fns:\n            x = f(x)\n        return x\n    return piped\n\npipeline = compose(double, square)\n\ndata = [1, 2, 3]\nresult = [pipeline(x) for x in data]\n\nprint(f"Input:  {data}")\nprint(f"Output: {result}")`;

// DOM

const $ = (id) => document.getElementById(id);
const el = {
    ed: $('ed'), ln: $('ln'), btn: $('run'),
    term: $('term'), status: $('status'),
};

// Utils

const fmt = (ms) => ms < 1000 ? `${ms.toFixed(0)}ms` : `${(ms / 1000).toFixed(2)}s`;

const fetchSvg = async (src, attrs = {}) => {
    const text = await fetch(src).then(r => r.text()).catch(() => '');
    const svg = new DOMParser().parseFromString(text, 'image/svg+xml').querySelector('svg');
    if (svg) for (const [k, v] of Object.entries(attrs)) svg.setAttribute(k, v);
    return svg;
};

const loadIcons = (scope = document) => Promise.all(
    [...scope.querySelectorAll('svg[data-icon]')].map(async (node) => {
        const attrs = Object.fromEntries(
            [...node.attributes].filter(a => a.name !== 'data-icon').map(a => [a.name, a.value])
        );
        const svg = await fetchSvg(node.getAttribute('data-icon'), attrs);
        if (svg) node.replaceWith(svg);
    })
);

const setStatus = (text, cls) => {
    el.status.textContent = text;
    el.status.className = `ml-auto ${cls}`;
};
const ok  = (t) => setStatus(t, 'status-ok');
const err = (t) => setStatus(t, 'status-err');

// Highlighter

const Highlighter = (() => {
    const KW = new Set(['as','if','in','is','or','and','def','del','for','not','try','case','elif','else','from','pass','type','with','async','await','break','class','match','raise','while','yield','assert','except','global','import','lambda','return','finally','continue','nonlocal']);
    const BI = new Set(['print','len','range','int','str','float','list','dict','tuple','set','bool','isinstance','enumerate','zip','abs','min','max','sum','round','pow','divmod','hash','id','repr','ord','chr','hex','oct','bin','input','reversed','sorted','any','all','format','ascii','callable','getattr','hasattr','type','self','cls']);
    const LIT = new Set(['True', 'False', 'None']);
    const CLASSES = [[KW, 'tk-kw'], [LIT, 'tk-lit'], [BI, 'tk-bi']];

    const TOKEN_RE = /(#[^\n]*)|((?:\b[fFrRbBuU]{1,2})?(?:"""[\s\S]*?"""|'''[\s\S]*?'''|"(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])*'))|(0[xX][\da-fA-F_]+|0[oO][0-7_]+|0[bB][01_]+|\d[\d_]*(?:\.[\d_]*)?(?:[eE][+-]?\d+)?[jJ]?|\.\d[\d_]*(?:[eE][+-]?\d+)?[jJ]?)|([A-Za-z_]\w*)/g;
    // Pre-compiled once; `replace` resets lastIndex per call so re-entry is safe.
    const F_INNER_RE = /\{\{|\}\}|\{([^{}]*)\}/g;
    const ESC = { '&': '&amp;', '<': '&lt;', '>': '&gt;' };
    const esc = (s) => s.replace(/[&<>]/g, (c) => ESC[c]);
    const span = (cls, s) => `<span class="${cls}">${s}</span>`;

    const tokenize = (m, com, str, num, word, offset, full) => {
        if (com) return span('tk-com', com);
        if (str) {
            if (/^[fFrRbBuU]*[fF]/.test(str)) {
                const parts = [];
                let last = 0;
                F_INNER_RE.lastIndex = 0;
                let fm;
                while ((fm = F_INNER_RE.exec(str)) !== null) {
                    if (last < fm.index) parts.push(span('tk-str', str.slice(last, fm.index)));
                    if (fm[1] != null) {
                        parts.push(span('tk-fexpr', `{${fm[1].replace(TOKEN_RE, tokenize)}}`));
                    } else {
                        parts.push(span('tk-str', fm[0]));
                    }
                    last = F_INNER_RE.lastIndex;
                }
                if (last < str.length) parts.push(span('tk-str', str.slice(last)));
                return parts.join('');
            }
            return span('tk-str', str);
        }
        if (num) return span('tk-num', num);
        if (word) {
            if (full[offset - 1] === '&' && full[offset + word.length] === ';') return word;
            for (const [set, cls] of CLASSES) if (set.has(word)) return span(cls, word);
            if (/^[A-Z]/.test(word)) return span('tk-class', word);
            return span(/^\s*\(/.test(full.slice(offset + word.length)) ? 'tk-func' : 'tk-var', word);
        }
        return m;
    };

    return { highlight: (src) => esc(src).replace(TOKEN_RE, tokenize) };
})();

// Worker

const PythonWorker = (() => {
    // `{ type: 'module' }` lets worker.js `import` from ./worker/*.js without a build step.
    const worker = new Worker('./js/worker.js', { type: 'module' });

    /* Single source of truth for "runtime busy". Gates the button and Ctrl+Enter.
       Runs requested while busy are queued and fire on `ready` so Ctrl+Enter
       during WASM load doesn't get silently dropped. */
    let busy = true;
    let pendingRun = null;
    const setBusy = (b) => { busy = b; el.btn.disabled = b; };

    const doRun = (src) => {
        setBusy(true);
        ok('Running...');
        el.term.textContent = '';
        worker.postMessage({ type: 'run', src, baseUrl: location.href, entryDir: ENTRY_DIR });
    };

    const onMsg = {
        ready:  ({ ms }) => {
            setBusy(false);
            ok(`Ready${DEV ? ' - Dev' : ''} (Loaded in ${fmt(ms)})`);
            if (pendingRun != null) { const src = pendingRun; pendingRun = null; doRun(src); }
        },
        line:   ({ line }) => {
            // `append` adds a text node in O(1); `+= text` rebuilds the whole string each call.
            if (el.term.firstChild) el.term.append('\n');
            el.term.append(line);
        },
        result: ({ out, ms }) => {
            if (out) {
                el.term.textContent = out;
                err(`Failed in ${fmt(ms)}`);
            } else {
                ok(`Ran in ${fmt(ms)}`);
            }
            setBusy(false);
        },
        error:  ({ message }) => { setBusy(false); err('Load failed'); el.term.textContent = `Could not load WASM.\n\n${message}`; },
    };
    worker.onmessage = ({ data }) => onMsg[data.type]?.(data);

    return {
        load: async () => {
            ok('Loading WASM...');
            worker.postMessage({
                type: 'load',
                url: await WASM_URL_PROMISE,
                opts: FETCH_OPTS ?? {},
                baseUrl: location.href,
            });
        },
        run: (src) => {
            if (busy) { pendingRun = src; ok('Queued — runtime not ready'); return; }
            doRun(src);
        },
    };
})();

/* Pure-text editor over CodeJar.
   We own Backspace, Tab/Shift+Tab, Enter, `:` (Python dedent), bracket auto-pair,
   selection-wrap, paste/drop normalisation, and copy/cut markdown fencing.
   CodeJar handles render, save/restore and history. */

const Editor = (() => {

    // --- constants ---

    const PAIRS = { '(': ')', '[': ']', '{': '}', '"': '"', "'": "'" };
    const OPENERS = new Set(Object.keys(PAIRS));
    const CLOSERS = new Set(Object.values(PAIRS));
    const STRING_START = /^([fFrRbBuU]{0,2})("""|'''|"|')/;
    const WS = /[ \t ]/;
    // Python `decreaseIndentPattern` (VSCode-compatible subset). Dedent on `:`.
    const DEDENT_RE = /^\s*(?:elif|else|except|finally|case)\b[^:]*:$/;
    const HTML_ESC = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' };
    const escapeHtml = (s) => s.replace(/[&<>"]/g, (c) => HTML_ESC[c]);

    // --- pure helpers ---

    // Walk from 0 to caret toggling code/string. `quote === ''` means code.
    const stringCtx = (src, caret) => {
        let i = 0, quote = '', isF = false;
        while (i < caret) {
            if (!quote) {
                if (src[i] === '#') {
                    const nl = src.indexOf('\n', i);
                    if (nl === -1 || nl >= caret) return { inStr: false, isF: false };
                    i = nl + 1; continue;
                }
                const m = src.slice(i).match(STRING_START);
                if (m && i + m[0].length <= caret) {
                    quote = m[2]; isF = /[fF]/.test(m[1]);
                    i += m[0].length; continue;
                }
                i++;
            } else {
                if (quote.length === 1 && src[i] === '\\') { i += 2; continue; }
                if (src.slice(i, i + quote.length) === quote) {
                    i += quote.length; quote = ''; isF = false; continue;
                }
                i++;
            }
        }
        return { inStr: !!quote, isF };
    };

    const lineAt = (text, caret) => {
        const start = text.lastIndexOf('\n', caret - 1) + 1;
        const nl = text.indexOf('\n', caret);
        const end = nl === -1 ? text.length : nl;
        return { start, end, body: text.slice(start, end), col: caret - start };
    };
    const firstNonWS = (s) => { for (let i = 0; i < s.length; i++) if (!WS.test(s[i])) return i; return s.length; };
    // VSCode `prevIndentTabStop`: distance to the previous tab stop from `col`.
    const prevTabDist = (col) => { const r = col % TAB_SIZE; return r === 0 ? TAB_SIZE : r; };

    // Range of full lines touched by `sel`. Excludes a trailing line whose
    // start is exactly at sel.end (selection ends just past a `\n`).
    const lineRange = (text, sel) => {
        const start = text.lastIndexOf('\n', sel.start - 1) + 1;
        const anchor = (sel.end > sel.start && text[sel.end - 1] === '\n') ? sel.end - 1 : sel.end;
        let end = text.indexOf('\n', anchor);
        if (end === -1) end = text.length;
        return { start, end };
    };

    // Strip the common leading-whitespace prefix from non-empty lines so a
    // snippet pasted from a deeper context lands flush with the cursor.
    const dedentCommon = (s) => {
        const lines = s.split('\n');
        const nonEmpty = lines.filter(l => l.trim().length > 0);
        if (nonEmpty.length === 0) return s;
        const min = Math.min(...nonEmpty.map(l => l.match(/^[ \t]*/)[0].length));
        return min ? lines.map(l => l.slice(Math.min(l.length, min))).join('\n') : s;
    };

    // Round-trip helper for paste: strip our own ```python fence transparently.
    const unwrapFence = (s) => {
        const m = s.match(/^```python\n([\s\S]*?)\n```\s*$/);
        return m ? m[1] : s;
    };

    // AltGr produces a printable symbol while reporting Ctrl+Alt — distinguish
    // from real Ctrl+Alt shortcuts (which produce a letter/digit `key`).
    const isAltGrChar = (e) =>
        e.ctrlKey && e.altKey && e.key.length === 1 && !/^[A-Za-z0-9]$/.test(e.key);

    // --- mutable state ---

    /* Caret right after the last auto-inserted pair; pair-collapse on Backspace
       only fires while caret still matches. Cleared on any non-our text mutation. */
    let autoPairCaret = -1;
    // True while the IME has an open composition; we must stay out of the way.
    let composing = false;

    // --- transitions ---

    const transitions = {
        // Typed character: skip existing closer, or auto-close an opener.
        char: (text, caret, key) => {
            if (CLOSERS.has(key) && text[caret] === key) {
                return { text, caret: caret + 1 };
            }
            if (!OPENERS.has(key)) return null;
            const { inStr, isF } = stringCtx(text, caret);
            // Inside a regular string don't auto-close anything; inside an f-string
            // we still expand `{` to `{}` so expression interpolation works.
            if (inStr && !(isF && key === '{')) return null;
            // Triple-quote opening: "" + " -> """""" (or '' + ' -> '''''').
            if ((key === '"' || key === "'") && caret >= 2 && text[caret - 2] === key && text[caret - 1] === key) {
                return {
                    text: text.slice(0, caret - 2) + key.repeat(6) + text.slice(caret),
                    caret: caret - 2 + 3,
                };
            }
            return {
                text: text.slice(0, caret) + key + PAIRS[key] + text.slice(caret),
                caret: caret + 1,
                autoPair: caret + 1,
            };
        },

        // Wrap a non-empty selection in opener/closer (VSCode `autoSurround`).
        wrapSelection: (text, sel, key) => {
            const closer = PAIRS[key];
            return {
                text: text.slice(0, sel.start) + key + text.slice(sel.start, sel.end) + closer + text.slice(sel.end),
                caretStart: sel.start + 1,
                caretEnd: sel.end + 1,
            };
        },

        // Backspace: collapse a fresh auto-pair, else VSCode `useTabStops` snap
        // when the caret is inside the line's leading whitespace.
        backspace: (text, caret) => {
            if (caret === 0) return null;
            if (caret === autoPairCaret && PAIRS[text[caret - 1]] === text[caret]) {
                return { text: text.slice(0, caret - 1) + text.slice(caret + 1), caret: caret - 1 };
            }
            if (!WS.test(text[caret - 1])) return null;
            const ln = lineAt(text, caret);
            if (ln.col === 0 || ln.col > firstNonWS(ln.body)) return null;
            const dist = prevTabDist(ln.col);
            return { text: text.slice(0, caret - dist) + text.slice(caret), caret: caret - dist };
        },

        // Tab: insert spaces to the next tab stop based on the current column.
        tab: (text, caret) => {
            const ln = lineAt(text, caret);
            const pad = ' '.repeat(TAB_SIZE - (ln.col % TAB_SIZE));
            return { text: text.slice(0, caret) + pad + text.slice(caret), caret: caret + pad.length };
        },

        // Shift+Tab: dedent the current line by up to one tab stop.
        shiftTab: (text, caret) => {
            const ln = lineAt(text, caret);
            const indent = firstNonWS(ln.body);
            if (indent === 0) return null;
            const newIndent = Math.floor((indent - 1) / TAB_SIZE) * TAB_SIZE;
            const removed = indent - newIndent;
            const newCaret = ln.col >= removed ? caret - removed : ln.start;
            return {
                text: text.slice(0, ln.start) + ln.body.slice(removed) + text.slice(ln.end),
                caret: newCaret,
            };
        },

        // Enter: inherit previous indent, +1 level after `:` / `[` / `(` / `{`.
        // Between an opener and its matching closer, split the closer onto its
        // own line (VSCode `IndentOutdent` 3-line block).
        enter: (text, caret) => {
            const ln = lineAt(text, caret);
            const before = ln.body.slice(0, ln.col);
            const indent = (before.match(/^[ \t ]*/) || [''])[0];
            const extra = /[:\[({][ \t]*$/.test(before) ? ' '.repeat(TAB_SIZE) : '';
            const pad = indent + extra;
            const opener = before.replace(/[ \t]+$/, '').slice(-1);
            const splitBracket = '[({'.includes(opener) && PAIRS[opener] === text[caret];
            if (splitBracket) {
                return {
                    text: text.slice(0, caret) + '\n' + pad + '\n' + indent + text.slice(caret),
                    caret: caret + 1 + pad.length,
                };
            }
            return { text: text.slice(0, caret) + '\n' + pad + text.slice(caret), caret: caret + 1 + pad.length };
        },

        // `:` after `else` / `elif` / `except` / `finally` / `case` head —
        // insert the colon then dedent the line by one tab stop.
        colon: (text, caret) => {
            const inserted = text.slice(0, caret) + ':' + text.slice(caret);
            const ln = lineAt(inserted, caret + 1);
            if (!DEDENT_RE.test(ln.body)) return null;
            const indent = firstNonWS(ln.body);
            const remove = Math.min(indent, TAB_SIZE);
            if (remove === 0) return { text: inserted, caret: caret + 1 };
            return {
                text: inserted.slice(0, ln.start) + inserted.slice(ln.start + remove),
                caret: caret + 1 - remove,
            };
        },

        // Tab on selection: prepend TAB_SIZE spaces to every line in the range.
        tabSelection: (text, sel) => {
            const { start, end } = lineRange(text, sel);
            const lines = text.slice(start, end).split('\n');
            const pad = ' '.repeat(TAB_SIZE);
            return {
                text: text.slice(0, start) + lines.map(l => pad + l).join('\n') + text.slice(end),
                caretStart: sel.start + TAB_SIZE,
                caretEnd: sel.end + lines.length * TAB_SIZE,
            };
        },

        // Shift+Tab on selection: dedent each line in the range by up to TAB_SIZE.
        shiftTabSelection: (text, sel) => {
            const { start, end } = lineRange(text, sel);
            const lines = text.slice(start, end).split('\n');
            let firstRm = 0, totalRm = 0;
            const dedented = lines.map((l, i) => {
                const indent = firstNonWS(l);
                const rm = Math.min(indent, TAB_SIZE);
                totalRm += rm;
                if (i === 0) firstRm = rm;
                return l.slice(rm);
            });
            if (totalRm === 0) return null;
            return {
                text: text.slice(0, start) + dedented.join('\n') + text.slice(end),
                caretStart: Math.max(start, sel.start - firstRm),
                caretEnd: sel.end - totalRm,
            };
        },
    };

    // --- CodeJar wiring ---

    const jar = CodeJar(el.ed,
        (ed) => { ed.innerHTML = Highlighter.highlight(ed.textContent); },
        { spellcheck: false, addClosing: false, catchTab: false, preserveIdent: false }
    );

    const apply = (result) => {
        if (!result) return false;
        jar.updateCode(result.text);
        const start = result.caretStart ?? result.caret;
        const end = result.caretEnd ?? result.caret;
        jar.restore({ start, end });
        autoPairCaret = result.autoPair ?? -1;
        return true;
    };

    const syncLines = () => {
        const lines = jar.toString().replace(/\n$/, '').split('\n');
        const n = Math.max(1, Math.min(lines.length, MAX_LINES));
        el.ln.textContent = Array.from({ length: n }, (_, i) => String(i + 1).padStart(2, '0')).join('\n');
        el.ln.scrollTop = el.ed.scrollTop;
    };

    // Insert plain text at the caret, replacing any selection. Used by paste
    // and drop after normalisation; enforces MAX_LINES on the resulting text.
    const insertAtCaret = (str) => {
        const pos = jar.save();
        const text = jar.toString();
        let next = text.slice(0, pos.start) + str + text.slice(pos.end);
        const all = next.split('\n');
        if (all.length > MAX_LINES) next = all.slice(0, MAX_LINES).join('\n');
        jar.updateCode(next);
        const caret = Math.min(pos.start + str.length, next.length);
        jar.restore({ start: caret, end: caret });
        autoPairCaret = -1;
    };

    // --- listeners ---

    /* IME composition flag mirrors `event.isComposing`. We track it explicitly
       because the `keydown` right after `compositionend` still needs to be a
       no-op on some platforms (Linux/Wayland) where the bridge re-fires. */
    el.ed.addEventListener('compositionstart', () => { composing = true; });
    el.ed.addEventListener('compositionend',   () => { composing = false; });

    el.ed.addEventListener('keydown', (e) => {
        // IME in progress, dead-key sequence, or AltGr producing a printable char:
        // never intercept. Each path corrupts accents/CJK input otherwise.
        if (composing || e.isComposing || e.keyCode === 229) return;
        if (e.key === 'Dead') return;
        if (isAltGrChar(e)) return;

        // Ctrl/Cmd+Enter is the only modifier shortcut we own.
        if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
            e.preventDefault(); PythonWorker.run(jar.toString()); return;
        }
        // Any other modifier-bearing keystroke (Ctrl+S, Cmd+C, Alt+arrow, …)
        // belongs to the browser / CodeJar.
        if (e.ctrlKey || e.metaKey || e.altKey) return;

        // Hard cap on document size: block Enter once we'd exceed MAX_LINES.
        if (e.key === 'Enter' && jar.toString().split('\n').length >= MAX_LINES) {
            e.preventDefault(); return;
        }

        const pos = jar.save();
        const text = jar.toString();
        let result;
        if (pos.start !== pos.end) {
            // Selection-aware: multi-line indent/dedent + bracket wrap.
            if (e.key === 'Tab' && e.shiftKey) result = transitions.shiftTabSelection(text, pos);
            else if (e.key === 'Tab')          result = transitions.tabSelection(text, pos);
            else if (OPENERS.has(e.key))       result = transitions.wrapSelection(text, pos, e.key);
            else return;
        } else {
            result =
                e.key === 'Backspace' ? transitions.backspace(text, pos.start) :
                e.key === 'Enter'     ? transitions.enter(text, pos.start) :
                e.key === 'Tab' && e.shiftKey ? transitions.shiftTab(text, pos.start) :
                e.key === 'Tab'       ? transitions.tab(text, pos.start) :
                e.key === ':'         ? transitions.colon(text, pos.start) :
                (OPENERS.has(e.key) || CLOSERS.has(e.key)) ? transitions.char(text, pos.start, e.key) :
                null;
        }

        // stopImmediatePropagation also blocks CodeJar's bubble-phase keydown
        // listener on the same element; without it CodeJar can re-run indent
        // logic on the freshly-mutated DOM and drift our caret.
        if (apply(result)) { e.preventDefault(); e.stopImmediatePropagation(); }
    }, true);

    el.ed.addEventListener('scroll', () => { el.ln.scrollTop = el.ed.scrollTop; });

    // Any text mutation outside our `apply()` invalidates the auto-pair marker
    // so a manually-typed pair never collapses on backspace.
    el.ed.addEventListener('input', () => { autoPairCaret = -1; });

    // Paste: strip our own ```python fence (round-trip), normalise CRLF + tabs,
    // smart-dedent the common indent, enforce MAX_LINES.
    el.ed.addEventListener('paste', (e) => {
        const raw = (e.clipboardData ?? window.clipboardData)?.getData('text');
        if (raw == null) return;
        e.preventDefault();
        const normalized = unwrapFence(raw).replace(/\r\n?/g, '\n').replace(/\t/g, ' '.repeat(TAB_SIZE));
        insertAtCaret(dedentCommon(normalized));
    });

    // Drag-and-drop of text or .py files. Fires `drop`, not `paste`.
    const hasTextOrFiles = (dt) => {
        if (!dt) return false;
        const types = Array.from(dt.types || []);
        return types.includes('text/plain') || types.includes('Files');
    };
    el.ed.addEventListener('dragover', (e) => { if (hasTextOrFiles(e.dataTransfer)) e.preventDefault(); });
    el.ed.addEventListener('drop', async (e) => {
        const dt = e.dataTransfer; if (!hasTextOrFiles(dt)) return;
        e.preventDefault();
        let raw = '';
        if (dt.files.length) {
            const f = dt.files[0];
            if (f.name.endsWith('.py') || (f.type || '').startsWith('text/')) {
                try { raw = await f.text(); } catch {}
            }
        } else {
            raw = dt.getData('text/plain') ?? '';
        }
        if (!raw) return;
        const normalized = unwrapFence(raw).replace(/\r\n?/g, '\n').replace(/\t/g, ' '.repeat(TAB_SIZE));
        insertAtCaret(dedentCommon(normalized));
    });

    /* Build the clipboard payload for copy/cut. On collapsed selection we
       fall back to the whole current line (VSCode parity); otherwise we use
       the visible selection. Returns plain (```python fence) + html (<pre>) so
       rich-text targets get a styled block. */
    const copyPayload = () => {
        const sel = window.getSelection();
        let snippet = sel?.toString() ?? '';
        let collapsed = false;
        if (!snippet) {
            const pos = jar.save();
            const ln = lineAt(jar.toString(), pos.start);
            snippet = ln.body;
            collapsed = true;
            if (!snippet) return null;
        }
        const trimmed = snippet.replace(/\n+$/, '');
        return {
            plain: '```python\n' + trimmed + '\n```',
            html: `<pre><code class="language-python">${escapeHtml(trimmed)}</code></pre>`,
            collapsed,
        };
    };

    el.ed.addEventListener('copy', (e) => {
        const p = copyPayload(); if (!p) return;
        e.preventDefault();
        e.clipboardData.setData('text/plain', p.plain);
        e.clipboardData.setData('text/html', p.html);
    });

    el.ed.addEventListener('cut', (e) => {
        const p = copyPayload(); if (!p) return;
        e.preventDefault();
        e.clipboardData.setData('text/plain', p.plain);
        e.clipboardData.setData('text/html', p.html);
        // Now actually remove the source. Collapsed-cut deletes the whole line
        // (including its trailing newline) so consecutive cuts compact upward.
        const text = jar.toString();
        const pos = jar.save();
        let next, caret;
        if (p.collapsed) {
            const ln = lineAt(text, pos.start);
            const removeEnd = ln.end < text.length ? ln.end + 1 : ln.end;
            next = text.slice(0, ln.start) + text.slice(removeEnd);
            caret = ln.start;
        } else {
            next = text.slice(0, pos.start) + text.slice(pos.end);
            caret = pos.start;
        }
        jar.updateCode(next);
        jar.restore({ start: caret, end: caret });
        autoPairCaret = -1;
    });

    jar.onUpdate(syncLines);
    jar.updateCode(DEFAULT_CODE);

    return { getCode: () => jar.toString(), setCode: (code) => jar.updateCode(code) };
})();

// Init
el.btn.addEventListener('click', () => PythonWorker.run(Editor.getCode()));
loadIcons();
PythonWorker.load();

fetch(`./${ENTRY_PATH}`, FETCH_OPTS ?? {}).then(r => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        return r.text();
    }).then(code => {
        Editor.setCode(code.replace(/\r\n?/g, '\n'));
    }).catch(() => {
        console.warn(`${ENTRY_PATH} could not be loaded, using default code.`);
    }
);
