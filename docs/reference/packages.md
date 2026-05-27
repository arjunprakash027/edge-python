---
title: "Official packages"
description: "The ready-made modules maintained alongside Edge Python, what they are, where they live, and how to import them."
---

Edge Python ships no bundled stdlib (see [What it is](/getting-started/what-it-is)), every module is external. This page is the catalog of the **official, ready-to-use packages** maintained alongside the compiler. You don't have to write these yourself, import them and go. To build your own, see [Writing modules](/reference/writing-modules).

There are two families, matching two of the three [delivery paths](/reference/writing-modules):

| Family | Repo | Form | Where it runs | Path |
|---|---|---|---|---|
| **Standard packages** | [`edge-python-std`](https://github.com/dylan-sutton-chavez/edge-python-std) | `.wasm` (Rust) | Inside the WASM sandbox, any host | [Path A](/reference/writing-modules#path-a-wasm-module-by-url) |
| **Host libraries** | [`edge-python-host`](https://github.com/dylan-sutton-chavez/edge-python-host) | Plain ESM (JS) | Browser main thread | [Path C](/reference/writing-modules#path-c-js-host-module) |

Standard packages are host-agnostic (they run wherever WASM runs). Host libraries are browser-only, they reach `document` / `window` / `localStorage`, surfaces that don't exist in a non-browser host.

## Standard packages (`edge-python-std`)

Language-agnostic `.wasm` plugins over the [WASM module ABI](/reference/wasm-abi). Import by bare name (the browser runtime resolves the official ones by default, see [Defaults](#defaults)), by URL, or via a `packages.json` alias; the host fetches the `.wasm` and treats its exports as native bindings.

### `json`

JSON serialization and deserialization, full CPython `json.loads` / `json.dumps` kwargs parity (`object_hook`, `parse_float`, `indent`, `sort_keys`, `ensure_ascii`, `default`, and more).

```python
from json import dumps, loads

data = loads('{"name":"ada","tags":["math","cs"]}')
print(data["name"]) # ada
print(dumps({"k": [1, 2, 3], "ok": True})) # {"k":[1,2,3],"ok":true}
```

Pre-built `.wasm` is published on the [`edge-python-std` releases](https://github.com/dylan-sutton-chavez/edge-python-std). Full API: [`json/README.md`](https://github.com/dylan-sutton-chavez/edge-python-std/tree/main/json).

> **`json` is an external package, but the browser runtime resolves it by default.** It isn't compiled into `compiler_lib.wasm`, it's this `.wasm` package. In the browser runtime you can write `from json import ...` with no `packages.json` (a built-in [default](#defaults), fetched lazily on first import). Other hosts, or `defaults: false`, need it declared (alias or URL) like any other module.

### `re`

Regular expressions, a CPython `re` subset on a compact backtracking engine. Unicode aware `\d` `\w` `\s` and `(?i)` without shipping Unicode tables, plus capture groups, backreferences, lookahead, and fixed width lookbehind. A step budget raises `RuntimeError` on catastrophic backtracking instead of hanging, so a degrading pattern is reported rather than freezing the worker.

```python
from re import search, sub, findall

print(search(r'(\d+)-(\d+)', 'order 12-34')) # 12-34
print(sub(r'\s+', '_', 'a  b   c')) # a_b_c
print(findall(r'\w+', 'one two three')) # ['one', 'two', 'three']
```

Functions: `match`, `search`, `fullmatch`, `findall`, `groups`, `span`, `sub`; flags go inline (`(?i)`, `(?s)`, `(?m)`). Pre-built `.wasm` is published on the [`edge-python-std` releases](https://github.com/dylan-sutton-chavez/edge-python-std). Full API: [`re/README.md`](https://github.com/dylan-sutton-chavez/edge-python-std/tree/main/re).

### `math`

CPython-style `math`, scalar transcendentals on `libm` (no platform libc) with CPython domain errors (`sqrt(-1)` raises `ValueError: math domain error`). Module constants `pi`, `e`, `tau`, `inf`, `nan`; integer helpers `factorial`, `gcd`, `lcm`, `isqrt`, `comb`, `perm`; tuple returns `modf`, `frexp`; variadic `hypot` and `gcd`. A packed-f64 batch path (`sqrt_all`, `fsum_all`, and friends) processes a whole `bytes` buffer in one host crossing for bulk work.

```python
from math import sqrt, pi, hypot, factorial

print(sqrt(2)) # 1.4142135623730951
print(pi) # 3.141592653589793 (a value, not a call)
print(hypot(3, 4, 12)) # 13.0
print(factorial(5)) # 120
```

Integers are bounded by the VM's `i128`, so `factorial`, `comb`, `perm`, and `lcm` raise `ValueError` past that range, and there is no `complex` / `cmath`. Pre-built `.wasm` is published on the [`edge-python-std` releases](https://github.com/dylan-sutton-chavez/edge-python-std). Full API: [`math/README.md`](https://github.com/dylan-sutton-chavez/edge-python-std/tree/main/math).

## Host libraries (`edge-python-host`)

Plain-JS capabilities that run on the browser's main thread, registered declaratively via the `host` field of [`packages.json`](/reference/imports#packages-json) (with the `<edge-python>` element), programmatically via `createWorker({ hostModules })`, or resolved by default with no config at all (see [Defaults](#defaults)). No `.wasm`, no Rust, no build step. Each call defers to the main thread over `postMessage` (around 0.1 to 0.4 ms); Python sees a synchronous call. The ESM loads lazily, the first time a run imports it.

### `dom`

Full browser DOM surface: queries, mutation, events, forms, files, observers, animations, layout, media, SVG, dialog, fullscreen, pointer lock.

```python
from dom import query, set_text, bind_event

set_text(query("#title"), "Hello")
bind_event(query("#btn"), "click", "clicked")
```

Representative handlers: `query`, `query_all`, `create_element`, `append_child`, `set_text`, `set_html`, `set_attribute`, `add_class`, `set_style`, `rect`, `bind_event`, `form_data`, `validity`, `get_files`, `file_read_text`, `observe_intersection`, `animate`, `media_play`, `show_modal`. Full API: [`dom/README.md`](https://github.com/dylan-sutton-chavez/edge-python-host/tree/main/dom).

### `network`

HTTP fetch, WebSocket, and Server-Sent Events. HTTP calls suspend the coroutine and compose with `gather` / `with_timeout`; WS/SSE stream events through `receive()`.

```python
from network import fetch_text, ws_open, ws_send

body = fetch_text("https://example.com") # suspends until the response arrives
sock = ws_open("wss://example.com/socket", "ws")
ws_send(sock, "hello")
```

Handlers: `fetch`, `fetch_text`, `fetch_json`, `abort_request`, `ws_open`, `ws_send`, `ws_close`, `ws_state`, `sse_open`, `sse_close`, `sse_state`. Full API: [`network/README.md`](https://github.com/dylan-sutton-chavez/edge-python-host/tree/main/network).

### `storage`

Persistent client-side storage: `localStorage`, `sessionStorage`, `IndexedDB`. KV handlers are synchronous; IndexedDB handlers suspend like `network`'s `fetch`.

```python
from storage import local_set, local_get, idb_open, idb_put

local_set("theme", "dark")
print(local_get("theme")) # dark
db = idb_open("notes", 1, '{"stores":["items"]}')
idb_put(db, "items", "1", '{"title":"hello"}')
```

Handlers: `local_get/set/remove/clear/keys`, `session_*` (same surface), `idb_open`, `idb_put`, `idb_get`, `idb_delete`, `idb_keys`, `idb_close`. Full API: [`storage/README.md`](https://github.com/dylan-sutton-chavez/edge-python-host/tree/main/storage).

### `time`

Wall and monotonic clocks, sleep, and calendar formatting, a sandbox-friendly subset of CPython's `time`. Clock reads are synchronous; `sleep` suspends the coroutine like `network`'s `fetch`, composing with `gather` / `with_timeout`. A `struct_time` crosses as a JSON nine-tuple string, decode it with `json` to read fields.

```python
from time import time, sleep, strftime, gmtime

print(time()) # seconds since the epoch
sleep(0.1) # suspends, resumes after ~100ms
print(strftime("%Y-%m-%d", gmtime(0))) # 1970-01-01
```

Handlers: `time`, `time_ns`, `monotonic`, `monotonic_ns`, `perf_counter`, `perf_counter_ns`, `sleep`, `gmtime`, `localtime`, `mktime`, `strftime`, `strptime`, `asctime`, `ctime`, `timezone`, `altzone`, `daylight`, `tzname`. CPU, thread, and POSIX clocks (`process_time`, `clock_gettime`, `tzset`, the `CLOCK_*` constants) are intentionally out of scope. Full API: [`time/README.md`](https://github.com/dylan-sutton-chavez/edge-python-host/tree/main/time).

## How to load them

| You have... | Do |
|---|---|
| Any official package, browser runtime | Just `from <name> import ...`, the runtime resolves the official std/host packages by default. Declare it only to pin a different version, or opt out with `defaults: false` |
| A standard `.wasm` package (e.g., `json`) | Quoted URL `from "https://.../json.wasm" import ...`, or a `packages.json` `imports` alias |
| A host library (e.g., `dom`, `network`, `storage`), `<edge-python>` element | Add it to the `host` field of `packages.json` |
| A host library, programmatic `createWorker` | Pass its URL in `hostModules` (lazy) or an in-memory factory in `mainThreadModules` (eager) |

```json
{
  "imports": { "json": "https://std.edgepython.com/json.wasm" },
  "host": { "dom": "./dom/src/index.js" }
}
```

One manifest drives both directions: `imports` for worker-side `.py` / `.wasm` modules, `host` for main-thread libraries. See [Imports](/reference/imports) for resolution semantics and the full `packages.json` schema, and the [runtime README](https://github.com/dylan-sutton-chavez/edge-python/tree/main/runtime) for `<edge-python>` attributes and `createWorker` options.

### Defaults

The browser runtime ships a built-in base manifest, so the official packages resolve by bare name with **no `packages.json` at all**: the std `.wasm` packages (`json`, `re`, `math`) and the host libraries (`dom`, `network`, `storage`, `time`). Three rules:

- **Lazy.** A default is fetched only when a run actually imports it. Unused defaults never hit the network.
- **Overridable.** Your `packages.json` (or `imports` / `hostModules`) wins for the same name, so you can pin a specific version or URL.
- **Opt-out.** Pass `defaults: false` to `createWorker` to disable the base manifest entirely (e.g. offline or non-browser embedders).

Defaults are a convenience of the browser runtime, not the compiler: `compiler_lib.wasm` stays hermetic and resolves bare names only through the manifest the host provides. Non-browser hosts decide their own defaults, if any.

## See also

- [Imports](/reference/imports), import syntax, `packages.json`, integrity verification.
- [Writing modules](/reference/writing-modules), build your own package (the three delivery paths).
- [WASM module ABI](/reference/wasm-abi), the contract standard `.wasm` packages implement.
