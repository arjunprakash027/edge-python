---
title: "Imports"
description: "Importing modules in Edge Python: syntax, resolution, and the two module flavors."
---

Edge Python supports `import`, `from <spec> import <names>`, and `from <spec> import *` with a key difference from CPython: **module resolution happens at compile time, not at runtime**. The VM never learns what a module is — by the time bytecode runs, every import has been flattened into a sequence of bytecode that materialises the module's exports.

## The two flavors

| Flavor | What it is | How it dispatches |
|---|---|---|
| **Code module** | A `.py` file written in Edge Python | The module's top level is spliced into the importing chunk; requested names are bound (or wrapped in a `HeapObj::Module` for `import X`). |
| **Native module** | Pre-compiled binary (`.wasm`, `.so`, `.dylib`, `.dll`) written in any low-level language | Dispatched through the `CallExtern` opcode (named import) or via a `HeapObj::Module` carrying `HeapObj::Extern` callables (`import X`). |

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

The runtime never fetches anything. The host (browser JS, CLI binary, embedded Rust app) is responsible for bringing the bytes; the compiler accepts them through the `Resolver` trait.

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

## Running with the CLI

The `edge` CLI wires this end-to-end:

```bash
# Project layout
my-app/
├── packages.json
├── main.py
├── lib/
│   └── utils.py
└── vendor/
    └── math.wasm        # built from your edge-sdk Rust crate

# Run it
edge main.py
```

Behind the scenes, the CLI:
1. Reads `packages.json` from the script's directory (if present)
2. Walks the script's imports during compilation
3. For each import, the default resolver:
   - Reads `.py` files and inlines their `def`s
   - Loads `.wasm` files via wasmtime and registers their exports
4. Runs the resulting bytecode

No Rust to write. No SDK on the consumer side. Just a script and an import map.

## Cold start and reproducibility

| Scenario | Behavior |
|---|---|
| CLI script with local files | Resolution is direct disk reads — instant |
| CLI script with `https://` URLs | Fetched synchronously at compile time via `ureq` + rustls. No cache yet. |
| Browser playground | The host (JS) is responsible for fetching URLs and feeding bytes to the resolver |
| Production deploy | `edge compile` (planned) will seal all imports into a single `.epy` artifact |

The CLI now supports `http(s)://` URLs end-to-end:

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

Caveats today:
- **No cache layer** — every compile re-fetches. Mirror to local files for production.
- **No integrity verification** — `#sha256-...` URL fragments are documented but not yet enforced.
- **No lockfile** — planned alongside `edge compile`.
- **Relative paths inside remote modules resolve against the entry script's directory, not the URL.** Use absolute URLs for transitive imports in remote modules until this is fixed.

## Sandbox

| Module type | Sandbox |
|---|---|
| Code modules (`.py`) | Full sandbox — code only calls capabilities it imports |
| Native WASM modules (`.wasm`) | Full sandbox — WASM isolates by construction |
| Native dyn-libs (`.so` / `.dylib` / `.dll`) | **No sandbox** — requires `--allow-native` flag at runtime |

By default, only `.py` and `.wasm` modules load. Hosts that need native FFI (CUDA, libssh, custom drivers) opt in explicitly and accept the security tradeoff.

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
