/*
Demo entry point: config, DOM, status helpers, runtime worker shim; editor lives in ./main/editor.js.
*/

import { createEditor } from './main/editor.js';

// Config

const ENTRY_PATH = "runtime/perceptron.py";
const ENTRY_DIR = ENTRY_PATH.slice(0, ENTRY_PATH.lastIndexOf('/') + 1);
const DEV = !['demo.edgepython.com'].includes(location.hostname);
const FETCH_OPTS = DEV ? { cache: 'no-store' } : {};

/* Dev/prod switch for runtime JS: local checkout in dev, edge-python-runtime in prod. Mirrors index.html's Tailwind switch, preserves dev-edit-refresh loop without bundling. */
const RUNTIME_URL = DEV
    ? '../../runtime/src/index.js'
    : 'https://runtime.edgepython.com/js/src/index.js';

const { createWorker } = await import(RUNTIME_URL);

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

let worker = null;
let busy = true;
let pendingSrc = null;
const setBusy = (b) => { busy = b; el.btn.disabled = b; };

async function runPython(src) {
    if (!worker) { pendingSrc = src; ok('Queued: runtime not ready'); return; }
    if (busy) return;
    setBusy(true);
    ok('Running...');
    el.term.textContent = '';
    try {
        const { out, ms } = await worker.run(src, { baseUrl: location.href, entryDir: ENTRY_DIR });
        if (out) { el.term.textContent = out; err(`Failed in ${fmt(ms)}`); }
        else ok(`Ran in ${fmt(ms)}`);
    } catch (e) {
        err('Run failed');
        el.term.textContent = e?.message ?? String(e);
    } finally {
        setBusy(false);
    }
}

// Init

const Editor = createEditor({ ed: el.ed, ln: el.ln, defaultCode: DEFAULT_CODE, onRun: runPython });

el.btn.addEventListener('click', () => runPython(Editor.getCode()));
loadIcons();

fetch(`./${ENTRY_PATH}`, FETCH_OPTS)
    .then(r => r.ok ? r.text() : Promise.reject())
    .then(code => Editor.setCode(code.replace(/\r\n?/g, '\n')))
    .catch(() => console.warn(`${ENTRY_PATH} could not be loaded, using default code.`));

// Async: spin up the worker. UI stays interactive (button disabled until ready).
ok('Loading WASM...');
(async () => {
    try {
        const ver = await fetch('./version.json', { cache: 'no-store' }).then(r => r.json()).catch(() => ({}));
        const bust = ver.v ? `?v=${ver.v}` : '';
        const wasmUrl = `https://cdn.edgepython.com/compiler.wasm${bust}`;

        const t0 = performance.now();
        worker = await createWorker({ wasmUrl, integrity: true, version: ver.v });
        worker.onOutput((line) => {
            // `append` adds a text node in O(1); `+= text` rebuilds the whole string each call.
            if (el.term.firstChild) el.term.append('\n');
            el.term.append(line);
        });
        const ms = performance.now() - t0;
        ok(`Ready${DEV ? ' - Dev' : ''} (Loaded in ${fmt(ms)})`);
        setBusy(false);
        if (pendingSrc != null) { const src = pendingSrc; pendingSrc = null; runPython(src); }
    } catch (e) {
        err('Load failed');
        el.term.textContent = `Could not load WASM.\n\n${e?.message ?? e}`;
    }
})();
