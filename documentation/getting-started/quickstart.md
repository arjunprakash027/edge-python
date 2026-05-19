---
title: "Quickstart"
description: "Run your first Edge Python program in under a minute."
---

## Run it

Edge Python is distributed as a WebAssembly module with a 170 KB size. The fastest way to try it is the playground; no install, runs entirely client-side via WebAssembly.

[Open the playground ->](https://demo.edgepython.com)

## Embed it

To run Edge Python in your own host (browser app, server, edge runtime), you need two artifacts:

1. The compiler module: `compiler_lib.wasm` (170 KB, contains lexer, parser, and stack VM).
2. A loader for your platform — the canonical browser loader is the [`runtime/`](https://github.com/dylan-sutton-chavez/edge-python/tree/main/runtime) package; WASI hosts wire it up via their runtime's import API.

Build the WASM yourself:

```bash
git clone https://github.com/dylan-sutton-chavez/edge-python
cd edge-python/compiler
cargo wasm # -> target/wasm32-unknown-unknown/release/compiler_lib.wasm
```

Rust crate consumers can let cargo fetch the release artifact via `DEP_COMPILER_LIB_WASM` (see the repo README). There is no native CLI binary — `compiler_lib.wasm` is the release artifact, and the host owns I/O, network, time, and module fetching. The full compiler↔host ABI is documented under [What it is — Where it runs](/getting-started/what-it-is#where-it-runs).

## Your first program

Open the [playground](https://demo.edgepython.com) and try the SimplePerceptron Rosenblatt implementation or try a Python snippet:

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

Edge Python is a Python subset with classes, async/await, structural pattern matching, and `packages.json` imports — compiled to bytecode and run on a sandboxed WebAssembly VM.

```python
# First-class functions
ops = [abs, len, str]
print([f(-3) for f in ops])

# Currying with closures
add = lambda x: lambda y: x + y
print(add(3)(4))

# Pure functions are template-memoised after two hits with the same arguments (no decorators needed, this is detected by the VM)
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
