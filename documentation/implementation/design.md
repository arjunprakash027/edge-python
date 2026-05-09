---
title: "Design"
description: "Compiler architecture, dispatch model, and runtime layout."
---

## Overview

Edge Python is a compact bytecode compiler and stack VM for a functional-first subset of Python 3.13. The release build is approximately 130 KB on `wasm32-unknown-unknown` with `panic=abort`, `opt-level=z`, `lto=true`, and `codegen-units=1`. The codebase is organised as a hand-written LUT-driven lexer, a single-pass Pratt parser that emits SSA-versioned bytecode directly, a peephole optimiser for constant folding, and a token-threaded interpreter with two layers of adaptive specialisation on top.

There is no AST and no IR: bytecode is the only intermediate representation between source and execution. The whole compiler is roughly 10,300 lines of Rust; production dependencies are `hashbrown` and `itoa` (SHA-256 is hand-rolled). The WASM build adds `lol_alloc` for a single-threaded leaking bump allocator.

Classes are state containers, not the primary abstraction. Inheritance, descriptor protocols, `super()`, `__slots__`, and dunder dispatch (other than `__init__`) are intentionally omitted to keep the VM small and the dispatch loop fast.

## Concepts

- **Offset-based tokens**: Tokens carry `(kind, line, start, end)` indices into the source buffer. No string copies during lexing; identifier and string content is sliced lazily by the parser.
- **Single-pass SSA codegen**: Variables are versioned per assignment (`x` -> `x_1`, `x_2`). Control-flow joins emit explicit `Phi` opcodes resolved at runtime.
- **Token-threaded dispatch**: The instruction stream is `Vec<Instruction>` where each `Instruction` is `(opcode: OpCode, operand: u16)`. The hot loop is a flat `match` on the opcode variant. Rust lowers it to a jump table; this is *token threading*, not direct threading (computed-goto is not available in safe Rust).
- **Per-instruction inline caching**: Each binary op records the type tags of its operands. After `QUICK_THRESH = 4` stable hits the IC stores a typed `FastOp` (`AddInt`, `AddFloat`, `AddStr`, `LtFloat`, `EqStr`, `ModInt`, ...) used as a speculative fast path with a type-guard deopt that invalidates the slot on miss.
- **Template memoisation**: Pure user functions cache `(args) -> result` after `TPL_THRESH = 2` hits, capped at 256 entries per function, gated on no-kw call and an outer scope that hasn't been observed performing impure ops (`StoreItem`, `StoreAttr`, `Raise`, `Yield`, `Global`, `Nonlocal`, `Import`, ...). Hashing uses an FNV-like fold over raw `Val.0` bits, with a value-eq verification step.
- **NaN-boxed values**: `Val` is a 64-bit union encoding ints (47-bit signed, inline), floats (full IEEE-754 with NaNs canonicalised), bools, None, an undef sentinel, and 28-bit heap indices in a single word.
- **Mark-and-sweep GC**: Triggered when `live >= gc_threshold` or `alloc_count >= max(live/4, 4096)`. After each sweep `gc_threshold = max(live * 2, 512)`. Roots include the stack, with-stack, yields, event queue, slots and live-slot snapshots, slot templates, globals, every iterator frame's `iter_stack`, opcode-cache constants, active const pools, and function templates.

## Bytecode shape

Each `Instruction` is 4 bytes: a 1-byte `OpCode` discriminant (with `#[repr(u8)]` planned), a 2-byte operand, and 1 byte of padding. Opcodes fall into 17 categories — load, store, arith, bitwise, compare, logic, identity, control flow, iter, build, container, comprehension, function, ssa (Phi), yield, side effects, and unsupported (raises at runtime). Roughly 40 specialised `Call*` variants exist for hot builtins, and `LoadAttr + Call(0)` pairs are fused into `CallMethod + CallMethodArgs` after the chunk is first dispatched.

```text
OpCode::LoadConst    operand = constant index
OpCode::LoadName     operand = name slot
OpCode::StoreName    operand = name slot
OpCode::Add / Sub    operand = 0 (IC slot derived from ip)
OpCode::Call         operand = (kw << 8) | pos
OpCode::Phi          operand = target slot, sources in chunk.phi_sources
OpCode::ForIter      operand = jump target on iterator exhaustion
```

## Dispatch shape

