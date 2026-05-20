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
    return Object.assign(
        {},
        tree(state),
        style(state),
        events(state, ctx),
        forms(state, ctx),
        observers(state, ctx),
        animations(state, ctx),
        media(state),
        platform(state),
    );
};

export default dom;
