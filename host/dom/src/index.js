/* Factory consumed by `createWorker({ mainThreadModules: { dom } })`. Composes all handler slices over a shared `state`. */

import { makeState } from './main/state.js';
import tree from './main/tree.js';
import style from './main/style.js';
import events from './main/events.js';
import forms from './main/forms.js';
import observers from './main/observers.js';
import animations from './main/animations.js';
import media from './main/media.js';
import platform from './main/platform.js';

export const dom = (ctx) => {
    const state = makeState();
    let errorMsg = null;
    // Surfaces async errors (listener throws, observer callbacks, swallowed promise rejections) to Python. Falls back to console.error if no consumer bound via bind_global_error.
    const emitError = (where, e) => {
        if (errorMsg) ctx.pushEvent(JSON.stringify({
            msg: errorMsg,
            where,
            error: (e && e.message) ? e.message : String(e),
            stack: (e && e.stack) ? e.stack : undefined,
        }));
        else console.error(`[dom:${where}]`, e);
    };
    const ctxPlus = { ...ctx, emitError };
    return Object.assign(
        {},
        tree(state),
        style(state),
        events(state, ctxPlus),
        forms(state, ctxPlus),
        observers(state, ctxPlus),
        animations(state, ctxPlus),
        media(state, ctxPlus),
        platform(state, ctxPlus),
        { bind_global_error: (msg) => { errorMsg = msg; } },
    );
};

export default dom;
