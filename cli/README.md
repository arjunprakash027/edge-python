# Edge Python CLI

The Edge Python developer CLI. Write `.py`, run it, serve it, test it, ship it.

You never compile anything: `edge` hosts the Edge Python runtime in a headless browser it downloads on first use, then runs your code against it. You just point it at a file.

```bash
edge run app.py # run a script
edge serve # dev server with live reload
edge repl # interactive shell
edge test # run your *_test.py files (not implemented yet)
edge init my-app # scaffold a project
edge add network # manage packages.json
edge build # bundle to dist/
```

## Install

```bash
# Prebuilt binary (recommended)
curl -fsSL https://dylan-sutton-chavez.github.io/edge-python/install.sh | sh

# Or from source (any platform with a Rust and Cargo tools)
cargo install --path cli
```

`install.sh` drops the binary at `~/.local/bin/edge` and appends that directory to your `~/.bashrc` or `~/.zshrc` if it is not already on `PATH`. Open a new shell (or `source` the file it printed) and `edge --version` should work. Re-run the same `curl … | sh` line any time to upgrade.

The first command that needs a browser downloads a known-good Chromium into the cache automatically. Non-x86_64 platforms (aarch64, ARM, Apple Silicon) need a system Chrome or `EDGE_CHROME_PATH` set; see [Running on non-x86_64](#running-on-non-x86_64).

---

## Run a Python File

Run a script and stream its output to the terminal. Imports resolve through `packages.json`; uncaught errors print a traceback to stderr and exit with code 1.

```text
$ edge run hello.py
Hello from Edge Python
the sum is 42
```

```text
$ edge run broken.py
before
error: ZeroDivisionError: division by zero
  --> <input>:2:1
  |
2 | x = 1 / 0
  | ^
```

Flags: `--packages <file>` (custom manifest). Reads from stdin when no file is given.

---

## Start an Edge Python Shell (Demo)

An interactive Edge Python shell for quick experiments.

```text
$ edge repl
Edge Python 0.1.0  ·  .exit, Ctrl+C or Ctrl+D to quit
>>> from math import sqrt, pi
>>> print(sqrt(2))
1.4142135623730951
>>> print([n * n for n in range(5)])
[0, 1, 4, 9, 16]
>>> .exit
```

History (arrow keys) and multi-line blocks (a line ending in `:` continues until a blank line) are supported. `.exit` or `Ctrl+C` quits; `Ctrl+D` also exits; `.reset` wipes the accumulated session. Expression results are not auto-printed; use `print()` explicitly.

State is preserved by **recompiling and rerunning the accumulated session on every prompt**. The runtime resets its VM on each `run_start`, so imports and definitions only persist by replay. Trade-offs: side effects (`time()`, `random()`, network, IO) re-fire on every input, the runtime's chunk heap grows linearly with session length, and each eval pays the recompile cost. A first-class incremental compile path in the VM is the proper fix and tracked for a future runtime change; for long sessions or side-effect-heavy code, prefer `edge run` on a script.

*Under development, this is just a demo because actual cost is o(n^2)*

---

## Setup a Local Server for your Browser App

A dev server for browser apps. Serves your project directory and reloads the page on any file change via an injected polling client.

```text
$ edge serve
  http://localhost:5173
  watching .
```

Flags: `--port <n>` (default `5173`), `--open` (open the browser).

---

## Edge Python Test

Not implemented yet.

---

## Initialize a Workspace

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

## Packages Manager

Manage `packages.json` by name. `edge` knows the official std (`json`, `re`, `math`) and host (`dom`, `network`, `storage`, `time`) packages, so you do not paste URLs.

```text
$ edge add math network
  + math       std
  + network    host

  updated packages.json
```

```text
$ edge remove network
  - network

  updated packages.json
```

Point a package at a custom URL with `edge add foo=https://example.com/foo.wasm`.

---

## Build an Application to Create a Portable Version

Bundles your app into a self-contained `dist/` for offline use or self-hosting: the runtime, the `compiler.wasm`, your scripts, and every package vendored locally so nothing is fetched at runtime.

```text
$ edge build
  bundled to dist/

  13 runtime files + compiler.wasm
  2 packages
  3 scripts

  1.24 MB · 5.3s
```

Flags: `--out <dir>` (default `dist/`).

---

## Global flags

| Flag | Effect |
|------|--------|
| `--packages <file>` | Use a specific manifest instead of `./packages.json` |
| `--no-color` | Disable colored output |
| `--version` / `-V` | Print version |

`Ctrl+C` cancels any running command cleanly.

## Running on non-x86_64

The bundled Chromium fetcher only ships x86_64 builds. On aarch64, ARM, or Apple Silicon, either install a system Chrome/Chromium (one of `chromium`, `google-chrome`, `microsoft-edge` on `PATH`) or set `EDGE_CHROME_PATH=/path/to/chrome` before `edge run` / `edge repl` / `edge build`.

## How it runs (the short version)

`edge` never asks you to compile. It downloads a known-good Chromium on first use, serves the Edge Python runtime alongside your code, and runs everything in that headless browser, streaming output back to your terminal. `edge serve` opens the same setup in your own browser for development.

The Edge Python runtime does the actual work; `edge` is the loop around it.
