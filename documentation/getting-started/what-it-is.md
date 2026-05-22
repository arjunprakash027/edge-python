---
title: "What Edge Python is"
description: "A subset of Python, compiled to bytecode and run on a sandboxed VM."
---

A sandboxed Python subset with classes, async/await, structural pattern matching, and `packages.json` imports — compiled to bytecode and run on a stack VM with adaptive inline caching and pure-function memoisation. See [Design](/implementation/design) for internals.

Reads like Python (parses Python syntax). Runs differently — what it executes is curated.

## What it supports

* **First-class functions**: pass them, return them, store them in lists and dicts. Decorators apply to both `def` and `class`.
* **Lambdas with closures**: full lexical capture by value-snapshot at `MakeFunction`, currying, partial application.
* **Generators and coroutines**: `yield`, `yield from`, `async def`, `await`. Generator expressions are eagerly materialised to lists; use a `def` with `yield` for true laziness.
* **Comprehensions**: list, dict, and set, with multiple `for` clauses and `if` guards.
* **Pattern matching**: `match` / `case` with literals, captures, OR-patterns, guards, and sequence patterns (one star permitted).
* **Exceptions**: `try` / `except` / `else` / `finally`, named handlers, `raise X from Y` (chain info discarded but `X` is what propagates), and subclass-aware matching (`except Exception` catches `RuntimeError`).
* **Context managers**: `with` and `async with` invoke `__enter__` / `__exit__` on the context-manager value; a truthy return from `__exit__` suppresses the raised exception.
* **Protocol dunders**: operator overloading, indexing, iteration, hashing, and `repr` / `str` / `format` dispatch through user-defined dunders — see [Dunders](/language/dunders) for the full matrix.
* **Numbers**: integers up to `±2^127` (auto-promoted past 47 bits; beyond the cap raises `OverflowError`) and full IEEE-754 floats. No `complex`, `Decimal`, `Fraction`, or arbitrary precision beyond 128 bits.
* **Sequences**: lists, tuples, dicts (insertion-ordered), sets, frozensets, ranges, strings (UTF-8, codepoint-indexed), and bytes.
* **f-strings**: full grammar — embedded expressions, `{expr=}` self-doc, `!r` / `!s` / `!a` conversions, and format specs covering `s d b o x X f F e E g G n % c` plus fill / align / sign / `#` / `0` / width / `,` / precision.
* **Walrus operator**: `:=` in expressions (Name target only).
* **Type annotations**: parsed and discarded — no runtime `__annotations__`, no enforcement.
* **Module identity**: `__name__` is bound to `"__main__"` in the entry chunk and to the module's spec inside imported modules, so the canonical `if __name__ == "__main__":` guard works as expected.
* **Modules**: `import`, `from <spec> import names`, and `from <spec> import *` resolve at parse time through a host-injected resolver, with optional `#sha256-<hex>` integrity on URL specs. Two flavors: `.py` source modules and native modules — see [Imports](/reference/imports) for resolution semantics and [Writing modules](/reference/writing-modules) for the four delivery paths.

## What it doesn't support

These parse for syntactic compatibility but raise at runtime, or simply don't exist:

- **Standard library**: no bundled stdlib; every module is external (see **Modules** above).
- **I/O**: `input()` reads from a host-provided buffer. There is no file system, no network, no `os`, no `sys` — those surface only when the host runtime registers them as [host packages](/reference/writing-modules#path-c-host-capability) (the same mechanism behind `print` and `input` themselves).
- **Async surface**: `async def` creates real coroutines and the VM runs a cooperative scheduler, but there is no `asyncio` module; primitives are top-level builtins (`run`, `sleep`, `gather`, `with_timeout`, `cancel`, `receive`). Coroutines do not expose `.send()` / `.throw()` / `.close()`.
- **Metaclasses, descriptor protocol, `__slots__`**: not modeled.
- **Dynamic code**: no `exec`, no `eval`, no `compile`, no `__import__` (use the `import_module(name)` builtin to look up an already-imported module by alias).
- **Reflection beyond `type`, `id`, `hash`, `repr`, `callable`, `getattr`, `hasattr`, `vars`, `globals`, `locals`, `isinstance`**. `type(x)` returns a string like `"<class 'int'>"`, not a type object. `issubclass` and `dir` are absent.
- **Relative imports**: `from . import x` is not supported; use the resolver-aware `import` / `from <spec> import` forms.

## Design philosophy

Multi-paradigm sandboxed compiler:

- **Smaller binary** — compiler + VM in 170 KB WebAssembly release.
- **Faster interpreter** — no method-resolution overhead; hot opcodes promote to type-specialised fast paths via IC.
- **Aggressive memoisation** — pure functions auto-cached; most functional code is pure by construction.
- **Easier sandboxing** — no protocol dispatch, no stdlib; attack surface is the fixed built-in set.

## Sandbox guarantees

Inherits WASM-host guarantees (no syscalls, no FS, no network, isolated linear memory). On top, embedders enforce per-VM caps via `Limits::sandbox()` — hits raise recoverable `RuntimeError` / `MemoryError` / `RecursionError` rather than crashing the host. See [Limits and errors](/reference/limits-and-errors#sandbox-limits).

## Where it runs

Single `.wasm` artifact (`compiler_lib.wasm`, 170 KB), runs anywhere WebAssembly does:

- **Browser**: served alongside the [`runtime/`](https://github.com/dylan-sutton-chavez/edge-python/tree/main/runtime) JS package, which bridges `print()` and module imports across the WASM ↔ JS boundary.
- **Server / edge runtimes**: Wasmtime, Wasmer, Cloudflare Workers, Fastly Compute, Spin. The host runtime owns I/O, fetching, and module loading.
- **Embedded Rust apps**: load `compiler_lib.wasm` via your runtime of choice or use the `compiler_lib` rlib when cargo-linked.

Two ABIs sit on top:

- **Compiler↔host imports** — embedder-declared, covering output, module fetching, native dispatch, wall-clock time. Custom embedders that ship [host packages](/reference/writing-modules#path-c-host-capability) declare additional imports (DOM, FS) without touching the plugin ABI.
- **Plugin ABI (sealed v1)** — contract for CDN-distributed `.wasm` plugin modules. Exactly 6 `env.*` imports, never extended. See the [WASM module ABI](/reference/wasm-abi).
