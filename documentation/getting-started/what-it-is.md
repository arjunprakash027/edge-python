---
title: "What Edge Python is"
description: "A functional subset of Python 3.13, compiled to bytecode and run on a sandboxed VM."
---

Edge Python is a compiler and stack VM for a **functional subset of CPython 3.13 syntax**. It targets edge computing: deterministic execution, hard sandbox limits, no I/O surface beyond `print`, and a release size around 130 KB compiled to WebAssembly.

The compiler is a single-pass pipeline — a hand-written lexer, a Pratt-style parser that emits SSA-tagged bytecode with constant folding, and a stack VM with NaN-boxed 64-bit values, inline caches that promote hot opcodes to type-specialised fast paths, and template memoisation for pure calls. Mark-and-sweep garbage collection runs against an arena heap. The whole compiler ships as one `cdylib` with `hashbrown` and `itoa` as the only production dependencies.

The language reads like Python because it parses Python's syntax. It runs differently because what it executes is curated.

## What it supports

- **First-class functions**: pass them, return them, store them in lists and dicts. Decorators apply to both `def` and `class`.
- **Lambdas with closures**: full lexical capture by value-snapshot at `MakeFunction`, currying, partial application.
- **Generators and coroutines**: `yield`, `yield from`, `async def`, `await`. Generator expressions are eagerly materialised to lists; use a `def` with `yield` for true laziness.
- **Comprehensions**: list, dict, and set, with multiple `for` clauses and `if` guards.
- **Pattern matching**: `match` / `case` with literals, captures, OR-patterns, guards, and sequence patterns (one star permitted).
- **Exceptions**: `try` / `except` / `else` / `finally`, named handlers, `raise X from Y` (chain info discarded but `X` is what propagates), and subclass-aware matching (`except Exception` catches `RuntimeError`).
- **Context managers**: `with` blocks, single and multi-target — they save and restore VM state but do not invoke `__enter__` / `__exit__`. Use `try` / `finally` for resource cleanup.
- **Numbers**: signed 47-bit integers (NaN-boxed inline; max `±140_737_488_355_327`, anything larger raises `OverflowError`) and full IEEE-754 floats. No `complex`, `Decimal`, `Fraction`, or arbitrary-precision integers.
- **Sequences**: lists, tuples, dicts (insertion-ordered), sets, frozensets, ranges, strings (UTF-8, codepoint-indexed), and bytes.
- **f-strings**: full grammar — embedded expressions, `{expr=}` self-doc, `!r` / `!s` / `!a` conversions, and format specs covering `s d b o x X f F e E g G n % c` plus fill / align / sign / `#` / `0` / width / `,` / precision.
- **Walrus operator**: `:=` in expressions (Name target only).
- **Type annotations**: parsed and discarded — no runtime `__annotations__`, no enforcement.
- **Module identity**: `__name__` is bound to `"__main__"` in the entry chunk and to the module's spec inside imported modules, so the canonical `if __name__ == "__main__":` guard works as expected.
- **Modules**: `import`, `from <spec> import names`, and `from <spec> import *` (excludes `_`-prefixed names) resolve at parse time through a host-injected resolver. Each module is compiled and initialised once: the parser registers it in the importing chunk's `imports` list, the VM runs every imported module's top level in dependency order before user code starts, and the resulting Module value is shared via the `LoadModule` opcode. Quoted specs may carry a `#sha256-<hex>` integrity fragment that the parser verifies before resolution. Two flavors: `.py` source modules and native modules (`.wasm` binaries loaded by URL per the [WASM ABI](/reference/wasm-abi), or in-process Rust closures for embedders linking `compiler_lib`). See [Imports](/reference/imports) and [Writing modules](/reference/writing-modules).

## What it doesn't support

These parse for syntactic compatibility but raise at runtime, or simply don't exist:

