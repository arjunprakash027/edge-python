# Edge Python

A compact bytecode compiler and stack VM for a sandboxed Python subset, written in Rust. See [Design](https://edgepython.com/implementation/design) for the architecture.

Edge Python is distributed as a WebAssembly module — `compiler.wasm`, ~170 KB. It runs anywhere WebAssembly runs: browsers, Cloudflare Workers, Fastly Compute, Wasmtime, Wasmer, Spin. Sandboxed by construction.

- **Demo:** [demo.edgepython.com](https://demo.edgepython.com/)
- **Docs:** [edgepython.com](https://edgepython.com/)

## Repository layout

Cargo workspace; commands work from any directory.

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

Native modules ship via four delivery paths (CDN `.wasm`, in-process Rust, host capability, JS host module) — see [Writing modules](https://edgepython.com/reference/writing-modules).

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

The runtime spawns a Web Worker that pre-fetches imports, dispatches native calls, and streams `print()` output back.

Build the WASM yourself:

```bash
cargo wasm # -> target/wasm32-unknown-unknown/release/compiler_lib.wasm  (~390 KB unstripped)

# Optional: optimize with wasm-opt
wasm-opt -Oz target/.../compiler_lib.wasm -o compiler_lib.opt.wasm
```

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

`edge-python`'s own `build.rs` declares `links = "compiler_lib"` and downloads `compiler_lib.wasm` for the matching tag into `OUT_DIR`; cargo exposes its absolute path to your build script as `DEP_COMPILER_LIB_WASM`. Copy it wherever your host loads it from. Pinning to a tag gives reproducible builds; swap for `branch = "main"` when iterating against unreleased changes. Requires `curl` on the host PATH. The fetch is gated by the default-on `prebuilt` feature.

### Server / edge runtimes (Wasmtime, Wasmer, Cloudflare Workers, Fastly Compute, Spin)

Edge Python is a `cdylib` — your host runtime instantiates `compiler_lib.wasm` and calls into its exported entry points. The same `.wasm` you serve to browsers is the artifact you embed server-side. Reading scripts, fetching imports, surfacing output are the host's responsibility, exactly as in the browser case (just with WASI / runtime APIs instead of `fetch` / `postMessage`).

There is no built-in CLI binary. If you need one for local development, embed `compiler_lib.wasm` in a 50-line wasmtime shell — the same pattern any WASI host uses.

## What it is

Edge Python targets sandboxed edge computing: a dynamic, multi-paradigm Python subset with classes, async/await, structural pattern matching, and compile-time module resolution. There is no bundled stdlib — modules are external artifacts.

Full language reference, scope, and what intentionally isn't supported: [What Edge Python is](https://edgepython.com/getting-started/what-it-is). Architecture details: [`compiler/README.md`](compiler/README.md).

## License

MIT OR Apache-2.0

## Sponsors 

- [PyneSys](https://pynesys.io/) — since May 2026
