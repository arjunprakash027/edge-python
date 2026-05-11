/* Demo entry point.
   Owns config, DOM lookup, status helpers and the Python worker shim;
   delegates the editor to ./main/editor.js (which pulls in the highlighter). */

import { createEditor } from './main/editor.js';

// Config

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

// Init

const Editor = createEditor({
    ed: el.ed,
    ln: el.ln,
    defaultCode: DEFAULT_CODE,
    onRun: (src) => PythonWorker.run(src),
});

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
