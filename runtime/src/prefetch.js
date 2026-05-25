/*
BFS over dependency graph: visited specs feed `ctx.fetchedSources`, register code/native, plus sibling `packages.json`. `importsMap` synthesizes a root manifest for bare names.
*/

import { fetchWithLockfile } from './fetch.js';
import { loadNativeModule, nativeTable } from './native.js';
import { dirOf, joinRel, scanStringImports } from './specs.js';

const TD = new TextDecoder();
const TE = new TextEncoder();

/* Hint when a module spec likely can't load: insecure scheme or schemeless URL. Null when it looks fine. */
function schemeHint(spec) {
    if (spec.startsWith('http://')) {
        return `'${spec}' uses http://; browsers block http subresources from an https page `
             + `(mixed content), so the fetch never leaves. Use https:// (an SSL connection).`;
    }
    // No scheme but a dotted first segment looks like a domain, yet the host treats it as a relative path.
    const relative = spec.startsWith('.') || spec.startsWith('/') || spec.includes('://');
    if (!relative && spec.split('/')[0].includes('.')) {
        return `'${spec}' has no scheme, so it resolved as a path on your own origin. `
             + `If it's a URL, prefix it with https://.`;
    }
    return null;
}

export async function bfsPrefetch(rootSrc, exports, lockfile, ctx) {
    const { fetchedSources, knownMissing, importsMap, mainThreadSpecs } = ctx;
    const visited = new Set();
    const queue = [];
    // Module specs that never registered; thrown together at the end so the user sees a clear cause, not the VM's later "not registered".
    const failures = [];

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

        // Main-thread modules are pre-registered by engine.run; the synthetic `mt:<name>` spec has no bytes to fetch.
        if (mainThreadSpecs && mainThreadSpecs.has(spec)) continue;

        // Reuse worker-lifetime cache (synthetic root packages.json from importsMap, or anything previously fetched in an earlier run) instead of re-fetching every Run.
        let bytes;
        if (fetchedSources.has(spec)) {
            bytes = fetchedSources.get(spec);
        } else {
            bytes = await fetchWithLockfile(spec, lockfile, ctx);
            if (!bytes) {
                // packages.json probes are opportunistic 404s; only a real module import is worth flagging.
                if (!spec.endsWith('packages.json')) failures.push(schemeHint(spec) ?? `could not fetch module '${spec}'`);
                continue;
            }
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
            let names, fns;
            try {
                ({ names, fns } = await loadNativeModule(spec, bytes, ctx));
            } catch (e) {
                // Bytes fetched but the module won't load (bad ABI / corrupt wasm); a scheme issue would have failed earlier at fetch, so surface the real error.
                failures.push(`'${spec}' failed to load as a wasm module: ${e?.message ?? e}`);
                continue;
            }
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

    if (failures.length) {
        throw new Error(`could not pre-fetch every imported module:\n  ${failures.join('\n  ')}`);
    }
}
