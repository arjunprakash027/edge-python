/*
Public entry. `createWorker(opts)` spawns a Web Worker around `engine.js` and returns a proxy whose methods round-trip via postMessage. See README for options.
*/

export async function createWorker(opts) {
    const workerUrl = new URL('../worker/worker.js', import.meta.url);
    const worker = new Worker(workerUrl, { type: 'module' });

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
