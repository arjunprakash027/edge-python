# Edge Python Standard Packages

Official `.wasm` standard-library packages for [Edge Python](https://edgepython.com). Each capability is a Rust crate compiled to `wasm32-unknown-unknown` against the [wasm-pdk](https://github.com/dylan-sutton-chavez/edge-python/tree/main/wasm-pdk) ABI. Hosts load the resulting `.wasm` over the standard plugin contract, no custom embedder, no Rust on the consumer side.

## Layout

```
tests/, agnostic Deno + Playwright runner driving the <edge-python> tag
<name>/, one folder per stdpkg crate, with src/, README.md, and <name>.json corpus
```

The folder name IS the package name IS the wasm artifact name (e.g. `json/` -> `json/target/wasm32-unknown-unknown/release/json.wasm`). Each package's `<name>.json` sits alongside `Cargo.toml`; cases in it are automatically prefixed with `from <name> import *\n` before dispatch, so the corpus only contains the code being tested.

## Packages

| Folder | Description |
|--------|-------------|
| `json` | JSON serialization/deserialization, see [`json/README.md`](json/README.md) |
| `re` | Regular expressions, a subset with capture, backreferences, lookaround, and a ReDoS step budget, see [`re/README.md`](re/README.md) |

## Build + test

Each package builds independently; the agnostic runner asserts against the produced `.wasm`. From the repo root:

```bash
# Build every package's .wasm artifact.
( cd json && cargo build --release --target wasm32-unknown-unknown )

# One command, drives all corpora through the shared runner.
deno test --allow-all tests/
```

The runner discovers packages by walking the repo root for `<name>/<name>.json` corpora. CI runs this matrix via `.github/workflows/`.

## Adding a new stdpkg

1. Create `<name>/` at the repo root with `Cargo.toml` (`name = "<name>"`, `crate-type = ["cdylib"]`, `wasm-pdk` dep) and a `src/lib.rs` exporting via `#[plugin_fn]`.
2. Drop `<name>/<name>.json` with the corpus (Edge Python source + expected `output` / `error` per case).
3. Run `cargo build --release --target wasm32-unknown-unknown` inside the package folder.
4. Run `deno test --allow-all tests/` from the repo root.

No edits to `tests/`.

## License

MIT OR Apache-2.0
