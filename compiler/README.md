# Edge Python

A compact, single-pass SSA-style bytecode compiler and stack VM for a functional subset of CPython 3.13 syntax. Hand-written lexer, Pratt-precedence parser that emits bytecode directly (no AST), and a threaded-code interpreter with per-instruction inline caching. Built for deterministic execution in sandboxed and embedded environments (≈ 130 KB WASM release).

* **Demo:** [demo.edgepython.com](https://demo.edgepython.com/)
* **Docs:** [edgepython.com](https://edgepython.com/)

---

## 1. Paradigm

Edge Python targets functional edge computing. The language treats functions as first-class values: lambdas, higher-order functions, currying, closures, comprehensions, and pure-function memoization are all central. Classes are supported with `__init__`, instance attributes, and methods. There is no inheritance and no method resolution order. Imports parse for compatibility but raise at runtime; the VM has no module system.

What this leaves is a small, fast, deterministic core: arithmetic with arbitrary-precision integers, sequences (lists, tuples, dicts, sets, strings, ranges), control flow, lambdas with closures, generators, exceptions, and a curated set of built-in functions exposed as first-class values.

---

## 2. Architecture

* **Lexer**: Hand-written, LUT-driven scanner over CPython 3.13 token kinds. Tokens are `(start, end, kind)` offsets into the source buffer; no string copies during lexing.
* **Parser**: Single-pass, Pratt precedence climbing. Emits SSA-versioned bytecode directly (`x` -> `x_1`, `x_2`) with explicit `Phi` opcodes at control-flow joins. No intermediate AST.
* **Optimizer**: One peephole pass: constant folding over adjacent literal operands, plus dead-code compaction with jump remapping. Does not propagate through `LoadName`.
* **VM**: Stack-based interpreter over `Vec<Instruction>` where each `Instruction` is `(opcode: OpCode, operand: u16)`. Dispatch is a flat `match` on the opcode (Rust lowers it to a jump table). One LoadAttr+Call superinstruction (`CallMethod` + `CallMethodArgs`), fused once per chunk and cached in `cache.fused_ref()`.
* **Inline Caching**: Per-instruction type-recording cache for arithmetic and comparisons. After 4 stable hits the IC stores a `FastOp` (`AddInt`, `LtFloat`, ...) used as a speculative fast path with type-guard deopt.
* **Template Memoization**: Pure functions called repeatedly with the same arguments return cached results after 2 hits, bypassing full execution.
* **Memory**: NaN-boxed 64-bit `Val` (48-bit signed int, IEEE-754 float, bool, None, 28-bit heap index). Mark-and-sweep GC. Arbitrary-precision `BigInt` fallback for integers outside the 48-bit range.

---

## 3. Compiler Design

The store convention is SSA: every assignment increments a per-name version counter and emits a fresh slot. Control-flow joins backup the version maps and emit `Phi` instructions on exit so the runtime can resolve which version is live.

The single optimization pass folds patterns of the form `LoadConst a, LoadConst b, BinOp` into `LoadConst (a OP b)`, plus unary `Not` and `Minus` over constants. It deliberately does **not** fold `LoadName` even when the value is statically known, because keeping the load preserves the IC slot that drives runtime specialization.

What the compiler intentionally does *not* do:

* No SSA-wide constant propagation through `LoadName`.
* No CSE, no GVN, no LICM, no inlining, no closed-form loop folding.
* No dead-store elimination beyond what falls out of constant folding.
* No IR — bytecode is the only representation.
* No module system: `import` and `from ... import` parse but raise at runtime.

---

## 4. Why this dispatch shape

* **Threaded operands** keep dispatch as a flat `match` over a typed enum rather than `(u16 opcode, u16 operand)` tuples. The Rust compiler lowers this to a jump table; this is *token-threading*, not direct-threading (computed-goto is unavailable in safe Rust).
* **Inline caching** records operand type tags per instruction and promotes to a typed `FastOp` after 4 stable hits. The fast path still re-checks types as a deopt guard; on a guard miss the cache invalidates and falls back to the generic handler.
* **Template memoization** caches pure-function results keyed by argument tuple. Functions are marked impure if they touch the heap (`StoreItem`, `StoreAttr`), do I/O (`CallPrint`, `CallInput`), raise, or yield — which fits a functional core well, where most user functions are pure.
* **No JIT.** Edge Python stays single-tier and pure Rust. Method JITs need per-arch stencils; trace JITs duplicate the execution model and complicate the GC contract. Single-tier loses on hot loops but is small, portable across `x86_64` / `aarch64` / `wasm32`, and trivial to embed.

---

## 5. Value Representation

64-bit NaN-boxed `Val`:

| Tag      | Encoding                            | Notes                                |
|----------|-------------------------------------|--------------------------------------|
| Int      | `QNAN \| SIGN \| i48`               | ±2⁴⁷ inline, BigInt above            |
| Float    | IEEE-754 (any non-canonical NaN)    | Quiet NaN remapped to canonical      |
| Bool     | `QNAN \| 2` / `QNAN \| 3`           | `True` / `False`                     |
| None     | `QNAN \| 1`                         |                                      |
| Heap     | `QNAN \| 4 \| i28`                  | 28-bit index into `HeapPool`         |

*BigInt uses a base-2³² limb array with Knuth-D long division. Strings ≤ 64 bytes are interned.*

---

## 6. Garbage Collection

Mark-and-sweep with roots: stack, globals, iterator frames, current slot window, and saved live-slot snapshots. Triggered by a configurable heap threshold inside `HeapPool::alloc`. `Limits` controls hard caps for sandboxed execution: max ops, max heap bytes, max call depth.

---

## 7. Project Structure

```text
├── Cargo.lock
├── Cargo.toml
├── README.md
├── src
│   ├── lib.rs
│   ├── main.rs
│   ├── modules
│   │   ├── fstr.rs
│   │   ├── fx.rs
│   │   ├── lexer
│   │   │   ├── mod.rs
│   │   │   ├── scan.rs
│   │   │   └── tables.rs
│   │   ├── parser
│   │   │   ├── control.rs
│   │   │   ├── expr.rs
│   │   │   ├── literals.rs
│   │   │   ├── mod.rs
│   │   │   ├── stmt.rs
│   │   │   └── types.rs
│   │   └── vm
│   │       ├── builtins.rs
│   │       ├── cache.rs
│   │       ├── handlers
│   │       │   ├── arith.rs
│   │       │   ├── data.rs
│   │       │   ├── function.rs
│   │       │   ├── methods.rs
│   │       │   └── mod.rs
│   │       ├── mod.rs
│   │       ├── ops.rs
│   │       ├── optimizer.rs
│   │       └── types.rs
│   └── wasm.rs
└── tests
    ├── cases
    │   ├── lexer.json
    │   ├── parser.json
    │   └── vm.json
    ├── lexer.rs
    ├── main.rs
    ├── parser.rs
    └── vm.rs
```

---

## 8. Quick Start

```bash
cargo build --release
./target/release/edge -c 'print((lambda x: x * 2)(21))'
./target/release/edge --sandbox script.py
```

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
11. **Knuth**, *The Art of Computer Programming, Vol. 2* (1981). Algorithm D for BigInt division.
12. **Backus**, *Can Programming Be Liberated from the von Neumann Style?* (CACM 1978). Function-level paradigm.