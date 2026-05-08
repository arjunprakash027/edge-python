---
title: "What Edge Python is"
description: "A functional subset of Python 3.13, compiled to bytecode and run on a sandboxed VM."
---

Edge Python is a compiler and stack VM for a **functional subset of CPython 3.13 syntax**. It targets edge computing: deterministic execution, hard sandbox limits, no I/O surface beyond `print`, and a release size around 130 KB compiled to WebAssembly.

The language reads like Python because it parses Python's syntax. It runs differently because what it executes is curated.

## What it supports

- **First-class functions**: pass them, return them, store them in lists and dicts.
- **Lambdas with closures**: full lexical capture, currying, partial application.
- **Generators**: `yield`, `yield from`, generator expressions.
- **Comprehensions**: list, dict, set, with multiple `for` clauses and guards.
- **Pattern matching**: `match` / `case`, including `_` wildcard.
- **Exceptions**: `try` / `except` / `else` / `finally`, with named handlers and `raise from`.
- **Context managers**: `with` blocks, single and multi-target.
- **Numbers**: arbitrary-precision integers (BigInt fallback past 2⁴⁷), IEEE-754 floats.
- **Sequences**: lists, tuples, dicts (insertion-ordered), sets, ranges, strings.
- **f-strings**: with format specs, embedded expressions, and `!r` / `!s` flags.
- **Walrus operator**: `:=` in expressions.
- **Type annotations**: parsed and ignored, like CPython for non-strict tools.
- **Module identity**: `__name__` is bound to `"__main__"` in the entry script and to the module's spec inside imported modules, so the canonical `if __name__ == "__main__":` guard works as expected.
- **Modules**: `import`, `from <spec> import names`, and `from <spec> import *` resolve at compile time through a host-injected `Resolver`. Each module is compiled and initialised once: the parser registers it in the importing chunk's `imports` list, the VM runs every imported module's top level in dependency order before user code starts, and the resulting Module value is shared via a `LoadModule` opcode. Two flavors: `.py` source modules and native modules (`.wasm` binaries loaded by URL per the [WASM ABI](/reference/wasm-abi), or in-process Rust closures for embedders linking `compiler_lib`). See [Imports](/reference/imports) and [Writing modules](/reference/writing-modules).

## What it doesn't support

These parse for syntactic compatibility but raise at runtime, or simply don't exist:

- **Inheritance / MRO**: classes work with `__init__`, attributes, and methods, but there is no base-class chain, no `super`, no method resolution order.
- **Standard library**: there is no bundled stdlib. Modules are external — `.py` files distributed via URL or filesystem, `.wasm` binaries published per the public [WASM ABI](/reference/wasm-abi), or in-process Rust bindings provided by the embedder. See [Imports](/reference/imports) and [Writing modules](/reference/writing-modules).
- **I/O**: `input()` reads from a host-provided buffer. There is no file system, no network, no `os`, no `sys` — *unless* the host registers a native module that provides those capabilities.
- **Async**: `async def` creates real coroutines. `run()` provides cooperative scheduling with `sleep()` and `receive()`.
- **Metaclasses, descriptors, decorators-on-classes, properties**: not modeled.
- **Dynamic code**: no `exec`, no `eval`, no `compile`, no `__import__`.
- **Reflection beyond `type`, `id`, `hash`, `repr`, `callable`, `getattr`, `hasattr`**.

## Design philosophy

Edge Python supports classes with `__init__`, attributes, and methods, but the core paradigm remains functional. Full OO features like inheritance, descriptor protocols, super, slots, and dunder dispatch are omitted to keep the VM small and fast.

A functional core gives Edge Python:

- **A smaller binary**: the entire VM fits in ~130 KB of WebAssembly.
- **A faster interpreter**: no method resolution overhead. Built-ins are first-class `NativeFn` values; user functions are pure `(params, body, defaults, captures)` tuples.
- **Aggressive memoization**: pure functions get their results cached after two hits with the same arguments. Most functional code is pure by construction, so this catches a lot of the cost.
- **Easier sandboxing**: with no class system, the attack surface is the built-in set, which is fixed.

## Sandbox guarantees

The whole Edge Python runtime is a WebAssembly module, so it inherits the sandbox guarantees of the WASM host (no syscalls, no FS, no network, isolated linear memory). On top of that, the embedder can enforce per-VM resource caps via `Limits::sandbox()`:

| Limit              | Default sandbox |
|--------------------|-----------------|
| Max operations     | 100,000,000     |
| Max heap bytes     | 100,000         |
| Max call depth     | 256             |

Hitting any limit raises a recoverable `RuntimeError` / `MemoryError` / `RecursionError` rather than crashing the host process. This matters when you embed the VM as a scripting layer.

## Where it runs

Edge Python ships as a single `.wasm` artifact (`compiler_lib.wasm`, ~130 KB). It runs anywhere WebAssembly does:

- **Browser**: served alongside the [`edge.js`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/demo/edge.js) shim, which bridges `print()` and module imports across the WASM ↔ JS boundary.
- **Server / edge runtimes**: Wasmtime, Wasmer, Cloudflare Workers, Fastly Compute, Spin, etc. The host runtime owns I/O, fetching, and module loading.
- **Embedded Rust apps**: load `compiler_lib.wasm` via your runtime of choice or, when `cargo`-linked, use the `compiler_lib` rlib directly.

The same compiler and the same VM run everywhere. The only host-specific surface is one host import — `env.js_print(ptr, len)` — called on every `print()` for real-time streaming output.