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
    const ESC = { '&': '&amp;', '<': '&lt;', '>': '&gt;' };
    const esc = (s) => s.replace(/[&<>]/g, (c) => ESC[c]);
    const span = (cls, s) => `<span class="${cls}">${s}</span>`;

    const tokenize = (m, com, str, num, word, offset, full) => {
        if (com) return span('tk-com', com);
        if (str) {
            if (/^[fFrRbBuU]*[fF]/.test(str)) {
                let result = '';
                let last = 0;
                const fRe = /\{\{|\}\}|\{([^{}]*)\}/g;
                let fm;
                while ((fm = fRe.exec(str)) !== null) {
                    if (last < fm.index) result += span('tk-str', str.slice(last, fm.index));
                    if (fm[1] != null) {
                        const inner = fm[1].replace(new RegExp(TOKEN_RE.source, TOKEN_RE.flags), tokenize);
                        result += span('tk-fexpr', `{${inner}}`);
                    } else {
                        result += span('tk-str', fm[0]);
                    }
                    last = fRe.lastIndex;
                }
                if (last < str.length) result += span('tk-str', str.slice(last));
                return result;
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
    const worker = new Worker('./js/worker.js');

    /* Single source of truth for "runtime busy". Gates the button and Ctrl+Enter.
       Runs requested while busy are queued and fire on `ready` so Ctrl+Enter
       during WASM load doesn't get silently dropped. */
    let busy = true;
    let pendingRun = null;
    const setBusy = (b) => { busy = b; el.btn.disabled = b; };

    const resolveUrl = async () => {
        const ver = await fetch('./version.json', { cache: 'no-store' }).then(r => r.ok ? r.json() : {}).catch(() => ({}));
        const bust = ver.v ? `?v=${ver.v}` : '';
        const path = DEV
            ? `https://demo.edgepython.com/compiler_lib.wasm${bust}`
            : `./compiler_lib.wasm${bust}`;
        return new URL(path, location.href).toString();
    };

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
        line:   ({ line }) => { el.term.textContent += (el.term.textContent ? '\n' : '') + line; },
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
                url: await resolveUrl(),
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

// Pure-text editor. We take over Backspace, Tab, Shift+Tab and Enter with
// VSCode `useTabStops` semantics so contenteditable / NBSP / cross-browser
// quirks don't leak into indent behavior. CodeJar handles render+caret.

const Editor = (() => {
    const PAIRS = { '(': ')', '[': ']', '{': '}', '"': '"', "'": "'" };
    const OPENERS = new Set(Object.keys(PAIRS));
    const CLOSERS = new Set(Object.values(PAIRS));
    const STRING_START = /^([fFrRbBuU]{0,2})("""|'''|"|')/;
    const WS = /[ \t\u00a0]/;

    // Tiny state machine: walk from 0 to caret toggling between code/string; `quote === ''` means we're in code; otherwise we're inside that string.
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

    // Locate the line containing `caret` and the column within it.
    const lineAt = (text, caret) => {
        const start = text.lastIndexOf('\n', caret - 1) + 1;
        const nl = text.indexOf('\n', caret);
        const end = nl === -1 ? text.length : nl;
        return { start, end, body: text.slice(start, end), col: caret - start };
    };
    // Index of first non-whitespace char in `s`, or s.length if none.
    const firstNonWS = (s) => { for (let i = 0; i < s.length; i++) if (!WS.test(s[i])) return i; return s.length; };
    // VSCode prevIndentTabStop distance: chars to delete to reach previous stop.
    const prevTabDist = (col) => { const r = col % TAB_SIZE; return r === 0 ? TAB_SIZE : r; };

    /* Caret position right after the last auto-inserted pair. Pair-deletion on
       Backspace only fires when caret still matches; cleared on any other input
       so manually-typed parens aren't both eaten by a single backspace. */
    let autoPairCaret = -1;

    const transitions = {
        // Typed character: skip existing closer, or auto-close an opener.
        char: (text, caret, key) => {
            if (CLOSERS.has(key) && text[caret] === key) {
                return { text, caret: caret + 1 };
            }
            if (OPENERS.has(key)) {
                const { inStr, isF } = stringCtx(text, caret);
                // Inside a normal string we don't auto-close anything; inside an f-string we still want `{` -> `{}` for expressions.
                if (inStr && !(isF && key === '{')) return null;
                // Triple-quote opening: "" + " -> """""" (or '' + ' -> '''''')
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
            }
            return null;
        },

        // Backspace: delete fresh auto-close pair (only when caret matches the
        // last auto-pair marker), else VSCode `useTabStops` snap in leading WS.
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

        // Shift+Tab: dedent the current line by one tab stop, snapped.
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

        // Enter: copy previous line's indent, add one tab if the line ends with
        // `:` `[` `(` `{`. If we're between an opener and its matching closer,
        // split the closer onto its own line (VSCode 3-line block).
        enter: (text, caret) => {
            const ln = lineAt(text, caret);
            const before = ln.body.slice(0, ln.col);
            const indentMatch = before.match(/^[ \t\u00a0]*/);
            const indent = indentMatch ? indentMatch[0] : '';
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

        // Tab on a multi-line selection: prepend TAB_SIZE spaces to every touched line.
        tabSelection: (text, sel) => {
            const startLine = text.lastIndexOf('\n', sel.start - 1) + 1;
            const endAnchor = (sel.end > sel.start && text[sel.end - 1] === '\n') ? sel.end - 1 : sel.end;
            let endNl = text.indexOf('\n', endAnchor);
            if (endNl === -1) endNl = text.length;
            const lines = text.slice(startLine, endNl).split('\n');
            const pad = ' '.repeat(TAB_SIZE);
            const segment = lines.map(l => pad + l).join('\n');
            return {
                text: text.slice(0, startLine) + segment + text.slice(endNl),
                caretStart: sel.start + TAB_SIZE,
                caretEnd: sel.end + lines.length * TAB_SIZE,
            };
        },

        // Shift+Tab on selection: dedent each touched line by up to TAB_SIZE.
        shiftTabSelection: (text, sel) => {
            const startLine = text.lastIndexOf('\n', sel.start - 1) + 1;
            const endAnchor = (sel.end > sel.start && text[sel.end - 1] === '\n') ? sel.end - 1 : sel.end;
            let endNl = text.indexOf('\n', endAnchor);
            if (endNl === -1) endNl = text.length;
            const lines = text.slice(startLine, endNl).split('\n');
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
                text: text.slice(0, startLine) + dedented.join('\n') + text.slice(endNl),
                caretStart: Math.max(startLine, sel.start - firstRm),
                caretEnd: sel.end - totalRm,
            };
        },
    };

    // Disable CodeJar's Tab/Enter/Backspace handling - we own them. CodeJar
    // keeps render, save/restore, history, paste/cut.
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

    el.ed.addEventListener('keydown', (e) => {
        if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
            e.preventDefault(); PythonWorker.run(jar.toString()); return;
        }
        if (e.key === 'Enter' && jar.toString().split('\n').length >= MAX_LINES) {
            e.preventDefault(); return;
        }
        const pos = jar.save();
        const text = jar.toString();
        let result;
        if (pos.start !== pos.end) {
            // Multi-line indent/dedent is the only selection-aware path we own.
            if (e.key === 'Tab' && e.shiftKey)      result = transitions.shiftTabSelection(text, pos);
            else if (e.key === 'Tab')               result = transitions.tabSelection(text, pos);
            else return;
        } else {
            result =
                e.key === 'Backspace' ? transitions.backspace(text, pos.start) :
                e.key === 'Enter'     ? transitions.enter(text, pos.start) :
                e.key === 'Tab' && e.shiftKey ? transitions.shiftTab(text, pos.start) :
                e.key === 'Tab'       ? transitions.tab(text, pos.start) :
                (OPENERS.has(e.key) || CLOSERS.has(e.key)) ? transitions.char(text, pos.start, e.key) :
                null;
        }

        // stopImmediatePropagation also blocks CodeJar's own keydown listener on
        // the same element; otherwise it can re-run indent logic and undo ours.
        if (apply(result)) { e.preventDefault(); e.stopImmediatePropagation(); }
    }, true);

    el.ed.addEventListener('scroll', () => { el.ln.scrollTop = el.ed.scrollTop; });

    // Any text mutation outside our `apply()` invalidates the auto-pair marker
    // so a manually-typed pair never collapses on backspace.
    el.ed.addEventListener('input', () => { autoPairCaret = -1; });

    // Paste: normalise CRLF/CR + tabs, enforce MAX_LINES on the resulting text.
    el.ed.addEventListener('paste', (e) => {
        const raw = (e.clipboardData ?? window.clipboardData)?.getData('text');
        if (raw == null) return;
        e.preventDefault();
        const normalized = raw.replace(/\r\n?/g, '\n').replace(/\t/g, ' '.repeat(TAB_SIZE));
        const pos = jar.save();
        const text = jar.toString();
        let inserted = text.slice(0, pos.start) + normalized + text.slice(pos.end);
        const allLines = inserted.split('\n');
        if (allLines.length > MAX_LINES) inserted = allLines.slice(0, MAX_LINES).join('\n');
        jar.updateCode(inserted);
        const caret = Math.min(pos.start + normalized.length, inserted.length);
        jar.restore({ start: caret, end: caret });
        autoPairCaret = -1;
    });

    // Copy: wrap the selection in a ```python fence for markdown-friendly pasting.
    el.ed.addEventListener('copy', (e) => {
        const sel = window.getSelection();
        const selected = sel?.toString() ?? '';
        if (!selected) return;
        e.preventDefault();
        const trimmed = selected.replace(/\n+$/, '');
        e.clipboardData.setData('text/plain', '```python\n' + trimmed + '\n```');
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