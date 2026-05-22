# Edge Python

A compact single-pass SSA bytecode compiler and stack VM for a sandboxed Python subset. Hand-written lexer, Pratt parser that emits bytecode directly (no AST), and a threaded-code interpreter with dual inline caching (scalar + instance-dunder), super-instruction fusion, and pure-function template memoization. Deterministic execution; ~170 KB WASM release.

* **Demo:** [demo.edgepython.com](https://demo.edgepython.com/)
* **Docs:** [edgepython.com](https://edgepython.com/)

## Architecture

Single-pass pipeline: source → bytecode in an SSA chunk; stack interpreter with adaptive inline caching and pure-function memoization.

* **Lexer** (`modules/lexer/`) — LUT-driven, offset-based tokens. See [Lexical](https://edgepython.com/implementation/lexical).
* **Parser** (`modules/parser/`) — Pratt precedence; SSA-versioned bytecode with `Phi` at control-flow joins; no AST. See [Syntax (impl)](https://edgepython.com/implementation/syntax).
* **Optimizer** (`modules/vm/optimizer.rs`) — constant folding, Phi-noop elimination, dead-instruction compaction. Preserves `LoadName` for IC.
* **VM** (`modules/vm/`) — flat-match dispatch on `(opcode, operand: u16)`; `handlers/` + `handlers/builtin_methods/`. `LoadAttr + Call(0)` fuses into `CallMethod`.
* **Inline caching** (`modules/vm/cache.rs`) — scalar IC promotes arith/compare to typed `FastOp` after 4 hits; instance-dunder IC caches `(class_idx, method)`.
* **Template memoization** — pure-function results cached after 2 hits; impurity tagged on `StoreItem` / `StoreAttr` / I/O / `raise` / `yield`.
* **Memory** — NaN-boxed 64-bit `Val` (47-bit inline int, IEEE-754 float, bool, None, 28-bit heap index); mark-and-sweep arena; interned strings/bytes ≤ 128 B; auto-promote to i128 `LongInt`, capped at ±2¹²⁷.
* **Resolver** (`modules/packages/`) — host-injected; `packages.json` walk-up; native imports register for `CallExtern` dispatch.

Full rationale, NaN-box patterns, IC thresholds, GC roots, and intentional omissions: [Design](https://edgepython.com/implementation/design).

## Layout

```text
src/
  abi.rs, lib.rs
  main/        — abi_bridge, errors, exports, resolver
  modules/
    lexer/     — scan.rs, tables.rs
    parser/    — control, expr, imports, literals, stmt, types
    packages/  — manifest, mod
    vm/
      builtins/         — async_ops, attr, container, conversion, …
      handlers/         — arith, dunder, format, function, methods, …
        builtin_methods/  — bytes, dict, list, set, string
      cache.rs, dispatch.rs, gc.rs, optimizer.rs, ops.rs
      types/            — coro, eq, err, math
  util/        — fstr, fx, sha256
tests/         — cases/*.json + lexer.rs, parser.rs, vm.rs, packages.rs, main.rs
```

## Quick start

```bash
cargo wasm           # release WASM artifact -> target/wasm32-unknown-unknown/release/compiler_lib.wasm
cargo test --release # host-side test suite
```

`cargo wasm` is a workspace alias (`.cargo/config.toml`) for `cargo build --release --target wasm32-unknown-unknown -p edge-python`. Plain `cargo build --release` produces host artifacts (`.rlib` + cdylib) for embedders linking `compiler_lib`. To add native modules from your own crate, implement the `Resolver` trait — see [Writing modules](https://edgepython.com/reference/writing-modules).

The host runtime owns I/O, network, and module fetching; there is no native CLI. Browser hosts use the [`runtime/`](../runtime/) JS package; server/edge runtimes use wasmtime, wasmer, Cloudflare Workers, Fastly Compute, Spin.

### Consuming the release from another Rust crate

This crate declares `links = "compiler_lib"` and its `build.rs` downloads the matching `compiler_lib.wasm` from the GitHub Release for `CARGO_PKG_VERSION` into `OUT_DIR`. Downstream crates read the absolute path through `DEP_COMPILER_LIB_WASM`.

```toml
# Downstream Cargo.toml
[dependencies]
edge-python = { git = "https://github.com/dylan-sutton-chavez/edge-python", tag = "v0.1.0" }
```

```rust
// Downstream build.rs
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let wasm = std::env::var("DEP_COMPILER_LIB_WASM")
        .expect("`DEP_COMPILER_LIB_WASM` unset — upstream `edge-python` must declare `links = \"compiler_lib\"`");
    std::fs::copy(&wasm, "runtime/compiler_lib.wasm").expect("copy failed");
}
```

URL is derived from `<repository>/releases/download/v<version>/compiler_lib.wasm` — a tag bump is the only retarget needed. Use `branch = "main"` for unreleased work. Requires `curl` on PATH. Gated by the default-on `prebuilt` feature; producer-side commands pass `--no-default-features` to skip.

## References

1. **Aho, Sethi & Ullman**, *Compilers: Principles, Techniques and Tools* (1986). LUT-based lexer.
2. **Pratt**, *Top Down Operator Precedence* (POPL 1973). Precedence climbing parser.
3. **Cytron et al.**, *Efficiently Computing Static Single Assignment Form* (TOPLAS 1991). SSA, φ-nodes.
4. **Gudeman**, *Representing Type Information in Dynamically Typed Languages* (1993). NaN-boxing.
5. **Deutsch & Schiffman**, *Efficient Implementation of the Smalltalk-80 System* (POPL 1984). Inline caching.
6. **Ertl & Gregg**, *The Structure and Performance of Efficient Interpreters* (JILP 2003). Threaded dispatch.
7. **Hölzle & Ungar**, *Optimizing Dynamically-Dispatched Calls with Run-Time Type Feedback* (PLDI 1994).
8. **Casey et al.**, *Towards Superinstructions for Java Interpreters* (SCOPES 2003). LoadAttr+Call fusion.
9. **Michie**, *Memo Functions and Machine Learning* (Nature 1968). Pure-function memoization.
10. **McCarthy**, *Recursive Functions of Symbolic Expressions* (CACM 1960). Mark-sweep GC.
11. **Backus**, *Can Programming Be Liberated from the von Neumann Style?* (CACM 1978). Function-level paradigm.
