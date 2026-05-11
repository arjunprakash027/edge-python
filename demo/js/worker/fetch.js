import { idbGet, idbPut } from './idb.js';
import { sha256Hex } from './specs.js';

/* Serve `spec` from CAS if the lockfile knows its hash, else fetch the
   canonicalized URL, hash it, and store. Returns the bytes (Uint8Array) or
   null on 404 — null is fine for opportunistic packages.json siblings.
   Drift detection: if the lockfile entry mismatches the freshly-computed
   hash, throw so the user notices a CDN change.
   `ctx` provides { baseUrl, entryDir, knownMissing } — passed in by the
   caller (BFS / handlers) instead of being module-level state. */
export async function fetchWithLockfile(spec, lockfile, ctx) {
    const { baseUrl, entryDir, knownMissing } = ctx;
    const expected = lockfile.get(spec);
    if (expected) {
        const cached = await idbGet('cas', expected);
        if (cached) return new Uint8Array(cached);
    }
    let resp;
    try {
        // spec is the canonical name the parser sees (e.g. './lib/format.py');
        // entryDir is the URL prefix where the project physically lives
        // (e.g. 'runtime/'). Keep them separate: register/lookup uses spec,
        // fetch uses entryDir + spec.
        const path = (spec.includes('://') || spec.startsWith('/')) ? spec : entryDir + spec;
        const url = path.includes('://')
            ? path
            : new URL(path, baseUrl ?? self.location.href).toString();
        resp = await fetch(url);
    } catch (e) {
        console.warn(`fetch failed for '${spec}':`, e);
        return null;
    }
    if (!resp.ok) {
        if (resp.status === 404 && spec.endsWith('packages.json')) {
            knownMissing.add(spec);
        } else {
            console.warn(`${resp.status} for '${spec}' at ${resp.url}`);
        }
        return null;
    }
    const bytes = new Uint8Array(await resp.arrayBuffer());
    const hash = await sha256Hex(bytes);
    if (expected && expected !== hash) {
        throw new Error(
            `integrity drift for '${spec}'\n  locked: sha256-${expected}\n  remote: sha256-${hash}`
        );
    }
    await idbPut('cas', hash, bytes);
    lockfile.set(spec, hash);
    return bytes;
}
