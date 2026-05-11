import { fetchWithLockfile } from './fetch.js';
import { instantiateNativeModule, nativeTable } from './native.js';
import { dirOf, joinRel, scanStringImports } from './specs.js';

const TD = new TextDecoder();
const TE = new TextEncoder();

/* BFS the dependency graph: visited specs feed `fetchedSources`, register code/native, plus opportunistic sibling `packages.json`. */
export async function bfsPrefetch(rootSrc, exports, lockfile, ctx) {
    const { fetchedSources, knownMissing } = ctx;
    const visited = new Set();
    const queue = [];

    // Alloc compiler memory, copy `bytes`, return pointer; fresh `Uint8Array` view per call (wasm_alloc may grow memory).
    const writeBytes = (bytes) => {
        const ptr = exports.wasm_alloc(Math.max(1, bytes.length));
        new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
        return ptr;
    };
    // Opportunistic enqueue of `<dir>/packages.json` for a just-registered spec.
    const enqueueManifestSibling = (forSpec) => {
        const m = dirOf(forSpec) + 'packages.json';
        if (!knownMissing.has(m)) queue.push(m);
    };

    // Root script's quoted imports anchor BFS in canonical form (no entryDir); fetchWithLockfile applies the URL prefix later.
    for (const q of scanStringImports(rootSrc)) queue.push(q);
    if (!knownMissing.has('packages.json')) queue.push('packages.json');

    while (queue.length) {
        const spec = queue.shift();
        if (visited.has(spec)) continue;
        visited.add(spec);

        const bytes = await fetchWithLockfile(spec, lockfile, ctx);
        if (!bytes) continue;
        fetchedSources.set(spec, bytes);

        if (spec.endsWith('packages.json')) {
            let parsed;
            try { parsed = JSON.parse(TD.decode(bytes)); }
            catch { continue; } // bad JSON surfaces at compile time via the bridge
            const dir = dirOf(spec);
            for (const target of Object.values(parsed.imports || {})) queue.push(joinRel(dir, target));
            if (parsed.extends) {
                const extDir = joinRel(dir, parsed.extends);
                queue.push((extDir.endsWith('/') ? extDir : extDir + '/') + 'packages.json');
            }
            continue;
        }

        if (spec.endsWith('.wasm')) {
            // Native module: instantiate via the universal ABI, register exports so the parser sees `from "<spec>" import name`.
            const { names, fns } = await instantiateNativeModule(spec, bytes, exports);
            const baseId = nativeTable.length;
            for (const fn of fns) nativeTable.push(fn);

            const specBytes = TE.encode(spec);
            const namesBytes = TE.encode(names.join('\n'));
            exports.register_native_module(
                writeBytes(specBytes), specBytes.length,
                writeBytes(namesBytes), namesBytes.length,
                baseId,
            );
            // Native modules don't carry transitive Python imports, but they CAN carry sibling packages.json (e.g. bundled companions).
            enqueueManifestSibling(spec);
            continue;
        }

        // .py module: hand to the parser via REGISTRY, then expand its own quoted imports + sibling packages.json.
        const specBytes = TE.encode(spec);
        exports.register_code_module(writeBytes(specBytes), specBytes.length, writeBytes(bytes), bytes.length);

        const dir = dirOf(spec);
        for (const q of scanStringImports(TD.decode(bytes))) queue.push(joinRel(dir, q));
        enqueueManifestSibling(spec);
    }
}
