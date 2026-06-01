/* Factory consumed by `createWorker({ mainThreadModules: { network } })`. Composes all handler slices over a shared `state`. */

import { makeState } from './main/state.js';
import http from './main/http.js';
import ws from './main/ws.js';
import sse from './main/sse.js';

export const network = (ctx) => {
    const state = makeState();
    return Object.assign(
        {},
        http(state),
        ws(state, ctx),
        sse(state, ctx),
    );
};

export default network;
