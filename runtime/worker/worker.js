/*
Web Worker entry. Receives postMessage requests from `createWorker`, dispatches to the engine, posts responses.
*/

import * as engine from '../src/engine.js';

const onLine = (text) => self.postMessage({ type: 'line', text });

const handlers = {
    load: (data) => engine.load(data.opts),
    run: (data) => engine.run({ ...data, onLine }),
    reset: () => engine.reset(),
    clearCache: () => engine.clearCache(),
    dispose: () => { engine.dispose(); self.close(); },
};

self.onmessage = async ({ data }) => {
    const handler = handlers[data.type];
    if (!handler) {
        self.postMessage({ type: 'error', reqId: data.reqId, message: `unknown message type: ${data.type}` });
        return;
    }
    try {
        const result = await handler(data);
        if (data.reqId != null) {
            self.postMessage({ type: 'response', reqId: data.reqId, result });
        }
    } catch (e) {
        self.postMessage({ type: 'error', reqId: data.reqId, message: e?.message ?? String(e) });
    }
};
