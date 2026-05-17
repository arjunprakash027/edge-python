/*
Public entry. `createWorker(opts)` spawns a Web Worker around `engine.js` and returns a proxy whose methods round-trip via postMessage. See README for options.
*/

export async function createWorker(opts) {
    // `new Worker(crossOriginUrl)` is blocked even with `type: 'module'` (Chromium does not implement the cross-origin path of the spec). Bootstrap from a same-origin Blob whose body dynamic-imports the real module; module workers ARE allowed to load cross-origin module scripts when the CDN sends permissive CORS (Cloudflare Pages does by default).
    const workerUrl = new URL('../worker/worker.js', import.meta.url).href;
    const blob = new Blob([`import(${JSON.stringify(workerUrl)});`], { type: 'application/javascript' });
    const blobUrl = URL.createObjectURL(blob);
    const worker = new Worker(blobUrl, { type: 'module' });
    URL.revokeObjectURL(blobUrl);

    let reqIdCounter = 0;
    const pending = new Map();
    let outputHandler = null;

    worker.onmessage = ({ data }) => {
        if (data.type === 'line') {
            if (outputHandler) outputHandler(data.text);
            return;
        }
        const cb = pending.get(data.reqId);
        if (!cb) return;
        pending.delete(data.reqId);
        if (data.type === 'error') cb.reject(new Error(data.message));
        else cb.resolve(data.result);
    };

    worker.onerror = (e) => {
        const err = new Error(e.message || 'worker error');
        for (const cb of pending.values()) cb.reject(err);
        pending.clear();
    };

    const send = (type, payload = {}) => new Promise((resolve, reject) => {
        const reqId = ++reqIdCounter;
        pending.set(reqId, { resolve, reject });
        worker.postMessage({ type, reqId, ...payload });
    });

    const ready = await send('load', { opts });

    return {
        integrityActive: ready.integrityActive,
        loadMs: ready.loadMs,

        run: (src, runOpts = {}) => send('run', { src, ...runOpts }),
        reset: () => send('reset'),
        clearCache: () => send('clearCache'),

        onOutput(handler) { outputHandler = handler; },

        dispose() {
            worker.postMessage({ type: 'dispose' });
            worker.terminate();
            for (const cb of pending.values()) cb.reject(new Error('worker disposed'));
            pending.clear();
        },
    };
}
