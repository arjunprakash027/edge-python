/* 
Demo entry point: config, DOM, status helpers, Python worker shim; editor lives in ./main/editor.js. 
*/

import { createEditor } from './main/editor.js';

// Config

const ENTRY_PATH = "runtime/perceptron.py";
const ENTRY_DIR = ENTRY_PATH.slice(0, ENTRY_PATH.lastIndexOf('/') + 1);
const DEV = !['demo.edgepython.com'].includes(location.hostname);
const FETCH_OPTS = DEV ? { cache: 'no-store' } : {};

/* Single fetch in parallel with worker boot; the body stream is transferred to the worker, so `compileStreaming` runs off-main-thread without a second hit. */
const WASM_FETCH = (async () => {
    const ver = await fetch('./version.json', { cache: 'no-store' }).then(r => r.json()).catch(() => ({}));
    const bust = ver.v ? `?v=${ver.v}` : '';
    const base = DEV ? 'https://demo.edgepython.com/compiler_lib.wasm' : './compiler_lib.wasm';
    const url = new URL(base + bust, location.href).toString();
    const response = await fetch(url, FETCH_OPTS);
    if (!response.ok) throw new Error(`HTTP ${response.status} fetching compiler_lib.wasm`);
    return { body: response.body, version: ver.v };
})();

const DEFAULT_CODE = `"""\nFunctional pipeline using lambdas, closures and list comprehensions.\nReference: Backus, J. (1978).\n"""\n\ndouble = lambda n: n * 2\nsquare = lambda n: n * n\n\ndef compose(*fns):\n    def piped(x):\n        for f in fns:\n            x = f(x)\n        return x\n    return piped\n\npipeline = compose(double, square)\n\ndata = [1, 2, 3]\nresult = [pipeline(x) for x in data]\n\nprint(f"Input:  {data}")\nprint(f"Output: {result}")`;

// DOM and utils

const $ = (id) => document.getElementById(id);
const el = { ed: $('ed'), ln: $('ln'), btn: $('run'), term: $('term'), status: $('status') };

const fmt = (ms) => ms < 1000 ? `${ms.toFixed(0)}ms` : `${(ms / 1000).toFixed(2)}s`;

const setStatus = (text, cls) => {
    el.status.textContent = text;
    el.status.className = `ml-auto ${cls}`;
};
const ok = (t) => setStatus(t, 'status-ok');
const err = (t) => setStatus(t, 'status-err');

const loadIcons = (scope = document) => Promise.all(
    [...scope.querySelectorAll('svg[data-icon]')].map(async (node) => {
        const text = await fetch(node.getAttribute('data-icon')).then(r => r.text()).catch(() => '');
        const svg = new DOMParser().parseFromString(text, 'image/svg+xml').querySelector('svg');
        if (!svg) return;
        for (const a of node.attributes) if (a.name !== 'data-icon') svg.setAttribute(a.name, a.value);
        node.replaceWith(svg);
    })
);

// Worker

const PythonWorker = (() => {
    // `{ type: 'module' }` lets worker.js `import` from ./worker/*.js without a build step.
    const worker = new Worker('./js/worker.js', { type: 'module' });

    /* Single 'runtime busy' flag gating button + Ctrl+Enter; queued runs fire on `ready` so WASM-load Ctrl+Enter survives. */
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
        ready: ({ ms }) => {
            setBusy(false);
            ok(`Ready${DEV ? ' - Dev' : ''} (Loaded in ${fmt(ms)})`);
            if (pendingRun != null) { const src = pendingRun; pendingRun = null; doRun(src); }
        },
        line: ({ line }) => {
            // `append` adds a text node in O(1); `+= text` rebuilds the whole string each call.
            if (el.term.firstChild) el.term.append('\n');
            el.term.append(line);
        },
        result: ({ out, ms }) => {
            if (out) { el.term.textContent = out; err(`Failed in ${fmt(ms)}`); }
            else ok(`Ran in ${fmt(ms)}`);
            setBusy(false);
        },
        error: ({ message }) => {
            setBusy(false); err('Load failed');
            el.term.textContent = `Could not load WASM.\n\n${message}`;
        },
    };
    worker.onmessage = ({ data }) => onMsg[data.type]?.(data);

    return {
        load: async () => {
            ok('Loading WASM...');
            try {
                const { body, version } = await WASM_FETCH;
                worker.postMessage(
                    { type: 'load', body, baseUrl: location.href, version },
                    [body],
                );
            } catch (e) {
                err('Load failed');
                el.term.textContent = `Could not load WASM.\n\n${e.message}`;
            }
        },
        run: (src) => {
            if (busy) { pendingRun = src; ok('Queued — runtime not ready'); return; }
            doRun(src);
        },
    };
})();

// Init

const Editor = createEditor({ ed: el.ed, ln: el.ln, defaultCode: DEFAULT_CODE, onRun: PythonWorker.run });

el.btn.addEventListener('click', () => PythonWorker.run(Editor.getCode()));
loadIcons();
PythonWorker.load();

fetch(`./${ENTRY_PATH}`, FETCH_OPTS)
    .then(r => r.ok ? r.text() : Promise.reject())
    .then(code => Editor.setCode(code.replace(/\r\n?/g, '\n')))
    .catch(() => console.warn(`${ENTRY_PATH} could not be loaded, using default code.`));
