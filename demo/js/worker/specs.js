/* Spec / URL helpers. Kept in lockstep with the Rust bridge's
   modules::packages::manifest helpers so a transitively-imported module's
   relative path canonicalizes identically on both sides. */

export const sha256Hex = async (bytes) => {
    const digest = await crypto.subtle.digest('SHA-256', bytes);
    return [...new Uint8Array(digest)].map(b => b.toString(16).padStart(2, '0')).join('');
};

export const dirOf = (spec) => {
    const i = spec.lastIndexOf('/');
    return i === -1 ? '' : spec.slice(0, i + 1);
};

export const parentDir = (dir) => {
    if (dir === '') return null;
    const trimmed = dir.endsWith('/') ? dir.slice(0, -1) : dir;
    const sch = trimmed.indexOf('://');
    if (sch !== -1 && !trimmed.slice(sch + 3).includes('/')) return null;
    const i = trimmed.lastIndexOf('/');
    if (i === -1) return '';
    return trimmed.slice(0, i + 1);
};

export const joinRel = (base, target) => {
    if (target.includes('://') || target.startsWith('/')) return target;
    if (base.includes('://')) return new URL(target, base).toString();
    let b = base, t = target;
    while (t.startsWith('../')) {
        const p = parentDir(b); b = p == null ? '' : p;
        t = t.slice(3);
    }
    if (t === '..') { const p = parentDir(b); return p == null ? '' : p; }
    if (t === '.' || t === '') return b;
    if (b !== '') {
        while (t.startsWith('./')) t = t.slice(2);
        if (!b.endsWith('/')) b += '/';
    }
    return b + t;
};

/* Match the Rust bridge's `scan_string_imports` — collect every quoted spec
   appearing after a `from`. Used to drive BFS pre-fetch without involving
   the WASM compiler for transitive specs. */
export const scanStringImports = (src) => {
    const out = [];
    for (const line of src.split('\n')) {
        const t = line.trimStart();
        if (!t.startsWith('from ')) continue;
        const rest = t.slice(5).trimStart();
        if (rest[0] !== '"') continue;
        const end = rest.indexOf('"', 1);
        if (end > 1) out.push(rest.slice(1, end));
    }
    return out;
};
