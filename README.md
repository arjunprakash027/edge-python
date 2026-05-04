# Edge Python

A compact, single-pass SSA bytecode compiler and stack VM for a functional subset of CPython 3.13 syntax. Hand-written lexer, Pratt parser that emits bytecode directly, and a threaded-code interpreter with per-instruction inline caching and pure-function memoization.

Built for deterministic execution in sandboxed and embedded environments. The release WASM build is ~130 KB.

- **Demo:** [demo.edgepython.com](https://demo.edgepython.com/)
- **Docs:** [edgepython.com](https://edgepython.com/)

## Repository layout

```text
# Rust crate: lexer, parser, optimizer, VM, packages module
compiler/

# SDK for writing native modules in Rust (compiled to .wasm)
edge-sdk/

# Browser playground (HTML + WASM + Web Worker)
demo/

# Mintlify documentation source
documentation/

# CI/CD pipelines (lint, native builds, WASM, demo)
.github/
```

## Quick start

### Native CLI

```bash
cd compiler
cargo build --release
./target/release/edge -c 'print((lambda x: x * 2)(21))'

# Run a file with sandbox limits
./target/release/edge --sandbox script.py

# Multi-file project with imports (.py + .wasm modules + HTTPS URLs)
./target/release/edge main.py    # reads packages.json from script's dir
```

Pre-built binaries for Linux, macOS, and Windows are available on the [releases page](https://github.com/dylan-sutton-chavez/edge-python/releases).

### Browser

Two files: the WASM module + a thin JS loader that ships in this repo at [`demo/edge.js`](demo/edge.js). Consumers don't write any JavaScript — they include both files and use the `EdgePython` class:

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

The shim handles all the WASM ↔ JS plumbing: pre-fetching imports, registering modules with the WASM runtime, dispatching native calls back into JS, decoding `print()` output. **Why a JS shim is unavoidable in browsers:** the WebAssembly sandbox doesn't expose network or filesystem to the WASM module — every external resource has to come through a host-side bridge, and in browsers that bridge is JavaScript. Edge Python's brand of "no JS for the user" is preserved by shipping the bridge as part of the official distribution; you include `edge.js` the same way you'd include any WASM library's loader (Pyodide, sql.js, etc.).

Build the WASM yourself:

```bash
cd compiler
cargo build --release --target wasm32-unknown-unknown --lib --features wasm
# → target/wasm32-unknown-unknown/release/compiler_lib.wasm  (~390 KB unstripped)

# Optional: optimize with wasm-opt
wasm-opt -Oz target/.../compiler_lib.wasm -o compiler_lib.opt.wasm
```

### WASI environments (Wasmtime, Wasmer, Cloudflare Workers, Fastly Compute)

The same Rust crate can target [WASI](https://wasi.dev/) — WebAssembly with system-level capabilities (network, FS, env vars). In WASI environments, the JS shim isn't needed because the WASM runtime itself provides standard syscalls. **Edge Python doesn't ship a dedicated WASI build today**; the path to one is:

1. Build for `wasm32-wasip1` (or `wasm32-wasip2` for the component model):

   ```bash
   rustup target add wasm32-wasip1
   cargo build --release --target wasm32-wasip1 --bin edge
   ```

2. Run with any WASI host:

   ```bash
   wasmtime ./target/wasm32-wasip1/release/edge.wasm script.py
   wasmer ./target/wasm32-wasip1/release/edge.wasm script.py
   ```

   Or deploy to edge platforms that support WASI (Cloudflare Workers via `workerd`, Fastly Compute, Spin, etc.).

3. The `edge` CLI binary works as-is once you wire its file/network access through WASI's standard interfaces — `std::fs` and `ureq` already do this transparently when targeting `wasm32-wasip1`.

The WASI path is what you want for serverless/edge runtimes where you need EdgePython without a browser. The browser path is what you want for client-side playgrounds and embedded scripts in web apps.

## What it is

Edge Python targets functional edge computing: first-class functions, lambdas, closures, generators, comprehensions, and pure-function memoization. Classes are supported with `__init__`, attributes, and methods. Imports resolve at compile time through a host-injected `Resolver`: `.py` modules are inlined as functions; native modules (`.wasm`/dyn-libs in any low-level language) dispatch via the `CallExtern` opcode. There is no bundled stdlib — modules are external artifacts distributed by URL.

For architecture details, see [`compiler/README.md`](compiler/README.md). For language reference, the import system, and how to author native modules, see the [docs](https://edgepython.com/).

## License

MIT OR Apache-2.0