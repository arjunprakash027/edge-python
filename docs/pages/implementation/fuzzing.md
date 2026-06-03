---
title: "Fuzzing"
description: "Mutation-based fuzzer for the compiler pipeline."
---

## Overview

A mutation-based fuzzer exercising the full compiler pipeline through automated input generation and panic detection. Each iteration mutates a Python source string from the corpus, runs it through the lexer, single-pass SSA emitter, and bytecode VM inside `catch_unwind`, and classifies the outcome. Panics are saved to `crashes/` as reproducible `.py` files.

## Pipeline

```bash
corpus -> mutate -> lex + parse + vm -> catch_unwind -> [crash | new coverage | discard]
```

| outcome       | meaning                                      | action                                 |
|---------------|----------------------------------------------|----------------------------------------|
| `Crash`       | panic anywhere in the pipeline               | save to `crashes/crash_NNNNNN.py`      |
| `ParseErr`    | parser emitted one or more diagnostics       | discard                                |
| `VmErr`       | VM returned a typed error                    | discard                                |
| `Clean(bm)`   | compiled and executed without panic          | admit to corpus if `bm` covers new opcodes |

`ParseErr` and `VmErr` are expected outcomes — typed errors are not bugs. Only an unhandled panic indicates a defect.

## Coverage

LLVM instrumentation is not available at the Rust level without cargo-fuzz. Coverage is approximated via an opcode bitmap: a `u128` where bit $N$ is set if the opcode at discriminant $N$ appears in the compiled chunk or any nested function or class body.

An input is admitted to the corpus only when its bitmap introduces at least one bit not seen in any prior run. This steers mutation toward inputs that reach new opcodes rather than repeating already-covered paths.

## Iteration

Some sstrategies are applied uniformly at random: `byte_flip` (XOR a random byte), `insert_keyword`, `drop_line`, `duplicate_line`, `splice` (join two corpus halves), `inject_boundary` (i64 boundary literals targeting VM overflow), `deep_nest` (100–220 bracket levels, attacks `MAX_EXPR_DEPTH`), `token_shuffle`, `indent_bomb` (50–110 nested `if True:` blocks), and `add_comment`.

## Known Targets

The VM fast path performs integer arithmetic without overflow checks:

```rust
(FastOp::AddInt, Obj::Int(x), Obj::Int(y)) => Obj::Int(x + y),
(FastOp::SubInt, Obj::Int(x), Obj::Int(y)) => Obj::Int(x - y),
(FastOp::MulInt, Obj::Int(x), Obj::Int(y)) => Obj::Int(x * y),
```

The slow path uses checked arithmetic and returns `VmErr`. The `inject_boundary` strategy supplies `i64::MAX` and `i64::MIN` as literals to reach this divergence via the inline cache after eight homogeneous-type hits.

The `[profile.fuzz]` build enables `debug-assertions = true`, which causes Rust to panic on integer overflow rather than wrapping silently. The fuzzer will detect this as a crash.

## Running

```bash
cargo run --bin fuzz --profile fuzz
```

The `fuzz` profile inherits from `release` with two overrides: `panic = "unwind"` so `catch_unwind` intercepts panics, and `debug-assertions = true` to surface overflow in the fast path.

Output is written to stderr every 10 000 iterations:

```txt
[5.3s]  iters=10000  1886/s  crashes=0  corpus=24  new_cov=4
```

Crashes are saved immediately on detection. To reproduce a crash against the standard compiler binary:

```bash
cargo run --bin compiler crashes/crash_000001.py
```

## References

- Mutation-based fuzzing: dl.acm.org/doi/10.1145/96267.96279
- OWASP A04:2021 insecure design: owasp.org/Top10/A04_2021-Insecure_Design
- Address sanitizer: dl.acm.org/doi/10.5555/2342821.2342849
