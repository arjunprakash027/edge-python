/*
Public entry. `createWorker(opts)` spawns a Web Worker around `engine.js` and returns a proxy whose methods round-trip via postMessage. See README for options.
*/

export async function createWorker(opts) {
    // Chromium blocks `new Worker(crossOriginUrl)` even with `type:'module'`; cross-origin runtimes need the Blob bootstrap below.
    const workerUrl = new URL('../worker/worker.js', import.meta.url);
    const sameOrigin = workerUrl.origin === self.location.origin;
    const worker = sameOrigin
        ? new Worker(workerUrl, { type: 'module' })
        : spawnCrossOriginWorker(workerUrl.href);

    let reqIdCounter = 0;
    const pending = new Map();
    let outputHandler = null;

    /* Fire a string into the running script's `receive()` queue. Defined early so main-thread module factories can capture it. */
    const pushEvent = (message) => worker.postMessage({ type: 'push-event', message: String(message) });

    /* Resolve each `mainThreadModules[name]` (factory or object) into a flat handler map keyed `module:name`. */
    const mainThreadHandlers = {};
    const manifests = [];
    for (const [modName, src] of Object.entries(opts?.mainThreadModules || {})) {
        const handlers = typeof src === 'function' ? src({ pushEvent }) : src;
        manifests.push({ name: modName, exports: Object.keys(handlers) });
        for (const [fnName, handler] of Object.entries(handlers)) {
            mainThreadHandlers[`${modName}:${fnName}`] = handler;
        }
    }

    worker.onmessage = async ({ data }) => {
        if (data.type === 'line') {
            if (outputHandler) outputHandler(data.text);
            return;
        }
        if (data.type === 'host-call') {
            const handler = mainThreadHandlers[`${data.module}:${data.name}`];
            if (!handler) {
                worker.postMessage({ type: 'host-call-response', reqId: data.reqId, error: `no main-thread handler for '${data.module}.${data.name}'` });
                return;
            }
            try {
                const value = await handler(...data.args);
                worker.postMessage({ type: 'host-call-response', reqId: data.reqId, value });
            } catch (e) {
                worker.postMessage({ type: 'host-call-response', reqId: data.reqId, error: e?.message ?? String(e) });
            }
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

    /* Strip mainThreadModules before crossing postMessage: factories/handlers aren't structured-cloneable. Send only the manifests. */
    const { mainThreadModules: _drop, ...workerOpts } = opts || {};
    const ready = await send('load', { opts: workerOpts, mainThreadManifests: manifests });

    /* Browser bridges fire `CustomEvent("edge-python-event")` on window; route the detail to the Worker. */
    if (typeof window !== 'undefined') {
        window.addEventListener('edge-python-event', (e) => {
            if (typeof e.detail === 'string') pushEvent(e.detail);
        });
    }

    return {
        integrityActive: ready.integrityActive,
        loadMs: ready.loadMs,

        run: (src, runOpts = {}) => send('run', { src, ...runOpts }),
        reset: () => send('reset'),
        clearCache: () => send('clearCache'),
        pushEvent,

        onOutput(handler) { outputHandler = handler; },

        dispose() {
            worker.postMessage({ type: 'dispose' });
            worker.terminate();
            for (const cb of pending.values()) cb.reject(new Error('worker disposed'));
            pending.clear();
        },
    };
}

/* Runs inside the worker. Buffers messages arriving during the dynamic import — without this, the event loop dispatches `postMessage('load')` before worker.js installs `self.onmessage` and the first message is lost. */
function crossOriginBootstrap(workerUrl) {
    const buffered = [];
    const enqueue = (event) => buffered.push(event.data);
    self.addEventListener('message', enqueue);
    import(workerUrl).then(() => {
        self.removeEventListener('message', enqueue);
        for (const data of buffered) self.dispatchEvent(new MessageEvent('message', { data }));
    }, (err) => {
        self.postMessage({ type: 'error', message: 'worker bootstrap failed: ' + (err && err.message || err) });
    });
}

/* Blob URL inherits the page's origin -> sidesteps Chromium's cross-origin block; the imported module then loads under CORS (Cloudflare Pages OK by default). `Function.toString` keeps the bootstrap as real JS in source. */
function spawnCrossOriginWorker(workerUrl) {
    const source = `(${crossOriginBootstrap.toString()})(${JSON.stringify(workerUrl)});`;
    const blob = new Blob([source], { type: 'application/javascript' });
    const blobUrl = URL.createObjectURL(blob);
    const worker = new Worker(blobUrl, { type: 'module' });
    // Defer revoke a tick; some browsers race it against the module fetch.
    setTimeout(() => URL.revokeObjectURL(blobUrl), 0);
    return worker;
}
