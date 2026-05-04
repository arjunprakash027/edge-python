---
title: "Imports"
description: "Importing modules in Edge Python: syntax, resolution, and the two module flavors."
---

Edge Python supports `import` and `from <spec> import <names>` with a key difference from CPython: **module resolution happens at compile time, not at runtime**. The VM never learns what a module is — by the time bytecode runs, every import has been flattened into either an inlined function or a direct native call.

## The two flavors

| Flavor | What it is | How it dispatches |
|---|---|---|
| **Code module** | A `.py` file written in Edge Python | Inlined into the importing chunk as a regular function (existing `Call` opcode) |
| **Native module** | Pre-compiled binary (`.wasm`, `.so`, `.dylib`, `.dll`) written in any low-level language | Dispatched through the `CallExtern` opcode to a function pointer |

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

# Plain `import X` — natives only (binds every export at top level)
import math
print(math_pi)   # if `math` exposes pi as a top-level binding
```

## How resolution works

1. The compiler scans your source for every `from <spec> ...` statement.
2. For each spec, it asks the **host's `Resolver`** to materialise the module:
   - `Resolved::Native(bindings)` — list of `(name, function pointer, pure flag)` tuples.
   - `Resolved::Code(source)` — raw `.py` source string for sub-parsing.
3. For natives, the bindings are appended to the chunk's `extern_table` and the alias is registered. Calling the imported name emits `CallExtern idx, argc` instead of the generic `LoadName + Call`.
4. For code modules, the source is parsed into a sub-chunk; each requested function definition is copied into the parent chunk's function table; `MakeFunction + StoreName` make the binding available.

The runtime never fetches anything. The host (browser JS, CLI binary, embedded Rust app) is responsible for bringing the bytes; the compiler accepts them through the `Resolver` trait.

## packages.json

Bare-name imports resolve through an import map declared in your project's `packages.json` (sitting next to the script):

```json
{
  "imports": {
    "utils": "./lib/utils.py",
    "math":  "./vendor/math.wasm"
  }
}
```

After this, `from math import add` resolves to `./vendor/math.wasm` relative to the script's directory.

The `packages.json` file is **optional**. Scripts can use string-form paths directly without any project config — useful for one-off scripts and playground demos.

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

## Sandbox

| Module type | Sandbox |
|---|---|
| Code modules (`.py`) | Full sandbox — code only calls capabilities it imports |
| Native WASM modules (`.wasm`) | Full sandbox — WASM isolates by construction |
| Native dyn-libs (`.so` / `.dylib` / `.dll`) | **No sandbox** — requires `--allow-native` flag at runtime |

By default, only `.py` and `.wasm` modules load. Hosts that need native FFI (CUDA, libssh, custom drivers) opt in explicitly and accept the security tradeoff.

## What doesn't work

- **`from X import *`** — star imports require enumerating module exports, which conflicts with compile-time resolution. Use named imports.
- **Transitive imports inside code modules (v1)** — a `.py` module imported by your script can't itself `import` further modules. v2 will lift this limit.
- **Dynamic imports** — no `__import__`, no `importlib`. The module set is fixed per compilation.
- **Module-level state in code modules** — only top-level `def` definitions are inlined; module-level constants and side effects don't transfer. Helper functions must be self-contained.

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