The hot loop reads `cache.fused_ref()[ip]` — a snapshot of the instruction stream where adjacent `LoadAttr + Call(0)` pairs have been fused into the `CallMethod + CallMethodArgs` superinstruction. Fusion is performed once per chunk, cached, and reused across calls.

For arithmetic and comparison opcodes, the loop first checks `cache.get_fast(ip)`. If a `FastOp` is present, the speculative path runs inline and pops two operands without a function call. On a type-guard miss the cache is invalidated and execution falls back to the generic handler. The IC is per-instruction, so monomorphic call sites stabilise independently.

`LoadConst` reads a pre-materialised `Vec<Val>` (`OpcodeCache::const_vals`) built once on first dispatch. Integer constants outside the 47-bit range raise `OverflowError` at materialisation, not at run time.

## Memory model

`Val` is 64 bits NaN-boxed (`QNAN = 0x7FFC_0000_0000_0000`, `SIGN = 0x8000…`):

| Tag       | Pattern                                 | Notes                                |
|-----------|-----------------------------------------|--------------------------------------|
| Float     | any non-canonical IEEE-754              | Quiet NaNs remapped to `0x7FF8…`     |
| Int       | `QNAN \| SIGN \| i48`                   | 47-bit signed inline; `OverflowError` above |
| Undef     | `QNAN`                                  | Unbound-local sentinel               |
| None      | `QNAN \| 1`                             |                                      |
| True      | `QNAN \| 2`                             |                                      |
| False     | `QNAN \| 3`                             |                                      |
| Heap      | `QNAN \| 4 \| (i28 << 4)`               | 28-bit index into `HeapPool` (max `1 << 28` slots) |

`INT_MAX = 140_737_488_355_327`, `INT_MIN = -140_737_488_355_328`. The 47-bit cap is architectural: NaN-boxed inline ints turn arithmetic into one ALU op with no boxing, and bigints would either need a `HeapObj::Bignum` variant (heap round-trip on every overflow) or abandoning NaN-boxing entirely (much wider `Val`, slower hot path).

`PartialEq` and `Hash` for `Val` funnel value-equal numerics through `f64` bits so `1 == 1.0` and `hash(1) == hash(1.0)` hold — dicts and sets see them as a single key.

The heap is a `Vec<HeapSlot>` arena with a free list (capped at 524,288 slots and sorted to prefer low indices). String and bytes values up to 128 bytes are interned in side hashes (`strings`, `bytes_intern`) so short literal compares short-circuit through identity. The hard cap on live heap objects comes from `Limits.heap` (default 10M; sandbox 100K). Integer arithmetic stays strictly within ±2⁴⁷; any overflow raises `OverflowError` instead of promoting to a heap variant. The collector is a single-colour mark-and-sweep that runs when `live >= gc_threshold` or `alloc_count >= max(live/4, 4096)`; cycles are reclaimed natively (there is no refcount).

`HeapObj` variants: `Str`, `Bytes`, `List` (`Rc<RefCell<Vec<Val>>>`), `Dict` (insertion-ordered), `Set`, `FrozenSet`, `Tuple`, `Func(fn_idx, defaults, captures)`, `Range`, `Slice`, `Ellipsis` (true singleton, distinct from `'...'`), `Type`, `ExcInstance`, `BoundMethod`, `NativeFn`, `Class(name, members)`, `Instance(class, attrs)`, `BoundUserMethod(recv, fn)`, `Coroutine(ip, slots, stack, fi, iter_stack)` (shared by generators and `async def`), `Module(spec, attrs)`, `Extern(Arc<dyn Fn>)`.

## What the compiler intentionally does *not* do

- No SSA-wide constant propagation through `LoadName`. The load is preserved because removing it pessimises the IC, super-op, and template paths.
- No CSE, GVN, LICM, inlining, branch DCE, or closed-form loop folding. The optimiser is constant folding plus phi-noop elimination plus dead-instruction compaction with jump-operand remap.
- No dead-store elimination beyond what falls out of constant folding.
- No IR — there is exactly one representation between source and dispatch.
- No JIT. Edge Python stays single-tier and pure Rust. Method JITs need per-architecture stencils; trace JITs duplicate the execution model and complicate the GC contract.
- No runtime module system. `import` and `from ... import` resolve at parse time through a host-injected `Resolver`; the VM never learns what a module is. See [Imports](/reference/imports).
- No dunder dispatch (other than `__init__`). Operators dispatch on the value's type tag, not on user-class methods. `__add__`, `__eq__`, `__iter__`, `__enter__`, `__getitem__`, etc. on user classes are never consulted; behaviour reuse is via free functions, not method overriding. `super()` is not registered as a builtin and there is no MRO machinery.
- No bigints, complex numbers, `bytearray`, `memoryview`, `Decimal`, or `Fraction`. No generator `send` / `throw` / `close`. No `asyncio` module — `run`, `sleep`, `gather`, `with_timeout`, `cancel`, `receive` are top-level builtins.

