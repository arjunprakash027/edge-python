/* Factory consumed by `createWorker({ mainThreadModules: { dom } })`. Composes all handler slices over a shared `state`. */

import { makeState } from './state.js';
import tree from './tree.js';
import style from './style.js';
import events from './events.js';
import forms from './forms.js';
import observers from './observers.js';
import animations from './animations.js';
import media from './media.js';
import platform from './platform.js';

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
