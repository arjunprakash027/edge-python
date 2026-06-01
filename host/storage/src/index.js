/* Factory consumed by `createWorker({ mainThreadModules: { storage } })`. Composes all handler slices over a shared `state`. */

import { makeState } from './main/state.js';
import kv from './main/kv.js';
import idb from './main/idb.js';

export const storage = () => {
    const state = makeState();
    return Object.assign(
        {},
        kv(),
        idb(state),
    );
};

export default storage;
