/*
CAS-backed fetch keyed by lockfile hash; else fetch + hash + store. Null on 404 (opportunistic ok), throws on drift.
*/

import { sha256Hex } from './specs.js';

export async function fetchWithLockfile(spec, lockfile, ctx) {
    const { cache, baseUrl, entryDir, knownMissing, integrityActive } = ctx;

    if (integrityActive) {
        const expected = lockfile.get(spec);
        if (expected) {
            const cached = await cache.getBytes(expected);
            if (cached) return new Uint8Array(cached);
        }
    }

    let resp;
    try {
        const absolute = spec.includes('://') || spec.startsWith('/');
        const path = absolute ? spec : entryDir + spec;
        const url = path.includes('://') ? path : new URL(path, baseUrl ?? self.location.href).toString();
        resp = await fetch(url);
    } catch (e) {
        console.warn(`[edge-python] fetch failed for '${spec}':`, e);
        return null;
    }

    if (!resp.ok) {
        if (resp.status === 404 && spec.endsWith('packages.json')) knownMissing.add(spec);
        else console.warn(`[edge-python] ${resp.status} for '${spec}' at ${resp.url}`);
        return null;
    }

    const bytes = new Uint8Array(await resp.arrayBuffer());

    if (integrityActive) {
        const hash = await sha256Hex(bytes);
        const expected = lockfile.get(spec);
        if (expected && expected !== hash) {
            throw new Error(`[edge-python] integrity drift for '${spec}'\n  locked: sha256-${expected}\n  remote: sha256-${hash}`);
        }
        await cache.putBytes(hash, bytes);
        lockfile.set(spec, hash);
    }

    return bytes;
}
