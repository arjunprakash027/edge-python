# `edge`

The Edge Python developer CLI. Write `.py`, run it, serve it, test it, ship it.

You never compile anything: `edge` hosts the Edge Python runtime in a headless browser it downloads on first use, then runs your code against it. You just point it at a file.

```bash
edge run app.py # run a script
edge serve # dev server with live reload
edge test # run your *_test.py files
edge init my-app # scaffold a project
edge add network # manage packages.json
```

## Install

```bash
# Prebuilt binary (recommended)
curl -fsSL https://edgepython.com/install.sh | sh

# Or from source
cargo install --path cli
```

The first command that needs a browser downloads a known-good Chromium into the cache automatically. Nothing else to set up.

---

## `edge run <file.py>`

Run a script and stream its output to the terminal, like `python app.py`. Imports resolve through `packages.json`; uncaught errors print a traceback and exit non-zero.

```text
$ edge run hello.py
Hello from Edge Python
the sum is 42
```

```text
$ edge run broken.py
Traceback (most recent call last):
  broken.py:3  in <module>
    print(1 / 0)
ZeroDivisionError: division by zero
```

Flags: `--packages <file>` (custom manifest), trailing args are passed to the script, reads from stdin when no file is given.

---

## `edge repl`

An interactive Edge Python shell for quick experiments.

```text
$ edge repl
Edge Python 0.1.0  ·  type .exit to quit
>>> from math import sqrt, pi
>>> sqrt(2)
1.4142135623730951
>>> [n * n for n in range(5)]
[0, 1, 4, 9, 16]
>>> .exit
```

History and multi-line blocks (functions, loops) are supported.

---

## `edge serve`

A dev server for browser apps, the ones that use `dom`, events, `network`, and friends. It serves your `index.html` + `<edge-python>` + scripts and reloads the page the moment you save.

```text
$ edge serve
  http://localhost:5173   ready in 238ms
  watching ./
```

```text
# after editing main.py:
  main.py changed, reload
```

Flags: `--port <n>`, `--open` (open the browser), `--no-reload`.

---

## `edge test [path]`

Runs your test files (`*_test.py`, or a `tests/` directory). Tests register with the `@test` decorator from the built-in `test` module; `edge` runs each one in isolation and reports per test, continuing past failures. The `test` module is provided by `edge`, so you never add it to `packages.json`.

```python
# math_test.py
from test import test, expect, raises
from math import sqrt, factorial

@test
def sqrt_of_square():
    expect(sqrt(16)).eq(4.0)
    expect(sqrt(2)).gt(1.41)

@test
def factorial_base():
    expect(factorial(5)).eq(120)

@test
def negative_raises():
    with raises(ValueError):
        sqrt(-1)
```

`expect(x).eq(y)` (and `.ne` / `.gt` / `.lt` / `.truthy()`) report both values on failure; `with raises(Exc):` asserts the error. Tests run against the runtime in the same headless browser as `edge run`.

```text
$ edge test
  math_test.py

  sqrt_of_square
  factorial_base
  negative_raises

  3 passed   0.04s
```

On failure, only the failing test is marked, with the assertion and both sides:

```text
$ edge test
  math_test.py

  sqrt_of_square
  factorial_base   failed
    expect(factorial(5)).eq(120)
    math_test.py:12   720 != 120
  negative_raises

  2 passed · 1 failed   0.18s
```

Flags: `--filter <substr>` (run a subset), `--watch` (rerun on change), `--bail` (stop at first failure).

---

## `edge init [name]`

Scaffolds a ready-to-run project: an entry script, an HTML host page, and a manifest.

```text
$ edge init my-app
  created my-app/
    ├─ index.html
    ├─ main.py
    └─ packages.json

  next:
    cd my-app && edge serve
```

`--bare` skips `index.html` for script-only projects.

---

## `edge add <pkg>...`  ·  `edge remove <pkg>...`

Manage `packages.json` by name. `edge` knows the official std (`json`, `re`, `math`) and host (`dom`, `network`, `storage`, `time`) packages, so you do not paste URLs.

```text
$ edge add math network
  + math      std
  + network   host

  updated packages.json
```

```text
$ edge remove network
  - network

  updated packages.json
```

Pin a version or a custom URL with `edge add math@0.2.0` or `edge add foo=https://example.com/foo.wasm`.

---

## `edge build`

Bundles your app into a self-contained `dist/` for offline use or self-hosting: the runtime, the `compiler.wasm`, your scripts, and every package vendored locally so nothing is fetched at runtime.

```text
$ edge build
  bundling to dist/

  runtime + compiler.wasm
  2 packages   math, network
  3 scripts

  dist/   1.24 MB
```

Flags: `--out <dir>` (default `dist/`), `--minify`.

---

## Global flags

| Flag | Effect |
|------|--------|
| `--packages <file>` | Use a specific manifest instead of `./packages.json` |
| `--quiet` / `-q` | Only program output, no `edge` chrome |
| `--no-color` | Disable colored output |
| `--version` / `-V` | Print version |

## How it runs (the short version)

`edge` never asks you to compile. It downloads a known-good Chromium on first use, serves the Edge Python runtime alongside your code, and runs everything in that headless browser, streaming output back to your terminal. `edge serve` opens the same setup in your own browser for development.

The Edge Python runtime does the actual work; `edge` is the loop around it.
