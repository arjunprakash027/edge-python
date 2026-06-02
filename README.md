<div align="center">
  <a href="https://edgepython.com/" target="_blank">
    <picture>
      <img width="300" src="docs/public/static/banner.svg" alt="Edge Python Logo">
    </picture>
  </a>
</div>

<br/>

Edge is a sandboxed subset of Python, compiled to a less than 200 KB WebAssembly binary and built in Rust to run on Cloudflare Workers and in the browser. Embed your full business logic, run LLMs client-side, build frontend apps and serverless workloads.

- Secure by default. No file, network, or environment access, unless explicitly enabled by the [host](https://edgepython.com/reference/packages#host-libraries).
- Less than 200 KB footprint. The full compiler and runtime ship as a single WASM binary.
- Compile-time imports. Every module resolves at parse time no dynamic loading, no runtime surprises.
- No AST, source compiles directly to bytecode in a single pass: o(n)

## More about it

- Demo: [demo.edgepython.com](https://demo.edgepython.com/)
- Docs: [edgepython.com](https://edgepython.com/)

## Repository layout

Cargo workspace; commands work from any directory.

```text
├── cli
├── compiler
├── demo
├── docs
├── host
├── runtime
├── std
├── target
├── wasm-abi
└── wasm-pdk
```

```bash
cargo wasm # release .wasm (the distributed artifact)
cargo build --release # host .rlib + cdylib for Rust embedders
cargo test --release --no-default-features # run the compiler test suite
```

*`--no-default-features` disables the `prebuilt` feature, which otherwise triggers a `build.rs` download of `compiler.wasm` from the GitHub release: that download is only needed by external Rust crates that consume this library, not when developing locally.*

Native modules ship via three delivery paths (CDN `.wasm`, host capability, JS host module), see [Writing modules](https://edgepython.com/reference/writing-modules).

## Quick start

### CLI

download it to your machine ([reference docs](https://edgepython.com/reference/cli)):

```bash
curl -fsSL https://cdn.edgepython.com/cli/install.sh | sh

edge -h # List all commands
```

`edge` hosts the runtime in a headless Chromium provisioned by `install.sh` (apt, dnf, pacman, zypper, apk, or brew on macOS) for `serve`, `repl`, `build` and `uninstall`.

### Browser

```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="UTF-8">
  <script type="module" src="https://cdn.edgepython.com/runtime/src/element.js"></script>
</head>
<body>
  <edge-python entry="./app/main.py" packages="./app/packages.json"></edge-python>
</body>
</html>
```

The runtime spawns a Web Worker that pre-fetches imports, dispatches native calls, and streams `print()` output back.

### Rust host / server / edge runtimes

Edge Python is a `cdylib`: your host instantiates `compiler.wasm` and calls its exports (Wasmtime, Wasmer, Cloudflare Workers, Fastly Compute, Spin). The same `.wasm` served to browsers is the server-side artifact; the host owns I/O. Declaring `edge-python` as a Cargo dependency fetches the matching release `.wasm` automatically, see [`compiler/README.md`](compiler/README.md).

## What it is

Edge Python targets sandboxed edge computing: a dynamic, multi-paradigm Python subset with classes, async/await, structural pattern matching, and compile-time module resolution. There is no bundled stdlib, modules are external artifacts.

Full language reference, scope, and what intentionally isn't supported: [What Edge Python is](https://edgepython.com/getting-started/what-it-is). Architecture details: [`compiler/README.md`](compiler/README.md).

## CI/CD

One workflow [`.github/workflows/main.yml`](.github/workflows/main.yml) that runs the complete CI/CD, where each package is a steps in a composite action under [`.github/actions/`](.github/actions).

On pushes to `main` it deploys three Cloudflare Pages projects: `edge-python-cdn` (the bundled package artifacts), `edge-python-demo`, and `edge-python-docs` (served at `edgepython.com`). 

## License

MIT OR Apache-2.0

## Sponsors 

- [PyneSys](https://pynesys.io/), since May 2026
