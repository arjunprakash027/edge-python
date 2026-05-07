---
title: "Imports"
description: "Importing modules in Edge Python: syntax, resolution, and the two module flavors."
---

Edge Python supports `import`, `from <spec> import <names>`, and `from <spec> import *` with a key difference from CPython: **module resolution happens at compile time, not at runtime**. The VM never learns what a module is — by the time bytecode runs, every import has been flattened into a sequence of bytecode that materialises the module's exports.

## The two flavors

| Flavor | What it is | How it dispatches |
|---|---|---|
| **Code module** | A `.py` file written in Edge Python | The module's top level is spliced into the importing chunk; requested names are bound (or wrapped in a `HeapObj::Module` for `import X`). |
| **Native module** | Pre-compiled `.wasm` binary written in any language that targets WebAssembly | Dispatched through the `CallExtern` opcode (named import) or via a `HeapObj::Module` carrying `HeapObj::Extern` callables (`import X`). |

The same `import` syntax covers both. The host's resolver decides which flavor a given spec maps to.

## Syntax

```python
# Bare-name imports — resolved via the host's import map / packages.json
from json import dumps, loads
from utils import normalize

# String-form imports — explicit URLs or local paths, no map needed
from "./lib/helpers.py" import slugify
from "https://std.edgepython.com/json@1.0.wasm" import dumps
from "https://github.com/foo/lib@v1.0/" import handler

# Aliases work as in CPython
from math import sqrt as root
from utils import normalize as n

# Plain `import X` — binds the module under its name; access exports via `.`
import math
print(math.sqrt(2.0))

# Star imports — every export becomes a flat name in scope
from utils import *
print(slugify("Hello world"))
```

## How resolution works

1. The compiler scans your source for every `from <spec> ...` statement.
2. For each spec, it asks the **host's `Resolver`** to materialise the module:
   - `Resolved::Native(bindings)` — list of `(name, function pointer, pure flag)` tuples.
   - `Resolved::Code(source)` — raw `.py` source string for sub-parsing.
3. For natives, the bindings are appended to the chunk's `extern_table`. Named imports register the alias so the call site can emit a direct `CallExtern idx, argc`; `import X` additionally emits `LoadExtern + LoadConst + BuildModule` to wrap the bindings in a `HeapObj::Module` value.
4. For code modules, the source is parsed into a sub-chunk and the **entire top level** is spliced into the parent chunk (constants, defs, classes, branches, with operand indices remapped). Named imports then either expose the bound parent slot directly or rebind it under an alias. `import X` reads each top-level binding via `LoadName` and folds them into a `HeapObj::Module`.

Each module's top-level names live in their own namespace; only the exports you explicitly request become visible in the importer's scope. Helpers and constants stay private to their defining module, even when two modules use the same internal name. Inside an imported module, `__name__` is rebound to the module's spec for the duration of the splice, so the canonical `if __name__ == "__main__":` guard skips when the module is imported.

The runtime never fetches anything. The host (browser JS, WASI runtime, embedded Rust app) is responsible for bringing the bytes; the compiler accepts them through the `Resolver` trait.

## packages.json

Bare-name imports resolve through an import map declared in your project's `packages.json` (sitting next to the entry script):

```json
{
  "imports": {
    "utils": "./lib/utils.py",
    "math":  "./vendor/math.wasm"
  }
}
```

After this, `from math import add` resolves to `./vendor/math.wasm` relative to the entry script's directory.

The `packages.json` file is **optional**. Scripts can use string-form paths directly without any project config — useful for one-off scripts and playground demos.

### Entry-point semantics

Only the **entry script's** `packages.json` is read. Every transitively-imported module — even a module sitting in a deeper directory with its own `packages.json` — resolves through the entry's import map. This mirrors how `Cargo.toml` at the workspace root drives every dependency, or how the root `package.json` is the only one consulted in a Node script.

Concretely:
- **Bare-name imports** (`from utils import x`) anywhere in the dependency graph hit the entry's `packages.json`.
- **Quoted relative paths** (`from "./helpers.py" import f`) resolve against the directory of the importing file — so a transitively-imported `lib/a.py` doing `from "./b.py" import g` correctly finds `lib/b.py`.
- A `packages.json` next to a sub-module is silently ignored. Configuration is centralized.

### Diamond imports and cycles

