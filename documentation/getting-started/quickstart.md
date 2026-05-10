---
title: "Quickstart"
description: "Run your first Edge Python program in under a minute."
---

## Run it

Edge Python is distributed as a WebAssembly module. The fastest way to try it is the playground — no install, runs entirely client-side via WebAssembly.

[Open the playground ->](https://demo.edgepython.com)

## Embed it

To run Edge Python in your own host (browser app, server, edge runtime), you need two artifacts:

1. The compiler module: `compiler_lib.wasm` (~153 KB, contains lexer, parser, and stack VM).
2. A loader for your platform — the canonical browser loader is [`demo/edge.js`](https://github.com/dylan-sutton-chavez/edge-python/blob/main/demo/edge.js); WASI hosts wire it up via their runtime's import API.

Build the WASM yourself:

```bash
git clone https://github.com/dylan-sutton-chavez/edge-python
cd edge-python/compiler
cargo wasm
# -> target/wasm32-unknown-unknown/release/compiler_lib.wasm
```

There is no native CLI binary — `compiler_lib.wasm` is the release artifact, and `compiler/src/main/` is gated to `wasm32`. The host runtime owns I/O, network, and module fetching: the guest exposes one entry point (`run`) and calls back through `host_print`, `host_fetch_bytes`, and `host_call_native`. This boundary is what keeps Edge Python sandboxed by construction.

## Your first program

Open the [playground](https://demo.edgepython.com) and paste:

```python
greet = lambda name: f"Hello, {name}!"

for who in ["world", "edge", "python"]:
    print(greet(who))
```

```text Output
Hello, world!
Hello, edge!
Hello, python!
```

## Language overview

Edge Python is a functional subset of Python 3.13. Functions are first-class values; lambdas, currying, higher-order functions, and comprehensions are central. Classes exist as flat state containers — no inheritance, no `super()`, no dunder dispatch.

```python
# First-class functions
ops = [abs, len, str]
print([f(-3) for f in ops])

# Currying with closures
add = lambda x: lambda y: x + y
print(add(3)(4))

# Pure functions are template-memoised after two
# hits with the same arguments
def fib(n):
    if n < 2: return n
    return fib(n - 1) + fib(n - 2)

print(fib(20))
```

```text Output
[3, 2, '-3']
7
6765
```

## Next steps

<CardGroup cols={2}>
  <Card title="What it is" icon="compass" href="/getting-started/what-it-is">
    Scope, paradigm, and what intentionally isn't supported.
  </Card>
  <Card title="Syntax" icon="code" href="/language/syntax">
    Operators, literals, and the language surface.
  </Card>
  <Card title="Built-ins" icon="package" href="/reference/builtins">
    Every built-in function with examples and outputs.
  </Card>
  <Card title="Methods" icon="list" href="/reference/methods">
    String, list, and dict methods.
  </Card>
</CardGroup>