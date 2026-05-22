---
title: "What Edge Python is"
description: "A subset of Python, compiled to bytecode and run on a sandboxed VM."
---

Edge Python is a sandboxed Python subset with classes, async/await, structural pattern matching, and `packages.json` imports — compiled to bytecode and run on a stack VM with adaptive inline caching and pure-function memoisation. See [Design](/implementation/design) for the compiler internals.

The language reads like Python because it parses Python syntax. It runs differently because what it executes is curated.

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

Edge Python is **multi-paradigm sandboxed compiler**. Edge Python gives:

- **A smaller binary**: the entire compiler and VM fit in 170 KB of WebAssembly release.
- **A faster interpreter**: no method-resolution overhead; hot opcodes promote to type-specialised fast paths via inline caching.
- **Aggressive memoisation**: pure functions get their results cached automatically — most functional code is pure by construction.
- **Easier sandboxing**: with no protocol dispatch and no stdlib, the attack surface is the fixed built-in set.

## Sandbox guarantees

The whole Edge Python runtime is a WebAssembly module, so it inherits the sandbox guarantees of the WASM host (no syscalls, no FS, no network, isolated linear memory). On top of that, the embedder can enforce per-VM resource caps via `Limits::sandbox()` — hitting any of them raises a recoverable `RuntimeError` / `MemoryError` / `RecursionError` rather than crashing the host process. See [Limits and errors](/reference/limits-and-errors#sandbox-limits) for the full table.

## Where it runs

Edge Python is distributed as a single `.wasm` artifact (`compiler_lib.wasm`, 170 KB). It runs anywhere WebAssembly does:

- **Browser**: served alongside the [`runtime/`](https://github.com/dylan-sutton-chavez/edge-python/tree/main/runtime) JS package, which bridges `print()` and module imports across the WASM ↔ JS boundary.
- **Server / edge runtimes**: Wasmtime, Wasmer, Cloudflare Workers, Fastly Compute, Spin, etc. The host runtime owns I/O, fetching, and module loading.
- **Embedded Rust apps**: load `compiler_lib.wasm` via your runtime of choice or, when `cargo`-linked, use the `compiler_lib` rlib directly.

The same compiler and the same VM run everywhere. Two ABIs sit on top:

- **Compiler↔host imports** — declared by the embedder against the host runtime, covering output, module fetching, native dispatch, and wall-clock time. Custom embedders that ship [host packages](/reference/writing-modules#path-c-host-capability) declare additional imports (e.g. DOM in the browser shim, FS in WASI) without touching the plugin ABI below.
- **Plugin ABI (sealed v1)** — the contract a CDN-distributed `.wasm` plugin module follows when imported via `from "<url>" import`. Exactly 6 `env.*` imports, never extended. See the [WASM module ABI](/reference/wasm-abi).
