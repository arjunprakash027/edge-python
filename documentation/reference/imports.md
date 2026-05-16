---
title: "Imports"
description: "Importing modules in Edge Python: syntax, resolution, and the two module flavors."
---

Edge Python supports `import`, `from <spec> import <names>`, and `from <spec> import *`. **Module resolution happens at compile time, not at runtime**: the VM never learns what a module is; by the time bytecode runs, every import has been flattened into a sequence of bytecode that materialises the module's exports.

## The two flavors

| Flavor | What it is | How it dispatches |
|---|---|---|
| **Code module** | A `.py` file written in Edge Python | The module's top level runs once at VM init in its own slot frame; the resulting bindings live in a `HeapObj::Module` value shared by every importer via `OpCode::LoadModule`. |
| **Native module** | A `.wasm` binary following the [WASM module ABI](/reference/wasm-abi) (loaded by URL or path) **or** Rust closures provided in-process by an embedder via the `Resolver` trait. See [Writing modules](/reference/writing-modules). | Dispatched through the `CallExtern` opcode (named import) or via a `HeapObj::Module` carrying `HeapObj::Extern` callables (`import X`). |

The same `import` syntax covers both. The host's resolver decides which flavor a given spec maps to.

Native modules also cover **host capabilities**: bindings the embedder ships as part of its runtime (DOM in a browser distribution, FS in a WASI distribution) — same dispatch path as ordinary in-process Rust bindings, just shipped together with the host runtime instead of as a separate crate. See [Writing modules / Path C](/reference/writing-modules#path-c-host-capability).

## Syntax

```python
# Bare-name imports — resolved via the host's import map / packages.json
from json import dumps, loads
from utils import normalize

# String-form imports — explicit URLs or local paths, no map needed
from "./lib/helpers.py" import slugify
from "https://example.com/utils.py" import normalize
from "https://example.com/math.wasm" import add

# Aliases via `as`
from math import sqrt as root
from utils import normalize as n

# Parenthesized name lists — multi-line, optional trailing comma
from utils import (
    slugify,
    normalize as n,
    titlecase,
)

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
   * `Resolved::Native { bindings, canonical }`, list of `(name, function pointer, pure flag)` tuples plus the resolver's authoritative spec.
   * `Resolved::Code { src, canonical }`; raw `.py` source plus the canonical spec.
3. For natives, the bindings are appended to the chunk's `extern_table`. Named imports register the alias so the call site can emit a direct `CallExtern idx, argc`. The module is also added to the chunk's `imports` list keyed by its canonical spec so first-class references and `import_module()` lookups resolve.
4. For code modules, the source is parsed into a fresh `SSAChunk` and registered in the chunk's `imports` list. Each requested name becomes a `LoadModule + LoadAttr + StoreName` triple at the call site — no per-importer splicing, no name mangling.

Modules are **singletons across the compilation unit**: the same canonical spec compiles to one `SSAChunk`, runs its top level once, and lives as one `HeapObj::Module` value shared by every importer. Two files importing `./util.py` see literally the same module value — `mod is mod_alias` is true and mutations to module attributes are observed by every consumer. Inside the module's top level, `__name__` is bound to the canonical spec, so the `if __name__ == "__main__":` guard skips when the file is imported.

Helpers and constants stay private to their defining module: they live in the module's own slot frame and are reached via attribute access on the `HeapObj::Module` Val, not through the parent chunk's name table. Two modules with same-named helpers (`a.helper` vs. `b.helper`) keep their own bindings — `a.f`'s call to `helper` resolves through `a`'s attrs, not the importer's globals.

The runtime never fetches anything. The host (browser JS, WASI runtime, embedded Rust app) is responsible for bringing the bytes; the compiler accepts them through the `Resolver` trait.

## packages.json

Bare-name imports resolve through an import map declared in your project's `packages.json` (sitting next to the entry script). The manifest is always called `packages.json`; there is no `edge.json` or other variant.

```json
{
  "imports": {
    "utils":   "./lib/utils.py",
    "helpers": "https://example.com/helpers.py",
    "math":    "./vendor/math.wasm"
  }
}
```

The schema is small and strict:

- The top-level value must be a JSON object. Empty `{}` is valid.
- `imports` (optional): object mapping alias -> spec string.
- `extends` (optional): string naming a directory whose `packages.json` is consulted when an alias isn't found locally.
- Unknown top-level keys are silently ignored (forward-compatible).
- Booleans, numbers, and arrays at any level are rejected.
- String escapes accepted: `\"`, `\\`, `\/`, `\n`, `\t`, `\r`. **`\uXXXX` is not supported**, paste the character literally (the file must be UTF-8).

After this, `from utils import x` resolves to `./lib/utils.py` relative to the entry script's directory; `from math import add` loads the `.wasm` per the [wire format](/reference/wasm-abi).

The `packages.json` file is **optional**. Scripts can use string-form paths directly without any project config — useful for one-off scripts and playground demos.

### Walk-up resolution

Bare-name imports resolve against the **nearest `packages.json` walking up** from the importing file's directory. Each `packages.json` defines a *package boundary*: every file under its directory belongs to that package, and the manifest is the sole authority for what bare names mean inside it. Sub-directories may carry their own `packages.json` to scope their own aliases — exactly the way `node_modules` discovery works in Node and the way each crate has its own `Cargo.toml` in Rust.

```
my_app/
├── packages.json <- root manifest
├── main.py   bare imports here resolve via root manifest
├── lib/
│   ├── packages.json <- sub-package manifest
│   └── helper.py   bare imports here resolve via lib/ first
└── ...
```

Concretely:
- **Bare-name imports** (`from utils import x`) walk up from the importing file's directory looking for `packages.json`. The first one found decides. The walk is capped (currently **32 hops**) — exceeding it raises `packages.json walk-up exceeded <cap> hops resolving '<name>'`.
- **Hermetic by default**: if the nearest manifest doesn't declare the alias, compilation fails (`alias '<name>' not declared in '<manifest>'`). There is no silent fall-through to outer manifests — that prevents a deep transitive dep from accidentally borrowing aliases the parent declared.
- **`extends` opts in to inheritance**: a sub-manifest with `"extends": ".."` (or any directory expression) re-runs the search from the extended directory if it doesn't declare the alias locally. Cycles in the extends chain are detected at compile time (`circular extends chain in packages.json`).
- **Quoted relative paths** (`from "./helpers.py" import f`) resolve against the importing file's directory — a transitively-imported `lib/a.py` doing `from "./b.py" import g` correctly finds `lib/b.py`.

Spec classification (handled by the resolver):

| Spec shape | Example | Resolution |
|---|---|---|
| URL (contains `://`) | `https://x.com/u.py` | Used as-is; passed to `fetch_bytes` |
| Absolute (`/`-prefixed) | `/usr/share/lib.py` | Used as-is |
| Relative (`./` or `../`) | `./util.py` | Joined against importer's directory |
| Bare name | `utils` | Walk-up `packages.json` resolution |

#### `extends`

```json
// lib/packages.json
{
  "extends": "..",
  "imports": { "db": "./postgres.py" }
}
```

`db` is local to `lib/`. Anything else falls through to `../packages.json`. Use it for monorepo-style sub-packages that share a common pool of upstream deps with the parent; omit it for hermetic libraries that should not be affected by what the consumer declares.

### Diamond imports and cycles

When multiple paths import the same module, it's fetched, parsed, and initialised exactly once. The parser caches the parsed `SSAChunk` per canonical spec and the VM's `init_modules` walk dedupes by spec, so the module's top-level body runs once even if a hundred files import it. Direct or indirect cycles (`a.py` imports `b.py`, `b.py` imports `a.py`) surface as a runtime `circular import` error during init.

## Host responsibilities

Edge Python's compiler is a WebAssembly module. Fetching bytes — from disk, from `https://`, from your build artifact pipeline — is the host's job. The browser runtime ([`runtime/`](https://github.com/dylan-sutton-chavez/edge-python/tree/main/runtime)) does it via `fetch()` inside a Web Worker. WASI hosts use their runtime's filesystem and network APIs. Embedders pre-stage modules and feed them to the `Resolver`.

| Scenario | Who fetches |
|---|---|
| Browser playground | `runtime/` package — pre-fetches every spec the script imports. `.py` files register via `register_code_module`; `.wasm` files instantiate via `WebAssembly.instantiate` and register exports via `register_native_module`. |
| WASI runtime | Host program reads `.py` files from disk / network using `wasi_snapshot_preview1`. `.wasm` modules can be loaded via the runtime's WebAssembly engine. |
| Embedded Rust app | Caller links `compiler_lib`, implements `Resolver`, returns either `Resolved::Code(src)` or `Resolved::Native(bindings)`. In-process bindings have full VM heap access (full type coverage); see [Writing modules](/reference/writing-modules). |
| Production deploy | `edge compile` (planned) will seal all imports into a single `.epy` artifact. |

URL imports work for both `.py` and `.wasm` modules as long as your host supports them:

```python
from "https://example.com/utils.py" import normalize
from "https://example.com/math.wasm" import add
```

Or via packages.json alias:

```json
{ "imports": { "utils": "https://example.com/utils.py" } }
```

```python
from utils import normalize
```

### Integrity verification

Append `#sha256-<64 hex chars>` to any URL spec to require a content match:

```python
from "https://example.com/utils.py#sha256-deadbeef0123456789abcdef0123456789abcdef0123456789abcdef01234567" import normalize
```

The compiler asks the host for the raw bytes (via the `Resolver::fetch_bytes` trait method), computes the SHA-256, and refuses to compile if the hash doesn't match — with a diagnostic that surfaces both expected and computed digests:

```text
error: integrity check failed for 'https://example.com/utils.py'
  expected sha256-deadbeef0123456789abcdef0123456789abcdef0123456789abcdef01234567
  got      sha256-feedface9876543210fedcba9876543210fedcba9876543210fedcba98765432
```

Verification lives in the compiler itself, not the host — so any host (browser shim, WASI runtime, embedder) inherits the guarantee uniformly. Hosts that don't implement `fetch_bytes` surface a clean "not supported" error instead of silently bypassing the check, so a script asking for integrity never runs unverified.

Only `sha256` is supported today. A spec with any other prefix (`md5-...`, `sha384-...`) fails with `unrecognized integrity fragment`. The fragment's hex body must be **exactly 64 characters**; any other length raises `sha256 fragment must be 64 hex chars`.

## Lockfile and content-addressed cache

The browser worker auto-generates a **lockfile** and a **content-addressed cache** as a side-effect of running scripts that import URLs. Both live in IndexedDB; together they reduce repeat runs to zero network round-trips and detect upstream content drift on demand.

| File | Who writes it | Purpose |
|---|---|---|
| `packages.json` | the user | declares aliases (the manifest) |
| `packages.lock.json` (logical, in IDB) | the worker | records every fetched spec -> SHA-256 hash |
| `cas/<hash>` (per blob, in IDB) | the worker | bytes content-addressed by SHA-256 |

### What runs do

* **First run, cold**: every URL in the dependency graph is fetched, hashed, written to the CAS, and recorded in the lockfile.
* **Subsequent runs**: each spec is looked up in the lockfile; if the hash is known and the corresponding blob is in the CAS, the bytes are served without touching the network. Identical content under different URLs deduplicates automatically (the hash is the key).
* **`clearCache()`**: wipes the in-memory map, the CAS, and the lockfile. The next run treats everything as fresh.

### Drift detection

If a previously-locked URL serves different bytes than its recorded hash (because the upstream changed, or the cache was partially evicted and a re-fetch happened), the worker fails with both digests visible:

```text
integrity drift for 'https://cdn.foo/kit/index.py'
  locked: sha256-abc123...
  remote: sha256-zzz999...
```

This is the same primitive as inline `#sha256-...` integrity, applied automatically to every URL the user imports — explicit hashes in the source are still honoured and fail at compile time before any code runs.

### Other hosts

WASI hosts and Rust embedders make their own caching choices: the `Resolver` trait sees only `(spec -> Resolved)` and `(spec -> bytes via fetch_bytes)`. A CLI host typically pairs `packages.lock.json` next to `packages.json` (commitable to git) with `~/.cache/edgepython/sha256/` for the CAS, mirroring Cargo's split between project lockfile and shared registry cache. The browser worker uses IDB instead because that's where persistent storage lives in the browser.

## Sandbox

| Module type | Sandbox |
|---|---|
| Code modules (`.py`) | Full sandbox — code only calls capabilities it imports |
| Native modules (`.wasm`) | Full sandbox — WASM isolates by construction; the imported `.wasm` runs in its own linear memory |
| Native modules (in-process Rust closures) | Trust boundary is the host process — closures run with the embedder's privileges |

Edge Python runs as a WebAssembly module; scripts execute inside that sandbox unconditionally. `.wasm` native modules add their own WASM sandbox layer. In-process Rust bindings come from code the embedder controls, so trust extends only to whatever the embedder chose to expose.

## What doesn't work

- **Dynamic imports** — no `__import__`, no `importlib`. The module set is fixed per compilation. Use `import_module(name)` to *dispatch* among modules already statically imported elsewhere in the program.
- **Relative imports** — `from . import x` and `from .pkg import y` are **not** supported. Use quoted relative specs (`from "./foo.py" import x`) or declare an alias in `packages.json`.
- **Dotted submodule auto-discovery** — `import a.b.c` parses, but the dotted form is not auto-walked across the filesystem. Each importable spec must either be declared in `packages.json` or supplied as a quoted path/URL.

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

Runtime errors from native bindings (e.g., `upper()` with a non-string argument) propagate normally as `VmErr`.
