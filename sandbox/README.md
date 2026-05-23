# Sandbox

Shared browser shell for every stdpkg. `index.html` boots the upstream Edge Python runtime worker, builds the `imports` map from the `?packages=<name>,...` query, and exposes `window.runEdgePython(src)` for the agnostic Deno test driver to call.

## Manual exploration

Build a package first so its artifact is on disk:

```bash
( cd json && cargo build --release --target wasm32-unknown-unknown )
```

Serve the repo root and open the sandbox pointed at that package:

```bash
python3 -m http.server 8000
# -> http://localhost:8000/sandbox/?packages=json
```

Multiple packages: `?packages=json,re`. Edit the textarea and press Run.

## Automated tests

`run.test.js` next to `index.html` is the agnostic Playwright driver, it discovers stdpkgs by walking the repo root for `<name>/<name>.json` corpora and runs each through this sandbox. From the repo root:

```bash
deno test --allow-all sandbox/
```
