# Edge Python

A compact, single-pass SSA bytecode compiler and stack VM for a sandboxed Python subset. Hand-written lexer, Pratt parser that emits bytecode directly, and a threaded-code interpreter with dual inline caching (scalar + instance-dunder), super-instruction fusion, and pure-function memoization.

Edge Python is distributed as a WebAssembly module — `compiler.wasm`, ~170 KB. It runs anywhere WebAssembly runs: browsers, Cloudflare Workers, Fastly Compute, Wasmtime, Wasmer, Spin. Sandboxed by construction; no native release artifact.

- **Demo:** [demo.edgepython.com](https://demo.edgepython.com/)
- **Docs:** [edgepython.com](https://edgepython.com/)

## Repository layout

This is a Cargo workspace. The root `Cargo.toml` declares the workspace members and shares profile settings; `cargo` commands work from any directory.

```text
├── .cargo
├── .github
│   └── workflows
├── compiler
│   ├── src
│   └── tests
├── demo
│   ├── css
│   ├── js
│   ├── runtime
│   └── static
├── documentation
│   ├── getting-started
│   ├── implementation
│   ├── language
│   └── reference
├── runtime
│   ├── loaders
│   ├── src
│   └── worker
├── starter-module
│   └── src
├── target
│   ├── debug
│   ├── flycheck0
│   └── tmp
├── wasm-abi
│   └── src
└── wasm-pdk
    ├── macros
    └── src
```

Common commands (from anywhere in the repo):

```bash
cargo wasm # Release WebAssembly artifact (the distributed product).
cargo build --release # Host artifacts (.rlib + cdylib) for Rust embedders.
cargo test --release # Full test suite.
```

Native modules come in two flavors: `.wasm` binaries any host can load by URL (per the [WASM ABI](documentation/reference/wasm-abi.md)) and in-process Rust bindings for embedders linking `compiler_lib` (full type coverage). See [Writing modules](documentation/reference/writing-modules.md).

## Quick start

### Browser

Two artifacts: the WASM module + the JS runtime published with this repo under [`runtime/`](runtime/). Consumers do not write any JavaScript — they import `createWorker` and use it:

```html
<script type="module">
    import { createWorker } from 'https://runtime.edgepython.com/js/src/index.js';

    const worker = await createWorker({
        wasmUrl: 'https://runtime.edgepython.com/js/compiler_lib.wasm',
        imports: { "math": "https://example.com/math.wasm" }
    });
    worker.onOutput(line => console.log(line));

    await worker.run(`
        from math import add
        from "https://example.com/utils.py" import normalize
        print(add(2, 3))
        print(normalize("  hi  "))
    `);
</script>
```

The runtime spawns a Web Worker that pre-fetches imports, registers modules with the compiler, dispatches native calls, and streams `print()` output back. **The JS runtime is necessary in browsers:** the WebAssembly sandbox does not expose network or filesystem to the WASM module — every external resource must come through a host-side bridge, and in browsers that bridge is JavaScript. Edge Python's "no JS for the user" principle is preserved by distributing the bridge as part of the official release; the runtime is consumed the same way as any WASM library's loader (Pyodide, sql.js, etc.).

Build the WASM yourself:

```bash
cargo wasm # -> target/wasm32-unknown-unknown/release/compiler_lib.wasm  (~390 KB unstripped)

# Optional: optimize with wasm-opt
wasm-opt -Oz target/.../compiler_lib.wasm -o compiler_lib.opt.wasm
```

`cargo wasm` is a workspace alias (`.cargo/config.toml`) for `cargo build --release --target wasm32-unknown-unknown -p edge-python`. Plain `cargo build --release` produces host-side library artifacts (`.rlib` + host cdylib) for embedders linking `compiler_lib` directly into a Rust app.

### Consume the release from a Rust host

If your host runtime is itself a Rust crate (a wasmtime shell, a custom browser bridge, a CLI wrapper, etc.), declare `edge-python` as a build dependency and the matching `compiler_lib.wasm` from the GitHub Release is fetched into `OUT_DIR` automatically — no manual download, no `cargo wasm` step.

`Cargo.toml`:

```toml
[dependencies]
edge-python = { git = "https://github.com/dylan-sutton-chavez/edge-python", tag = "v0.1.0" }
```

`build.rs`:

```rust
fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    let wasm = std::env::var("DEP_COMPILER_LIB_WASM")
        .expect("`DEP_COMPILER_LIB_WASM` unset — upstream `edge-python` must declare `links = \"compiler_lib\"`");

    std::fs::copy(&wasm, "runtime/compiler_lib.wasm").expect("copy failed");
}
```

`edge-python`'s own `build.rs` declares `links = "compiler_lib"` and downloads `compiler_lib.wasm` for the matching tag into `OUT_DIR`; cargo exposes its absolute path to your build script as `DEP_COMPILER_LIB_WASM`. Copy it wherever your host loads it from (e.g. a `runtime/` directory served by your JS host, or your binary's resources). Pinning to a tag gives reproducible builds; swap for `branch = "main"` when iterating against unreleased changes. Requires `curl` on the host PATH.

The fetch is gated by the default-on `prebuilt` feature. Default dependency lines (above) already enable it, so nothing changes for typical consumers; if you opt out of default features for unrelated reasons, re-enable explicitly with `features = ["prebuilt"]`.

### Server / edge runtimes (Wasmtime, Wasmer, Cloudflare Workers, Fastly Compute, Spin)

Edge Python is a `cdylib` — your host runtime instantiates `compiler_lib.wasm` and calls into its exported entry points. The same `.wasm` you serve to browsers is the artifact you embed server-side. Reading scripts, fetching imports, surfacing output are the host's responsibility, exactly as in the browser case (just with WASI / runtime APIs instead of `fetch` / `postMessage`).

There is no built-in CLI binary. If you need one for local development, embed `compiler_lib.wasm` in a 50-line wasmtime shell — the same pattern any WASI host uses.

## What it is

Edge Python targets sandboxed edge computing. The language is dynamic and multi-paradigm: first-class functions, lambdas, closures, decorators (including class decorators), generators, async/await with a built-in cooperative scheduler, comprehensions, structural pattern matching, and pure-function memoization. Classes support single-level inheritance, `super()`, dunder-method dispatch (operators, indexing, iteration, context managers, etc.), and `@property` / `@x.setter`. Integers are 47-bit inline with automatic promotion to i128 LongInt on overflow; the hard cap is ±2^127.

Imports resolve at compile time through a host-injected resolver. Bare names walk up `packages.json` manifests; quoted specs (`"./util.py"`, `"https://..."`) are loaded verbatim and may carry a `#sha256-<hex>` integrity fragment. `.py` modules are compiled and run once; native modules dispatch via the `CallExtern` opcode (either a `.wasm` loaded by URL per the public ABI, or in-process Rust closures from the embedder). There is no bundled stdlib — modules are external artifacts.

For architecture details, see [`compiler/README.md`](compiler/README.md). For language reference and the import system, see the [docs](https://edgepython.com/).

## License

MIT OR Apache-2.0

## Sponsors 

- [PyneSys](https://pynesys.io/) — since May 2026