## Architecture

```text
compiler/src/
 ├── lib.rs
 ├── abi.rs       # sealed WASM ABI v1: ops, tags, ErrorKind, HandleTable
 ├── main.rs      # WASM orchestration: parser/VM lifecycle + JS imports (wasm32-only)
 └── modules/
     ├── fstr.rs       # numeric formatter + s!/push!/err! string macros
     ├── fx.rs         # FxHasher + per-map seeded FxBuildHasher
     ├── sha256.rs     # hand-rolled FIPS 180-4 SHA-256 (used by integrity)
     ├── lexer/
     │   ├── mod.rs
     │   ├── scan.rs
     │   └── tables.rs
     ├── packages/
     │   ├── mod.rs
     │   └── manifest.rs  # hand-rolled JSON parser for packages.json
     ├── parser/
     │   ├── mod.rs
     │   ├── stmt.rs
     │   ├── expr.rs
     │   ├── control.rs
     │   ├── literals.rs
     │   ├── imports.rs
     │   └── types.rs
     └── vm/
         ├── mod.rs
         ├── ops.rs
         ├── types.rs
         ├── optimizer.rs
         ├── cache.rs
         ├── builtins.rs
         └── handlers/
             ├── mod.rs
             ├── arith.rs
             ├── data.rs
             ├── format.rs
             ├── function.rs
             └── methods.rs
```

## Capabilities

| Types  | Control flow     | Built-ins         | Lexical         |
|--------|------------------|-------------------|-----------------|
| int    | if / elif / else | I/O               | indentation     |
| float  | for / while      | type conversion   | f-string        |
| str    | match / case     | introspection     | walrus operator |
| bool   | functions        | iteration         | comments        |
| list   | lambdas          | aggregation       | docstrings      |
| dict   | generators       | math              | underscore      |
| tuple  | comprehensions   | sequence ops      | escape sequences|
| set    | try / except     | logical reduction | -               |
| range  | with             | number formatting | -               |
| None   | async / await    | -                 | -               |
|        | yield / yield from | -               | -               |

`async def` and `yield`-bearing `def` both produce a `HeapObj::Coroutine` (one variant covers both). `run()` drives the cooperative scheduler with `sleep()`, `gather()`, `with_timeout()`, `cancel()`, and `receive()` as top-level builtins. There is no `asyncio` module.

`with` is a stack-save scope: `SetupWith` and `ExitWith` save and restore VM state, but they do **not** invoke `__enter__` or `__exit__` on the context-manager value (same for `async with`). For deterministic resource cleanup, use explicit `try` / `finally`.

## References

1. Aho, Sethi & Ullman. *Compilers: Principles, Techniques and Tools* (1986). LUT-based lexer.
2. Pratt. *Top Down Operator Precedence* (POPL 1973).
3. Cytron et al. *Efficiently Computing Static Single Assignment Form* (TOPLAS 1991).
4. Gudeman. *Representing Type Information in Dynamically Typed Languages* (1993). NaN-boxing.
5. Deutsch & Schiffman. *Efficient Implementation of the Smalltalk-80 System* (POPL 1984). Inline caching.
6. Ertl & Gregg. *The Structure and Performance of Efficient Interpreters* (JILP 2003). Threaded dispatch.
7. Casey et al. *Towards Superinstructions for Java Interpreters* (SCOPES 2003). LoadAttr+Call fusion.
8. Michie. *Memo Functions and Machine Learning* (Nature 1968). Pure-function memoization.
9. McCarthy. *Recursive Functions of Symbolic Expressions* (CACM 1960). Mark-sweep GC.
10. Backus. *Can Programming Be Liberated from the von Neumann Style?* (CACM 1978). Function-level paradigm.