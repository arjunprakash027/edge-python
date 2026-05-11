import { fetchWithLockfile } from './fetch.js';
import { instantiateNativeModule, nativeTable } from './native.js';
import { dirOf, joinRel, scanStringImports } from './specs.js';

/* BFS over the dependency graph. Each visited spec contributes:
     • its bytes to ctx.fetchedSources (so host_fetch_bytes can serve them),
     • either a register_code_module call (.py) or a recursive queue
       expansion (packages.json),
     • a queued sibling packages.json next to .py/.wasm files (opportunistic —
       a 404 ends that arm of the search silently).

   The queue holds canonical specs (matching what the bridge resolver will
   look up); transitive relative imports are joined to their importer's dir
   before queuing. */
export async function bfsPrefetch(rootSrc, exports, lockfile, ctx) {
    const { fetchedSources, knownMissing } = ctx;
    const decoder = new TextDecoder();
    const visited = new Set();
    const queue = [];
    // Root script's quoted imports anchor BFS in their canonical form
    // (no entryDir prefix — the parser sees them that way too). The URL
    // prefix for physical fetch is applied inside fetchWithLockfile.
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
            try { parsed = JSON.parse(decoder.decode(bytes)); }
            catch { continue; }   // bad JSON surfaces at compile time via the bridge
            const dir = dirOf(spec);
            for (const target of Object.values(parsed.imports || {})) {
                queue.push(joinRel(dir, target));
            }
            if (parsed.extends) {
                const extDir = joinRel(dir, parsed.extends);
                const extDirSlash = extDir.endsWith('/') ? extDir : extDir + '/';
                queue.push(extDirSlash + 'packages.json');
            }
            continue;
        }

        if (spec.endsWith('.wasm')) {
            // Native module: instantiate against the universal ABI, then
            // register its callable exports with compiler.wasm so the
            // EdgePython parser sees them as `from "<spec>" import name`.
            const { names, fns } = await instantiateNativeModule(spec, bytes, exports);
            const baseId = nativeTable.length;
            for (const fn of fns) nativeTable.push(fn);

            const specBytes = new TextEncoder().encode(spec);
            const namesBytes = new TextEncoder().encode(names.join('\n'));
            const sp = exports.wasm_alloc(specBytes.length);
            const np = exports.wasm_alloc(Math.max(1, namesBytes.length));
            new Uint8Array(exports.memory.buffer, sp, specBytes.length).set(specBytes);
            new Uint8Array(exports.memory.buffer, np, namesBytes.length).set(namesBytes);
            exports.register_native_module(sp, specBytes.length, np, namesBytes.length, baseId);
            // Native modules don't carry transitive Python imports, but
            // they CAN carry sibling packages.json (e.g. for bundled
            // companions). Try opportunistically.
            const wasmManifest = dirOf(spec) + 'packages.json';
            if (!knownMissing.has(wasmManifest)) queue.push(wasmManifest);
            continue;
        }

        // .py module: hand to the parser via REGISTRY, then expand its own
        // quoted imports + sibling packages.json.
        const specBytes = new TextEncoder().encode(spec);
        const sp = exports.wasm_alloc(specBytes.length);
        const cp = exports.wasm_alloc(bytes.length);
        new Uint8Array(exports.memory.buffer, sp, specBytes.length).set(specBytes);
        new Uint8Array(exports.memory.buffer, cp, bytes.length).set(bytes);
        exports.register_code_module(sp, specBytes.length, cp, bytes.length);

        const dir = dirOf(spec);
        for (const q of scanStringImports(decoder.decode(bytes))) {
            queue.push(joinRel(dir, q));
        }
        const pyManifest = dir + 'packages.json';
        if (!knownMissing.has(pyManifest)) queue.push(pyManifest);
    }
}
