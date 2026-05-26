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

Language-agnostic `.wasm` plugins over the [WASM module ABI](/reference/wasm-abi). Import by URL or via a `packages.json` alias; the host fetches the `.wasm` and treats its exports as native bindings.

### `json`

JSON serialization and deserialization, full CPython `json.loads` / `json.dumps` kwargs parity (`object_hook`, `parse_float`, `indent`, `sort_keys`, `ensure_ascii`, `default`, and more).

```python
from "https://std.edgepython.com/json.wasm" import dumps, loads

data = loads('{"name":"ada","tags":["math","cs"]}')
print(data["name"]) # ada
print(dumps({"k": [1, 2, 3], "ok": True})) # {"k":[1,2,3],"ok":true}
```

Or with a `packages.json` alias so scripts can write the bare name:

```json
{
  "imports": { 
    "json": "https://std.edgepython.com/json.wasm" 
  } 
}
```

```python
from json import dumps, loads
```

Pre-built `.wasm` is published on the [`edge-python-std` releases](https://github.com/dylan-sutton-chavez/edge-python-std). Full API: [`json/README.md`](https://github.com/dylan-sutton-chavez/edge-python-std/tree/main/json).

> **`json` is not built-in.** Examples elsewhere in these docs write `from json import ...` for brevity, but `json` is this external package, you must declare it (alias or URL) like any other module.

### `re`

Regular expressions, a CPython `re` subset on a compact backtracking engine. Unicode aware `\d` `\w` `\s` and `(?i)` without shipping Unicode tables, plus capture groups, backreferences, lookahead, and fixed width lookbehind. A step budget raises `RuntimeError` on catastrophic backtracking instead of hanging, so a degrading pattern is reported rather than freezing the worker.

```python
from "https://std.edgepython.com/re.wasm" import search, sub, findall

print(search(r'(\d+)-(\d+)', 'order 12-34')) # 12-34
print(sub(r'\s+', '_', 'a  b   c')) # a_b_c
print(findall(r'\w+', 'one two three')) # ['one', 'two', 'three']
```

Functions: `match`, `search`, `fullmatch`, `findall`, `groups`, `span`, `sub`; flags go inline (`(?i)`, `(?s)`, `(?m)`). The same `packages.json` alias trick lets scripts write the bare `from re import ...`. Pre-built `.wasm` is published on the [`edge-python-std` releases](https://github.com/dylan-sutton-chavez/edge-python-std). Full API: [`re/README.md`](https://github.com/dylan-sutton-chavez/edge-python-std/tree/main/re).

## Host libraries (`edge-python-host`)

Plain-JS capabilities that run on the browser's main thread, registered declaratively via the `host` field of [`packages.json`](/reference/imports#packages-json) (with the `<edge-python>` element) or programmatically via `createWorker({ mainThreadModules })`. No `.wasm`, no Rust, no build step. Each call defers to the main thread over `postMessage` (around 0.1 to 0.4 ms); Python sees a synchronous call.

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
| A standard `.wasm` package (e.g., `json`) | Quoted URL `from "https://.../json.wasm" import ...`, or a `packages.json` `imports` alias |
| A host library (e.g., `dom`, `network`, `storage`), `<edge-python>` element | Add it to the `host` field of `packages.json` |
| A host library, programmatic `createWorker` | Pass it in `mainThreadModules` |

```json
{
  "imports": { "json": "https://std.edgepython.com/json.wasm" },
  "host": { "dom": "./dom/src/index.js" }
}
```

One manifest drives both directions: `imports` for worker-side `.py` / `.wasm` modules, `host` for main-thread libraries. See [Imports](/reference/imports) for resolution semantics and the full `packages.json` schema, and the [runtime README](https://github.com/dylan-sutton-chavez/edge-python/tree/main/runtime) for `<edge-python>` attributes and `createWorker` options.

## See also

- [Imports](/reference/imports), import syntax, `packages.json`, integrity verification.
- [Writing modules](/reference/writing-modules), build your own package (the three delivery paths).
- [WASM module ABI](/reference/wasm-abi), the contract standard `.wasm` packages implement.
