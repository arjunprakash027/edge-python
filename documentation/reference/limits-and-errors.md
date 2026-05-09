---
title: "Limits and errors"
description: "Sandbox limits, error types, and runtime guarantees."
---

## Sandbox limits

Edge Python supports two limit profiles. Pick one when constructing the VM via `VM::with_limits` — the host chooses, so the same `compiler.wasm` runs unsandboxed in trusted contexts and clamped in untrusted ones.

| Limit          | `none()` (default) | `sandbox()`   | What hitting it raises |
|----------------|--------------------|---------------|------------------------|
| Max call depth | 1,000              | 256           | `RecursionError`       |
| Max operations | unbounded          | 100,000,000   | `RuntimeError`         |
| Max heap bytes | 10,000,000         | 100,000       | `MemoryError`          |

## Integer width

Edge Python integers are inline 47-bit signed values inside a NaN-boxed `Val`. The range is `±140_737_488_355_327` (`±2^47 - 1`); literal or arithmetic results outside that range raise `OverflowError`.

```python
print(140737488355327)    # max value, fine
try:
    print(2 ** 47)        # 140737488355328 — past the cap
except OverflowError:
    print("overflow")
```

```text Output
140737488355327
overflow
```

This is architectural. Bigint would either need a secondary heap variant (every arith op pays a type check on the overflow path) or abandoning NaN-boxing (slower hot path, larger `Val`). Both regress the WASM-size and inner-loop performance goals that motivated the project.

### Triggering limits

```python
# Recursion depth
def loop(n):
    return loop(n + 1)

try:
    loop(0)
except RecursionError:
    print("hit max depth")
```

```text Output
hit max depth
```

```python
# Heap quota — a tight loop allocating new objects
try:
    xs = []
    while True:
        xs = xs + [0] * 1000
except MemoryError:
    print("hit heap limit")
```

## Source size

The source file must be under **10 MiB**. Larger inputs are rejected at lex time.

## Token limits

| Limit                | Value |
|----------------------|-------|
| Max indent depth     | 100   |
| Max f-string depth   | 200   |
| Max expression depth | 200   |
| Max instructions per chunk | 65,535 |

These prevent pathological asymmetric DoS — a small input that produces an exponentially large parse tree or instruction stream.

## Error types

### Compile-time

Reported as `Diagnostic { start, end, msg }` — `start`/`end` are byte offsets into the source; line and column are computed lazily by `render()` for human-facing output. Caught before any code runs.

| Diagnostic                                | Cause                                  |
|-------------------------------------------|----------------------------------------|
| `expected X, got 'Y'`                     | Unexpected token                       |
| `'(' was never closed` (or `'['` / `'{'`) | Bracket opened with no matching closer |
| `')' does not match '[', expected ']'`    | Wrong closer kind for innermost opener |
| `unexpected ')', no matching opener`      | Closer with no opener on the stack     |
| `unexpected ':' (missing 'if', 'while', 'for', ...)` | `expr:` at statement level  |
| `unterminated string literal`             | String missing closing quote           |
| `unterminated triple-quoted string literal` | Triple-quoted string hit EOF         |
| `f-string was never closed`               | F-string body hit EOF before close     |
| `inconsistent indentation: mixing tabs and spaces` | Indent mixes both whitespace kinds |
| `'break' outside loop`                    | Misplaced control keyword              |
| `'continue' outside loop`                 | Misplaced control keyword              |
| `default 'except:' must be last`          | Bare `except` not at end               |
| `expression too deeply nested`            | Past `MAX_EXPR_DEPTH`                  |
| `program too large: exceeded maximum instruction limit` | Past `MAX_INSTRUCTIONS` |

### Runtime

Raised as `VmErr`. Most are catchable with `try` / `except`.

| Variant         | Class name           | When                               |
|-----------------|----------------------|------------------------------------|
| `Type`          | `TypeError`          | Wrong operand type                 |
| `TypeMsg`       | `TypeError`          | Wrong operand type (with context)  |
| `Value`         | `ValueError`         | Right type, invalid value          |
| `Attribute`     | `AttributeError`     | Attribute not found on object      |
| `Name`          | `NameError`          | Undefined name                     |
| `ZeroDiv`       | `ZeroDivisionError`  | Division or modulo by zero         |
| `Overflow`      | `OverflowError`      | Integer arithmetic past ±2⁴⁷       |
| `Raised("KeyError")`       | `KeyError`         | Dict / set lookup miss          |
| `Raised("IndexError")`     | `IndexError`       | Sequence index out of range     |
| `Raised("StopIteration")`  | `StopIteration`    | Iterator exhausted              |
| `Raised("TimeoutError")`   | `TimeoutError`     | `with_timeout` deadline expired |
| `Raised("CancelledError")` | `CancelledError`   | User-thrown cancellation        |
| `CallDepth`     | `RecursionError`     | Past `max_calls`                   |
| `Heap`          | `MemoryError`        | Past heap limit                    |
| `Budget`        | `RuntimeError`       | Past op limit                      |
| `Runtime`       | `RuntimeError`       | Internal invariant or unsupported  |
| `Raised`        | (custom)             | User `raise X` (X may be a class or string) |

#### Exception hierarchy

The standard exception classes form a flat tree rooted at `BaseException -> Exception`. `except` clauses walk parent links, so `except Exception` catches `RuntimeError`, `ValueError`, `KeyError`, etc., and `except RuntimeError` catches `RecursionError` and `NotImplementedError`.

```python
try:
    raise RuntimeError("oops")
except Exception as e:
    print("caught via parent:", e)

try:
    [][0]
except Exception:
    print("caught IndexError as Exception")
```

```text Output
caught via parent: oops
caught IndexError as Exception
```

User-defined classes do not participate in the hierarchy — they're flat state containers, caught only by their own name or by a bare `except`. `raise X from Y` raises `X`; the cause is currently discarded (no `__cause__` / `__context__` chaining).

### Catching errors

```python
def safe(f, x):
    try:
        return f(x)
    except TypeError:
        return "type"
    except ValueError:
        return "value"
    except ZeroDivisionError:
        return "zero"
    except:
        return "other"

print(safe(lambda x: 1 / x, 0))
print(safe(lambda x: int(x), "abc"))
print(safe(lambda x: len(x), 42))
```

```text Output
zero
value
type
```

### Environmental errors

A small set of failures are surfaced **before** the source reaches the compiler, so they carry no line/column preview — there is no parsed code to anchor to. They are emitted as plain text and cannot be caught from Python.

| Error                                       | When                                          | Resolution                            |
|---------------------------------------------|-----------------------------------------------|---------------------------------------|
| `input rejected: invalid utf-8 at byte N`   | Input bytes from the host are not valid UTF-8 | Re-encode the source as UTF-8         |
| `source file exceeds maximum size (10 MiB)` | Source larger than the 10 MiB lex-time cap    | Split or trim the input               |

These describe a problem with the runtime input, not with your code. Handle them at the embedder layer (file path validation, encoding, size check) before invoking the compiler.

## Unsupported features at runtime

These parse but raise `RuntimeError` when executed.

```python
# Imports
try:
    import os
except RuntimeError as e:
    print("import:", e)
```

These exist for syntactic compatibility — Python source can be loaded without parse errors — but the VM rejects them when reached. For code reuse, use higher-order functions.

## Determinism

For a given source program and input, Edge Python produces the same output across runs and across architectures (`x86_64`, `aarch64`, `wasm32`). There is no time, no randomness, no thread scheduling, no OS interaction. The only source of nondeterminism is the heap pool's slot reuse, which is observable through `id(x)` only — never through `==`, `repr`, or any other operation.