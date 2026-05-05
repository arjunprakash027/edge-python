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

- **Transitive imports inside code modules** — a `.py` module imported by your script can't itself `import` further modules. The sub-parser uses a `NoopResolver`, so module-of-module is intentionally rejected. A future revision will lift this.
- **Dynamic imports** — no `__import__`, no `importlib`. The module set is fixed per compilation.
- **Mutual recursion across top-level defs in code modules** — `def is_even` referencing `is_odd` (defined after it in the same module) still fails: the body chunk records `is_odd_0` while the splicer ends up storing `is_odd_1`, and propagation matches by exact SSA name. Forward references inside the same code module remain a regression pin.

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
