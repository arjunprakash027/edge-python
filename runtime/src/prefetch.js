/*
BFS over the dependency graph: visited specs feed `ctx.fetchedSources`, register code/native, plus opportunistic sibling `packages.json`. Bare-name shortcut via `importsMap` synthesizes a root manifest.
*/

import { fetchWithLockfile } from './fetch.js';
import { loadNativeModule, nativeTable } from './native.js';
import { dirOf, joinRel, scanStringImports } from './specs.js';

const TD = new TextDecoder();
const TE = new TextEncoder();

export async function bfsPrefetch(rootSrc, exports, lockfile, ctx) {
    const { fetchedSources, knownMissing, importsMap } = ctx;
    const visited = new Set();
    const queue = [];

    const writeBytes = (bytes) => {
        const ptr = exports.wasm_alloc(Math.max(1, bytes.length));
        new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
        return ptr;
    };
    const enqueueManifestSibling = (forSpec) => {
        const m = dirOf(forSpec) + 'packages.json';
        if (!knownMissing.has(m)) queue.push(m);
    };

    // Seed a virtual root packages.json from `importsMap` so bare-name imports resolve without a physical manifest.
    if (importsMap && Object.keys(importsMap).length > 0) {
        const synthetic = JSON.stringify({ imports: importsMap });
        fetchedSources.set('packages.json', TE.encode(synthetic));
        knownMissing.delete('packages.json');
        // Enqueue the targets so BFS fetches them.
        for (const target of Object.values(importsMap)) queue.push(joinRel('', target));
    }

    for (const q of scanStringImports(rootSrc)) queue.push(q);
    if (!knownMissing.has('packages.json')) queue.push('packages.json');

    while (queue.length) {
        const spec = queue.shift();
        if (visited.has(spec)) continue;
        visited.add(spec);

        // Reuse worker-lifetime cache (synthetic root packages.json from importsMap, or anything previously fetched in an earlier run) instead of re-fetching every Run.
        let bytes;
        if (fetchedSources.has(spec)) {
            bytes = fetchedSources.get(spec);
        } else {
            bytes = await fetchWithLockfile(spec, lockfile, ctx);
            if (!bytes) continue;
            fetchedSources.set(spec, bytes);
        }

        if (spec.endsWith('packages.json')) {
            let parsed;
            try { parsed = JSON.parse(TD.decode(bytes)); }
            catch { continue; }
            const dir = dirOf(spec);
            for (const target of Object.values(parsed.imports || {})) queue.push(joinRel(dir, target));
            if (parsed.extends) {
                const extDir = joinRel(dir, parsed.extends);
                queue.push((extDir.endsWith('/') ? extDir : extDir + '/') + 'packages.json');
            }
            continue;
        }

        if (spec.endsWith('.wasm')) {
            const { names, fns } = await loadNativeModule(spec, bytes, ctx);
            const baseId = nativeTable.length;
            for (const fn of fns) nativeTable.push(fn);

            const specBytes = TE.encode(spec);
            const namesBytes = TE.encode(names.join('\n'));
            exports.register_native_module(
                writeBytes(specBytes), specBytes.length,
                writeBytes(namesBytes), namesBytes.length,
                baseId,
            );
            enqueueManifestSibling(spec);
            continue;
        }

        // .py module
        const specBytes = TE.encode(spec);
        exports.register_code_module(writeBytes(specBytes), specBytes.length, writeBytes(bytes), bytes.length);

        const dir = dirOf(spec);
        for (const q of scanStringImports(TD.decode(bytes))) queue.push(joinRel(dir, q));
        enqueueManifestSibling(spec);
    }
}
