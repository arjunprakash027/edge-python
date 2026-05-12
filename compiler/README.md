# Edge Python

A compact, single-pass SSA-style bytecode compiler and stack VM for a functional subset of Python 3.13 syntax. Hand-written lexer, Pratt-precedence parser that emits bytecode directly (no AST), and a threaded-code interpreter with per-instruction inline caching, super-instruction fusion, and pure-function template memoization. Built for deterministic execution in sandboxed and embedded environments (≈ 153 KB WASM release).

* **Demo:** [demo.edgepython.com](https://demo.edgepython.com/)
* **Docs:** [edgepython.com](https://edgepython.com/)

---

## 1. Paradigm

Edge Python targets functional edge computing. The language treats functions as first-class values: lambdas, higher-order functions, closures, comprehensions, decorators (including class decorators), generators, async/await, pattern matching, and pure-function memoization. Classes support single-level inheritance, `super()`, dunder protocol dispatch (operators, indexing, iteration, context managers, hashing, etc.), and `@property` / `@x.setter`.

`import` and `from <spec> import names` resolve at compile time through a host-injected resolver (see `modules/packages/`, manifest = `packages.json`). Each module is compiled and initialised once: the parser registers it in the importing chunk's `imports` list, the VM runs every imported module's top level in dependency order, and importers reach the resulting `HeapObj::Module` value via `OpCode::LoadModule`. Native modules dispatch via `CallExtern` for fast call-site fusion. Quoted specs may carry a `#sha256-<hex>` integrity fragment.

What this leaves is a small, fast, deterministic core: 47-bit inline integers + IEEE-754 floats, sequences (list, tuple, dict, set, frozenset, str, bytes, range), control flow, exceptions, generators and coroutines (with a top-level cooperative scheduler — `run` / `sleep` / `gather` / `with_timeout` / `cancel` — instead of an `asyncio` module), and a curated set of built-in functions exposed as first-class values.

---

## 2. Architecture

* **Lexer**: Hand-written, LUT-driven scanner (`modules/lexer/{mod,scan,tables}.rs`) over Python 3.13 token kinds. Tokens are `(start, end, kind)` offsets into the source buffer; no string copies during lexing. Indentation tracked as INDENT/DEDENT pairs against an explicit stack; UTF-8 BOM stripped.
* **Parser**: Single-pass, Pratt precedence climbing (`modules/parser/`). Emits SSA-versioned bytecode directly (`x` -> `x_1`, `x_2`, ...) with explicit `Phi` opcodes at control-flow joins. No intermediate AST.
* **Optimizer**: One peephole pass (`modules/vm/optimizer.rs`): constant folding over adjacent literal arithmetic / comparison / unary operands, Phi-noop elimination, and dead-instruction compaction with jump-operand remapping. Deliberately leaves `LoadName` alone to preserve the inline-cache slot.
* **VM**: Stack-based interpreter over `Vec<Instruction>`, where each `Instruction` is `(opcode: OpCode, operand: u16)`. The hot loop lives in `modules/vm/dispatch.rs` as a flat `match` on the opcode (Rust lowers it to a jump table); the VM struct and constructor live in `modules/vm/mod.rs`, with `init.rs` / `helpers.rs` / `gc.rs` covering module init, stack/iter primitives, and the collector. The hot path is split across handler modules (`handlers/{arith,data,format,function,methods,methods_helpers,mod}.rs`). `LoadAttr + Call(0)` is fused into a `CallMethod` / `CallMethodArgs` super-instruction at first execution and cached per call site.
* **Inline Caching**: Per-instruction type-recording cache (`modules/vm/cache.rs`) for arithmetic and comparisons. After 4 stable hits the IC promotes the slot to a typed `FastOp` (`AddInt`, `AddFloat`, `LtFloat`, `EqStr`, ...); the fast path keeps a type-tag guard so a miss falls back to the generic handler.
* **Template Memoization**: Pure functions called with the same arguments return a cached result after 2 hits, bypassing full execution. Functions are tagged impure on first observed side effect (`StoreItem`, `StoreAttr`, `print`, `input`, `raise`, `yield`).
* **Memory**: NaN-boxed 64-bit `Val` (47-bit signed inline int, IEEE-754 float, bool, None, 28-bit heap index). Heap is an arena of `HeapObj` slots managed by a mark-and-sweep GC. Strings and bytes ≤ 128 bytes are interned. **Integers are 47-bit inline with automatic i128 (`LongInt`) promotion on overflow**, hard-capped at ±2^127.

---

## 3. Compiler Design

The store convention is SSA: every assignment increments a per-name version counter and emits a fresh slot. Control-flow joins back up the version maps and emit `Phi` instructions on exit so the runtime can resolve which version is live. Synthetic temps (`#cmp`, `#match`, `#match_item`) carry compiler-generated values across compare-chain and pattern-match desugaring.

The optimizer folds patterns of the form `LoadConst a, LoadConst b, BinOp` into `LoadConst (a OP b)` for arithmetic, comparison, and bitwise ops, plus unary `Not` and `Minus` over a constant. It deliberately does **not** fold `LoadName` even when the value is statically known, because the load is what carries the inline-cache slot that drives runtime specialization.

What the compiler intentionally does *not* do:

* No SSA-wide constant propagation through `LoadName`.
* No CSE, no GVN, no LICM, no inlining, no loop unrolling.
* No dead-branch elimination beyond what falls out of folding.
* No IR — bytecode is the only representation.
* No bundled stdlib: `import`, `from ... import`, and `from ... import *` resolve at compile time through a host-injected resolver (`modules/packages/`, manifest is `packages.json` — never `edge.json`). Each module compiles to its own `SSAChunk` and runs once during `vm.init_modules` (invoked by the WASM `run` entry point before user code dispatches). The resulting `HeapObj::Module` value is registered in `vm.module_table` keyed by canonical spec; `OpCode::LoadModule` is an O(1) lookup so every importer sees the same module instance. Native imports register in `chunk.extern_table` for fast `CallExtern` dispatch.

---

## 4. Why this dispatch shape

* **Threaded operands** keep dispatch as a flat `match` over a typed enum rather than `(u16 opcode, u16 operand)` tuples. The Rust compiler lowers this to a jump table; this is *token-threading*, not direct-threading (computed-goto is unavailable in safe Rust).
* **Inline caching** records operand type tags per instruction and promotes to a typed `FastOp` after 4 stable hits. The fast path still re-checks types as a deopt guard; on a guard miss the cache invalidates and falls back to the generic handler.
* **Template memoization** caches pure-function results keyed by argument tuple. Functions are marked impure if they touch the heap (`StoreItem`, `StoreAttr`), do I/O (`CallPrint`, `CallInput`), raise, or yield — which fits a functional core well, where most user functions are pure.
* **No JIT.** Edge Python stays single-tier and pure Rust. Method JITs require per-architecture stencils; trace JITs duplicate the execution model and complicate the GC contract. Single-tier dispatch is slower on hot loops but remains compact, portable across `x86_64` / `aarch64` / `wasm32`, and straightforward to embed.

---

## 5. Value Representation

64-bit NaN-boxed `Val` (`QNAN = 0x7FFC_0000_0000_0000`):

| Tag      | Encoding                            | Notes                                              |
|----------|-------------------------------------|----------------------------------------------------|
| Int      | `QNAN \| SIGN \| i47`               | 47-bit signed; overflow raises `OverflowError`     |
| Float    | IEEE-754 (any non-canonical NaN)    | All NaNs canonicalised to `0x7FF8_…`               |
| Bool     | `QNAN \| TAG_TRUE / TAG_FALSE`      |                                                    |
| None     | `QNAN \| TAG_NONE`                  |                                                    |
| Heap     | `QNAN \| TAG_HEAP \| i28`           | 28-bit index into `HeapPool` (max 1 << 28 slots)   |

*Strings and bytes ≤ 128 bytes are interned, so `"abc" is "abc"`. There is no bignum or arbitrary-precision integer path: 47-bit ints are the architectural ceiling and exist as a single ALU instruction in the hot path.*

---

## 6. Garbage Collection

Mark-and-sweep with roots: operand stack, with-stack, pending yields, event queue, current slot window, saved live-slot snapshots, globals, every iterator frame, opcode-cache constants, active const pools, and template memoization entries. The threshold starts at 512 live slots and is recomputed `(live * 2).max(512)` after each sweep, capped by `Limits.heap` (default 10M slots, sandbox profile 100K). The free list is capped at 524,288 entries and kept sorted to prefer low indices, which keeps recently-released slots hot in cache. Cycles are reclaimed natively — there is no refcount layer to leak through. `Limits` also caps call depth (1000 default / 256 sandbox) and call count.

---

## 7. Project Structure

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
│   │       │   ├── data.rs
│   │       │   ├── dunder.rs
│   │       │   ├── format.rs
│   │       │   ├── function.rs
│   │       │   ├── methods_helpers.rs
│   │       │   ├── methods.rs
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

## 8. Quick Start

```bash
# Build the release WebAssembly module — the only artifact this crate distributes.
cargo wasm
# -> target/wasm32-unknown-unknown/release/compiler_lib.wasm

# Run the host-side test suite (lexer, parser, VM, packages JSON cases).
cargo test --release
```

`cargo wasm` is a workspace alias (`.cargo/config.toml` at the repo root)
for `cargo build --release --target wasm32-unknown-unknown -p edge-python`.
Plain `cargo build --release` produces host-side library artifacts (`.rlib`
+ host cdylib) for embedders linking `compiler_lib` directly. To extend
Edge Python with native modules from your own Rust app, depend on
`compiler_lib` and implement the `Resolver` trait — see
[Writing modules](../documentation/reference/writing-modules.md).

Edge Python is loaded by a host runtime — browser via `demo/edge.js`, server / edge via wasmtime / wasmer / Cloudflare Workers / Fastly Compute / Spin. There is no native CLI binary; the host owns I/O, network, and module fetching.

---

## 9. References

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
