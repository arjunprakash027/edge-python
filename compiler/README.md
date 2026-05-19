# Edge Python

A compact, single-pass SSA-style bytecode compiler and stack VM for a sandboxed Python subset. Hand-written lexer, Pratt-precedence parser that emits bytecode directly (no AST), and a threaded-code interpreter with dual inline caching (scalar + instance-dunder), super-instruction fusion, and pure-function template memoization. Built for deterministic execution in sandboxed and embedded environments (around 170 KB WASM release).

* **Demo:** [demo.edgepython.com](https://demo.edgepython.com/)
* **Docs:** [edgepython.com](https://edgepython.com/)

---

## Architecture

The compiler is a single-pass pipeline that emits bytecode directly into an SSA chunk; the VM is a stack interpreter with adaptive inline caching and pure-function memoization.

* **Lexer** (`modules/lexer/`) — hand-written LUT-driven scanner; offset-based tokens. See [Lexical](https://edgepython.com/implementation/lexical).
* **Parser** (`modules/parser/`) — Pratt precedence climbing; emits SSA-versioned bytecode with `Phi` opcodes at control-flow joins; no AST. See [Syntax (impl)](https://edgepython.com/implementation/syntax).
* **Optimizer** (`modules/vm/optimizer.rs`) — constant folding, Phi-noop elimination, dead-instruction compaction. Preserves `LoadName` to keep the IC slot live.
* **VM** (`modules/vm/`) — flat-match dispatch on `(opcode: OpCode, operand: u16)`; hot path split across `handlers/` and a per-type method package in `handlers/builtin_methods/`. `LoadAttr + Call(0)` fuses into `CallMethod` super-instruction.
* **Inline caching** (`modules/vm/cache.rs`) — scalar IC promotes arithmetic/comparison sites to typed `FastOp` after 4 hits; instance-dunder IC caches `(class_idx, method)` for monomorphic dispatch.
* **Template memoization** — pure-function results cached after 2 hits; impurity tagged on `StoreItem` / `StoreAttr` / I/O / `raise` / `yield`.
* **Memory** — NaN-boxed 64-bit `Val` (47-bit inline int, IEEE-754 float, bool, None, 28-bit heap index); mark-and-sweep arena heap with interned strings/bytes ≤ 128 B; integers auto-promote to i128 `LongInt` on overflow, capped at ±2^127.
* **Resolver** (`modules/packages/`) — host-injected; `packages.json` walk-up; native imports register in `chunk.extern_table` for `CallExtern` dispatch.

Full design rationale, NaN-box bit patterns, IC thresholds, GC root list, and the "what the compiler intentionally does *not* do" list: [Design](https://edgepython.com/implementation/design).

---

## Project Structure

```text
├── Cargo.toml
├── README.md
├── src
│   ├── abi.rs
│   ├── lib.rs
│   ├── main
│   │   ├── abi_bridge.rs
│   │   ├── errors.rs
│   │   ├── exports.rs
│   │   ├── mod.rs
│   │   └── resolver.rs
│   ├── modules
│   │   ├── lexer
│   │   │   ├── mod.rs
│   │   │   ├── scan.rs
│   │   │   └── tables.rs
│   │   ├── packages
│   │   │   ├── manifest.rs
│   │   │   └── mod.rs
│   │   ├── parser
│   │   │   ├── control.rs
│   │   │   ├── expr.rs
│   │   │   ├── imports.rs
│   │   │   ├── literals.rs
│   │   │   ├── mod.rs
│   │   │   ├── stmt.rs
│   │   │   └── types.rs
│   │   └── vm
│   │       ├── builtins
│   │       │   ├── async_ops.rs
│   │       │   ├── attr.rs
│   │       │   ├── bytes_helpers.rs
│   │       │   ├── container.rs
│   │       │   ├── conversion.rs
│   │       │   ├── identity.rs
│   │       │   ├── index.rs
│   │       │   ├── io.rs
│   │       │   ├── mod.rs
│   │       │   ├── numeric.rs
│   │       │   └── sequence.rs
│   │       ├── cache.rs
│   │       ├── dispatch.rs
│   │       ├── gc.rs
│   │       ├── handlers
│   │       │   ├── arith.rs
│   │       │   ├── builtin_methods
│   │       │   │   ├── bytes.rs
│   │       │   │   ├── dict.rs
│   │       │   │   ├── list.rs
│   │       │   │   ├── mod.rs
│   │       │   │   ├── prelude.rs
│   │       │   │   ├── set.rs
│   │       │   │   └── string.rs
│   │       │   ├── data.rs
│   │       │   ├── dunder.rs
│   │       │   ├── format.rs
│   │       │   ├── function.rs
│   │       │   ├── methods.rs
│   │       │   ├── methods_helpers.rs
│   │       │   └── mod.rs
│   │       ├── helpers.rs
│   │       ├── init.rs
│   │       ├── mod.rs
│   │       ├── ops.rs
│   │       ├── optimizer.rs
│   │       └── types
│   │           ├── coro.rs
│   │           ├── eq.rs
│   │           ├── err.rs
│   │           ├── math.rs
│   │           └── mod.rs
│   └── util
│       ├── fstr.rs
│       ├── fx.rs
│       └── sha256.rs
└── tests
    ├── cases
    │   ├── lexer.json
    │   ├── packages.json
    │   ├── parser.json
    │   └── vm.json
    ├── common.rs
    ├── lexer.rs
    ├── main.rs
    ├── packages.rs
    ├── parser.rs
    └── vm.rs
```

---

## Quick Start

```bash
# Build the release WebAssembly module, the only artifact this crate distributes.
cargo wasm # -> target/wasm32-unknown-unknown/release/compiler_lib.wasm

# Run the host-side test suite (lexer, parser, VM, packages JSON cases).
cargo test --release
```

`cargo wasm` is a workspace alias (`.cargo/config.toml` at the repo root) for `cargo build --release --target wasm32-unknown-unknown -p edge-python`. Plain `cargo build --release` produces host-side library artifacts (`.rlib` + host cdylib) for embedders linking `compiler_lib` directly. To extend Edge Python with native modules from your own Rust app, depend on `compiler_lib` and implement the `Resolver` trait — see [Writing modules](https://edgepython.com/reference/writing-modules).

Edge Python is loaded by a host runtime, browser via the [`runtime/`](../runtime/) JS package, server / edge via wasmtime / wasmer / Cloudflare Workers / Fastly Compute / Spin. There is no native CLI binary; the host owns I/O, network, and module fetching.

### Consuming the release from another Rust crate

The crate declares `links = "compiler_lib"` and its `build.rs` downloads the matching `compiler_lib.wasm` from the GitHub Release for `CARGO_PKG_VERSION` into `OUT_DIR`. Any downstream crate that depends on this one receives the absolute path through `DEP_COMPILER_LIB_WASM` — cargo's standard `links` metadata channel. No need to invoke `cargo wasm` in the consumer build.

Downstream `Cargo.toml`:

```toml
[dependencies]
edge-python = { git = "https://github.com/dylan-sutton-chavez/edge-python", tag = "v0.1.0" }
```

Downstream `build.rs`:

```rust
fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    let wasm = std::env::var("DEP_COMPILER_LIB_WASM")
        .expect("`DEP_COMPILER_LIB_WASM` unset — upstream `edge-python` must declare `links = \"compiler_lib\"`");

    std::fs::copy(&wasm, "runtime/compiler_lib.wasm").expect("copy failed");
}
```

URL is derived entirely from this crate's `Cargo.toml` (`<repository>/releases/download/v<version>/compiler_lib.wasm`), so a tag bump is the only thing a consumer ever needs to retarget. `branch = "main"` is also valid for unreleased work; pin to a `tag` for reproducible builds. Requires `curl` on the host PATH. The fetch is gated by the default-on `prebuilt` feature; producer-side workspace commands pass `--no-default-features` to skip it.

---

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