When multiple paths import the same module, it's fetched and parsed once: the resolver caches each canonical spec. Direct or indirect cycles (`a.py` imports `b.py`, `b.py` imports `a.py`) surface as parse-time `circular import` diagnostics instead of looping the splicer.

## Host responsibilities

Edge Python's compiler is a WebAssembly module. Fetching bytes — from disk, from `https://`, from your build artifact pipeline — is the host's job. The browser shim ([`demo/edge.js`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/demo/edge.js)) does it via `fetch()`. WASI hosts use their runtime's filesystem and network APIs. Embedders pre-stage modules and feed them to the `Resolver`.

| Scenario | Who fetches |
|---|---|
| Browser playground | `edge.js` shim — pre-fetches every spec the script imports, hands bytes to `Resolver` |
| WASI runtime | Host program reads from disk / network using `wasi_snapshot_preview1` |
| Embedded Rust app | Caller pre-stages a `HashMap<String, Resolved>` and constructs a custom `Resolver` |
| Production deploy | `edge compile` (planned) will seal all imports into a single `.epy` artifact |

URL imports work as long as your host supports them:

```python
from "https://example.com/json.wasm" import dumps
```

Or via packages.json alias:

```json
{ "imports": { "json": "https://example.com/json.wasm" } }
```

```python
from json import dumps
```

### Integrity verification

Append `#sha256-<64 hex chars>` to any URL spec to require a content match:

```python
from "https://example.com/json.wasm#sha256-deadbeef0123456789abcdef0123456789abcdef0123456789abcdef01234567" import dumps
```

The compiler asks the host for the raw bytes (via the `Resolver::fetch_bytes` trait method), computes the SHA-256, and refuses to compile if the hash doesn't match — with a diagnostic that surfaces both expected and computed digests:

```text
error: integrity check failed for 'https://example.com/json.wasm'
  expected sha256-deadbeef0123456789abcdef0123456789abcdef0123456789abcdef01234567
  got      sha256-feedface9876543210fedcba9876543210fedcba9876543210fedcba98765432
```

Verification lives in the compiler itself, not the host — so any host (browser shim, WASI runtime, embedder) inherits the guarantee uniformly. Hosts that don't implement `fetch_bytes` surface a clean "not supported" error instead of silently bypassing the check, so a script asking for integrity never runs unverified.

Only `sha256` is supported today. A spec with any other prefix (`md5-...`, `sha384-...`) fails with `unrecognized integrity fragment`.

## Caching

The reference browser shim (`demo/edge.js`) keeps every fetched module's raw bytes in an in-memory `Map<spec, Uint8Array>` for the duration of a single `run()`. Two consequences:

- **Same URL twice in one script fetches once.** The shim deduplicates specs (`new Set([...stringSpecs, ...bareSpecs])`) and the per-run map persists fetched bytes through both `register_*_module` and any `#sha256-...` integrity check the compiler triggers — zero extra HTTP round-trips for verification.
- **The map clears between runs.** Each `run()` starts with `fetchedBytes.clear()`, so the same script run twice re-fetches. Persistent caching across runs is the host's responsibility — wrap `fetch()` with a `Cache` API service worker, an IndexedDB layer, or mirror to local files for production.

WASI hosts and Rust embedders make their own caching choices: the `Resolver` trait sees only `(spec → Resolved)` and `(spec → bytes for integrity)`; how those are sourced and how long they're held is the host's contract.

Lockfile-style integrity (record every URL → hash on first build, verify against the file on subsequent builds) is planned alongside `edge compile`.

## Sandbox

| Module type | Sandbox |
|---|---|
| Code modules (`.py`) | Full sandbox — code only calls capabilities it imports |
| Native modules (`.wasm`) | Full sandbox — WASM isolates by construction |

Edge Python runs as a WebAssembly module, so the entire execution environment is sandboxed by the WASM runtime. There is no native dyn-lib loader and no `dlopen` path — that would defeat the sandbox guarantee that's the whole point.

## What doesn't work

- **Dynamic imports** — no `__import__`, no `importlib`. The module set is fixed per compilation.

## Errors

Resolution errors surface as parse-time diagnostics with the import statement's source position:

```text
error: module 'unknown' not found (no resolver configured)
   --> main.py:1:6
    |
  1 | from unknown import f
    |      ^^^^^^^

error: module 'json' has no export 'badname'
   --> main.py:2:6
    |
  2 | from json import badname
    |      ^^^^
```

Runtime errors from native bindings (e.g., `add()` with non-int args) propagate normally as `VmErr`.
