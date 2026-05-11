import { idbGet, idbPut } from './idb.js';
import { sha256Hex } from './specs.js';

/* 
CAS-backed fetch keyed by lockfile hash; else fetch+hash+store. Null on 404 (opportunistic ok), throws on drift. `ctx` carries baseUrl/entryDir/knownMissing instead of module-level state. 
*/
export async function fetchWithLockfile(spec, lockfile, ctx) {
    const { baseUrl, entryDir, knownMissing } = ctx;
    const expected = lockfile.get(spec);
    if (expected) {
        const cached = await idbGet('cas', expected);
        if (cached) return new Uint8Array(cached);
    }

    let resp;
    try {
        // spec is the parser's canonical name; entryDir is the physical URL prefix. Register uses spec; fetch joins entryDir + spec.
        const absolute = spec.includes('://') || spec.startsWith('/');
        const path = absolute ? spec : entryDir + spec;
        const url = path.includes('://') ? path : new URL(path, baseUrl ?? self.location.href).toString();
        resp = await fetch(url);
    } catch (e) {
        console.warn(`fetch failed for '${spec}':`, e);
        return null;
    }

    if (!resp.ok) {
        if (resp.status === 404 && spec.endsWith('packages.json')) knownMissing.add(spec);
        else console.warn(`${resp.status} for '${spec}' at ${resp.url}`);
        return null;
    }

    const bytes = new Uint8Array(await resp.arrayBuffer());
    const hash = await sha256Hex(bytes);
    if (expected && expected !== hash) {
        throw new Error(`integrity drift for '${spec}'\n  locked: sha256-${expected}\n  remote: sha256-${hash}`);
    }
    await idbPut('cas', hash, bytes);
    lockfile.set(spec, hash);
    return bytes;
}
