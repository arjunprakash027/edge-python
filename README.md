# Edge Python

A compact, single-pass SSA bytecode compiler and stack VM for a functional subset of CPython 3.13 syntax. Hand-written lexer, Pratt parser that emits bytecode directly, and a threaded-code interpreter with per-instruction inline caching and pure-function memoization.

Edge Python ships as a WebAssembly module — `compiler.wasm`, ~130 KB. It runs anywhere WebAssembly runs: browsers, Cloudflare Workers, Fastly Compute, Wasmtime, Wasmer, Spin. Sandboxed by construction; no native release artifact.

- **Demo:** [demo.edgepython.com](https://demo.edgepython.com/)
- **Docs:** [edgepython.com](https://edgepython.com/)

## Repository layout

```text
# Rust crate: lexer, parser, optimizer, VM, packages module. Compiles to .wasm.
compiler/

# SDK for writing native modules in Rust (compiled to .wasm)
edge-sdk/

# Browser playground (HTML + WASM + Web Worker)
demo/

# Mintlify documentation source
documentation/

# CI/CD pipelines (lint, WASM build, demo deploy)
.github/
```

## Quick start

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

### Server / edge runtimes (Wasmtime, Wasmer, Cloudflare Workers, Fastly Compute, Spin)

Edge Python is a `cdylib` — your host runtime instantiates `compiler_lib.wasm` and calls into its exported entry points. The same `.wasm` you serve to browsers is the artifact you embed server-side. Reading scripts, fetching imports, surfacing output are the host's responsibility, exactly as in the browser case (just with WASI / runtime APIs instead of `fetch` / `postMessage`).

There is no built-in CLI binary. If you need one for local development, embed `compiler_lib.wasm` in a 50-line wasmtime shell — the same pattern any WASI host uses.

## What it is

Edge Python targets functional edge computing: first-class functions, lambdas, closures, generators, comprehensions, and pure-function memoization. Classes are supported with `__init__`, attributes, and methods. Imports resolve at compile time through a host-injected `Resolver`: `.py` modules are inlined as functions; `.wasm` modules dispatch via the `CallExtern` opcode. There is no bundled stdlib — modules are external artifacts the host fetches and feeds to the resolver.

For architecture details, see [`compiler/README.md`](compiler/README.md). For language reference and the import system, see the [docs](https://edgepython.com/).

## License

MIT OR Apache-2.0
