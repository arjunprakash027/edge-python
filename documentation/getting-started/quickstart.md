---
title: "Quickstart"
description: "Run your first Edge Python program in under a minute."
---

## Run it

Edge Python ships as a 170 KB WebAssembly module. Fastest way to try it, the playground, no install, fully client-side.

[Open the playground ->](https://demo.edgepython.com)

## Embed it

Two artifacts:

1. `compiler_lib.wasm` (170 KB, lexer, parser, stack VM).
2. A loader. Browser: the [`runtime/`](https://github.com/dylan-sutton-chavez/edge-python/tree/main/runtime) package; WASI: your runtime's import API.

Build yourself:

```bash
git clone https://github.com/dylan-sutton-chavez/edge-python
cd edge-python/compiler
cargo wasm # -> target/wasm32-unknown-unknown/release/compiler_lib.wasm
```

Rust consumers can let cargo fetch the release artifact via `DEP_COMPILER_LIB_WASM` (see the repo README). No native CLI, `compiler_lib.wasm` is the artifact and the host owns I/O, network, time, module fetching. Full ABI: [What it is, Where it runs](/getting-started/what-it-is#where-it-runs).

### Drop-in HTML element

In the browser, the runtime's `<edge-python>` element runs a `.py` file declaratively, no JS wiring. With a host library like the DOM (declared in `packages.json`), the script renders straight into the page:

```html
<div id="app"></div>

<script type="module" src="https://runtime.edgepython.com/js/src/element.js"></script>
<edge-python entry="./hello.py" packages="./packages.json"></edge-python>
```

```json
// packages.json
{ "host": { "dom": "./dom/src/index.js" } }
```

```python
# hello.py
from dom import query, set_text
set_text(query("#app"), "Hello from Python")
```

`dom` is the reference [host library](https://github.com/dylan-sutton-chavez/edge-python-host), served as JS sources alongside your app. See the [runtime README](https://github.com/dylan-sutton-chavez/edge-python/tree/main/runtime) for all attributes and the `imports` field for `.py` / `.wasm` modules.

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

Edge Python is a Python subset with classes, async/await, structural pattern matching, and `packages.json` imports, compiled to bytecode and run on a sandboxed WebAssembly VM.

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
