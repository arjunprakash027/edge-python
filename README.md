# Edge Python

A compact bytecode compiler and stack VM for a sandboxed Python subset, written in Rust. See [Design](https://edgepython.com/implementation/design) for the architecture.

Edge Python is distributed as a WebAssembly module, `compiler.wasm`, around 170 KB. It runs anywhere WebAssembly runs: browsers, Cloudflare Workers, Fastly Compute, Wasmtime, Wasmer, Spin. Sandboxed by construction.

- **Demo:** [demo.edgepython.com](https://demo.edgepython.com/)
- **Docs:** [edgepython.com](https://edgepython.com/)

## Repository layout

Cargo workspace; commands work from any directory.

```text
├── compiler
├── demo
├── documentation
├── runtime
├── starter-module
├── target
├── wasm-abi
└── wasm-pdk
```

```bash
cargo wasm           # release .wasm (the distributed artifact)
cargo build --release # host .rlib + cdylib for Rust embedders
cargo test --release  # full test suite
```

Native modules ship via three delivery paths (CDN `.wasm`, host capability, JS host module), see [Writing modules](https://edgepython.com/reference/writing-modules).

## Quick start

### Browser

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

The runtime spawns a Web Worker that pre-fetches imports, dispatches native calls, and streams `print()` output back. Build the WASM yourself with `cargo wasm` (output around 390 KB unstripped; optionally `wasm-opt -Oz` to shrink).

### Consume the release from a Rust host

Declare `edge-python` as a dependency and `compiler_lib.wasm` from the matching GitHub Release is fetched into `OUT_DIR` automatically, no manual download.

```toml
# Cargo.toml
[dependencies]
edge-python = { git = "https://github.com/dylan-sutton-chavez/edge-python", tag = "v0.1.0" }
```

```rust
// build.rs
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let wasm = std::env::var("DEP_COMPILER_LIB_WASM")
        .expect("`DEP_COMPILER_LIB_WASM` unset, upstream must declare `links = \"compiler_lib\"`");
    std::fs::copy(&wasm, "runtime/compiler_lib.wasm").expect("copy failed");
}
```

Pin to a tag for reproducible builds; use `branch = "main"` for unreleased changes. Requires `curl` on PATH. Gated by the default-on `prebuilt` feature.

### Server / edge runtimes (Wasmtime, Wasmer, Cloudflare Workers, Fastly Compute, Spin)

Edge Python is a `cdylib`, your host instantiates `compiler_lib.wasm` and calls its exports. The same `.wasm` you serve to browsers is the server-side artifact; the host owns I/O, fetching, and output (WASI / runtime APIs instead of `fetch` / `postMessage`). No built-in CLI, embed `compiler_lib.wasm` in around 50 LOC wasmtime shell for local dev.

## What it is

Edge Python targets sandboxed edge computing: a dynamic, multi-paradigm Python subset with classes, async/await, structural pattern matching, and compile-time module resolution. There is no bundled stdlib, modules are external artifacts.

Full language reference, scope, and what intentionally isn't supported: [What Edge Python is](https://edgepython.com/getting-started/what-it-is). Architecture details: [`compiler/README.md`](compiler/README.md).

## License

MIT OR Apache-2.0

## Sponsors 

- [PyneSys](https://pynesys.io/), since May 2026
