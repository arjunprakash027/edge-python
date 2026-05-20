/* Shared handle tables + helpers. One `makeState()` per `createWorker` so multiple workers don't share `nodes[]`. */

const HANDLE_NONE = -1;

export const makeState = () => {
    const nodes = [];
    const bindings = [];
    const intersectionObservers = [];
    const resizeObservers = [];
    const mutationObservers = [];
    const animations = [];
    const files = [];

    const alloc = (n) => {
        if (n === null || n === undefined) return HANDLE_NONE;
        nodes.push(n);
        return nodes.length - 1;
    };

    const node = (h) => {
        if (h < 0 || h >= nodes.length || nodes[h] === null) {
            throw new Error('invalid DOM node handle: ' + h);
        }
        return nodes[h];
    };

    const allocList = (list) => {
        if (!list || list.length === 0) return '';
        const out = new Array(list.length);
        for (let i = 0; i < list.length; i++) out[i] = alloc(list[i]);
        return out.join(',');
    };

    return { nodes, bindings, intersectionObservers, resizeObservers, mutationObservers, animations, files, alloc, node, allocList };
};
