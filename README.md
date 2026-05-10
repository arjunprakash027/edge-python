# Edge Python

A compact, single-pass SSA bytecode compiler and stack VM for a functional subset of Python 3.13 syntax. Hand-written lexer, Pratt parser that emits bytecode directly, and a threaded-code interpreter with per-instruction inline caching and pure-function memoization.

Edge Python is distributed as a WebAssembly module — `compiler.wasm`, ~153 KB. It runs anywhere WebAssembly runs: browsers, Cloudflare Workers, Fastly Compute, Wasmtime, Wasmer, Spin. Sandboxed by construction; no native release artifact.

- **Demo:** [demo.edgepython.com](https://demo.edgepython.com/)
- **Docs:** [edgepython.com](https://edgepython.com/)

## Repository layout

This is a Cargo workspace. The root `Cargo.toml` declares one crate and shares profile settings; `cargo` commands work from any directory.

```text
Cargo.toml                  Workspace manifest (members + shared profile)
.cargo/config.toml          Workspace-wide aliases (`cargo wasm`)

compiler/                   Rust crate `edge-python`: lexer, parser, optimizer,
                            VM, packages module. Compiles to compiler_lib.wasm
                            (the only artifact the project distributes).

demo/                       Browser playground (HTML + WASM + Web Worker)
documentation/              Mintlify documentation source
.github/                    CI/CD pipelines (lint, WASM build, demo deploy)
```

Common commands (from anywhere in the repo):

```bash
cargo wasm                  # release WebAssembly artifact (the distributed product)
cargo build --release       # host artifacts (.rlib + cdylib) for Rust embedders
cargo test --release        # full test suite
```

Native modules come in two flavors: `.wasm` binaries any host can load by URL (per the [WASM ABI](documentation/reference/wasm-abi.md)) and in-process Rust bindings for embedders linking `compiler_lib` (full type coverage). See [Writing modules](documentation/reference/writing-modules.md).

## Quick start

### Browser

Two files: the WASM module + a thin JS loader included in this repo at [`demo/edge.js`](demo/edge.js). Consumers do not write any JavaScript — they include both files and use the `EdgePython` class:

```html
<script type="module">
  import { EdgePython } from './edge.js';

  const ep = await EdgePython.create({
    wasmUrl: './compiler_lib.wasm',
    imports: { "math": "https://example.com/math.wasm" }
  });
  ep.onOutput(line => console.log(line));

  await ep.run(`
    from math import add
    from "https://example.com/utils.py" import normalize
    print(add(2, 3))
    print(normalize("  hi  "))
  `);
</script>
```

The shim handles the WASM ↔ JS plumbing: pre-fetching imports, registering modules with the WASM runtime, dispatching native calls back into JS, and decoding `print()` output. **The JS shim is necessary in browsers:** the WebAssembly sandbox does not expose network or filesystem to the WASM module — every external resource must come through a host-side bridge, and in browsers that bridge is JavaScript. Edge Python's "no JS for the user" principle is preserved by distributing the bridge as part of the official release; `edge.js` is included the same way as any WASM library's loader (Pyodide, sql.js, etc.).

Build the WASM yourself:

```bash
cargo wasm
# -> target/wasm32-unknown-unknown/release/compiler_lib.wasm  (~390 KB unstripped)

# Optional: optimize with wasm-opt
wasm-opt -Oz target/.../compiler_lib.wasm -o compiler_lib.opt.wasm
```

`cargo wasm` is a workspace alias (`.cargo/config.toml`) for `cargo build --release --target wasm32-unknown-unknown -p edge-python`. Plain `cargo build --release` produces host-side library artifacts (`.rlib` + host cdylib) for embedders linking `compiler_lib` directly into a Rust app.

### Server / edge runtimes (Wasmtime, Wasmer, Cloudflare Workers, Fastly Compute, Spin)

Edge Python is a `cdylib` — your host runtime instantiates `compiler_lib.wasm` and calls into its exported entry points. The same `.wasm` you serve to browsers is the artifact you embed server-side. Reading scripts, fetching imports, surfacing output are the host's responsibility, exactly as in the browser case (just with WASI / runtime APIs instead of `fetch` / `postMessage`).

There is no built-in CLI binary. If you need one for local development, embed `compiler_lib.wasm` in a 50-line wasmtime shell — the same pattern any WASI host uses.

## What it is

Edge Python targets functional edge computing: first-class functions, lambdas, closures, decorators (including class decorators), generators, async/await with a built-in cooperative scheduler, comprehensions, structural pattern matching, and pure-function memoization. Classes exist as flat state containers with `__init__`, attributes, and methods — no inheritance, no `super()`, no descriptor protocol, and no dunder-method dispatch (operators and protocols dispatch on type tag, not user-class methods). Integers are 47-bit inline (overflow raises `OverflowError`); there is no bignum.

Imports resolve at compile time through a host-injected resolver. Bare names walk up `packages.json` manifests; quoted specs (`"./util.py"`, `"https://..."`) are loaded verbatim and may carry a `#sha256-<hex>` integrity fragment. `.py` modules are compiled and run once; native modules dispatch via the `CallExtern` opcode (either a `.wasm` loaded by URL per the public ABI, or in-process Rust closures from the embedder). There is no bundled stdlib — modules are external artifacts.

For architecture details, see [`compiler/README.md`](compiler/README.md). For language reference and the import system, see the [docs](https://edgepython.com/).

## License

MIT OR Apache-2.0

## Sponsors 

- [PyneSys](https://pynesys.io/) — since May 2026
