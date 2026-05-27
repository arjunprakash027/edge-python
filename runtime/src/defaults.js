/*
Built-in base manifest. Official std packages resolvable by bare name with no user packages.json.
Lowest precedence (user `imports` win) and lazy: an unused default is never fetched.
*/

/* Worker-side std packages (.wasm). Pinned for reproducibility; the lockfile verifies the bytes when integrity is on. */
export const DEFAULT_IMPORTS = {
    json: 'https://std.edgepython.com/json.wasm',
    re: 'https://std.edgepython.com/re.wasm',
    math: 'https://std.edgepython.com/math.wasm',
};

/* Main-thread host libraries (ESM). Pages flattens each `<name>/src/` to `host.edgepython.com/<name>/`. Same lazy + opt-out rules; merged under any user `host` entries. */
export const DEFAULT_HOST = {
    dom: 'https://host.edgepython.com/dom/index.js',
    network: 'https://host.edgepython.com/network/index.js',
    storage: 'https://host.edgepython.com/storage/index.js',
    time: 'https://host.edgepython.com/time/index.js',
};
