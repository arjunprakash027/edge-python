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
<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <script type="module" src="https://runtime.edgepython.com/js/src/element.js"></script>
</head>
<body>
    <div id="app"></div>
    <edge-python entry="./app/hello.py" packages="./app/packages.json"></edge-python>
</body>
</html>
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

`dom` is one of the official [host libraries](/reference/packages#host-libraries) (`dom`, `network`, `storage` and more); standard `.wasm` packages like [`json`](/reference/packages#json) sit alongside them. The `packages.json` above declares `dom` explicitly, but the browser runtime also resolves the official packages by bare name with no manifest at all (see [Defaults](/reference/packages#defaults)), fetching each lazily on first import. See [Official packages](/reference/packages) for the full catalog, and the [runtime README](https://github.com/dylan-sutton-chavez/edge-python/tree/main/runtime) for all `<edge-python>` attributes and the `imports` field for `.py` / `.wasm` modules.

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