- **Inheritance and protocol dispatch**: classes carry `__init__`, attributes, and methods, but there is no base-class chain, no `super()`, no method resolution order, and no dunder dispatch (`__add__`, `__eq__`, `__iter__`, `__enter__`, `__getitem__`, etc. are never consulted on user instances). Operators dispatch on the built-in type tag, not on user classes.
- **Standard library**: there is no bundled stdlib. Modules are external — `.py` files distributed via URL or filesystem, `.wasm` binaries published per the public [WASM ABI](/reference/wasm-abi), or in-process Rust bindings provided by the embedder. See [Imports](/reference/imports) and [Writing modules](/reference/writing-modules).
- **I/O**: `input()` reads from a host-provided buffer. There is no file system, no network, no `os`, no `sys` — *unless* the host registers a native module that provides those capabilities.
- **Async surface**: `async def` creates real coroutines and the VM runs a cooperative scheduler, but there is no `asyncio` module — primitives are top-level builtins (`run`, `sleep`, `gather`, `with_timeout`, `cancel`, `receive`). Coroutines do not expose `.send()` / `.throw()` / `.close()`.
- **Metaclasses, descriptors, properties, `__slots__`**: not modeled.
- **Dynamic code**: no `exec`, no `eval`, no `compile`, no `__import__` (use the `import_module(name)` builtin to look up an already-imported module by alias).
- **Reflection beyond `type`, `id`, `hash`, `repr`, `callable`, `getattr`, `hasattr`, `vars`, `globals`, `locals`, `isinstance`**. `type(x)` returns a string like `"<class 'int'>"`, not a type object. `issubclass` and `dir` are absent.
- **Relative imports**: `from . import x` is not supported; use the resolver-aware `import` / `from <spec> import` forms.

## Design philosophy

Edge Python is **functional-first**. Classes exist as basic state containers, not as the primary abstraction. Inheritance, descriptor protocols, `super()`, `__slots__`, and dunder method dispatch are intentionally omitted to keep the VM small and fast — behaviour reuse goes through function composition, not method overriding.

A functional core gives Edge Python:

- **A smaller binary**: the entire compiler and VM fit in ~130 KB of WebAssembly.
- **A faster interpreter**: no method resolution overhead. Built-ins are first-class `NativeFn` values; user functions are `(params, body, defaults, captures)` tuples. Hot opcodes promote to type-specialised fast paths (`AddInt`, `LtInt`, `EqStr`, etc.) after four cache hits.
- **Aggressive memoisation**: pure functions get their results cached after two hits with the same arguments. Most functional code is pure by construction, so this catches a lot of the cost.
- **Easier sandboxing**: with no protocol dispatch and no stdlib, the attack surface is the fixed built-in set.

## Sandbox guarantees

The whole Edge Python runtime is a WebAssembly module, so it inherits the sandbox guarantees of the WASM host (no syscalls, no FS, no network, isolated linear memory). On top of that, the embedder can enforce per-VM resource caps via `Limits::sandbox()`:

| Limit              | Default sandbox |
|--------------------|-----------------|
| Max calls          | 256             |
| Max heap bytes     | 100,000         |

Hitting any limit raises a recoverable `RuntimeError` / `MemoryError` / `RecursionError` rather than crashing the host process. This matters when you embed the VM as a scripting layer.

## Where it runs

Edge Python ships as a single `.wasm` artifact (`compiler_lib.wasm`, ~130 KB). It runs anywhere WebAssembly does:

- **Browser**: served alongside the [`edge.js`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/demo/edge.js) shim, which bridges `print()` and module imports across the WASM ↔ JS boundary.
- **Server / edge runtimes**: Wasmtime, Wasmer, Cloudflare Workers, Fastly Compute, Spin, etc. The host runtime owns I/O, fetching, and module loading.
- **Embedded Rust apps**: load `compiler_lib.wasm` via your runtime of choice or, when `cargo`-linked, use the `compiler_lib` rlib directly.

The same compiler and the same VM run everywhere. The only host-specific surface is one host import — `env.js_print(ptr, len)` — called on every `print()` for real-time streaming output.