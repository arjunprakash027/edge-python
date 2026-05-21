# Edge Python Runtime

The JavaScript half of Edge Python: hosts `compiler_lib.wasm` in a Web Worker, resolves and registers `.py` / `.wasm` modules, dispatches native calls. The Rust half is the `compiler_lib.wasm` artifact published with each release.

## Install

No install. The official CDN serves the runtime and the matching `compiler_lib.wasm` from a single origin, tracking `main`:

```js
import { createWorker } from "https://runtime.edgepython.com/js/src/index.js";
```

For local development against a checkout of this repo, import the relative path:

```js
import { createWorker } from "../../runtime/src/index.js";
```

## Usage

```js
const worker = await createWorker({
    wasmUrl: "https://runtime.edgepython.com/js/compiler_lib.wasm",
    integrity: true, // default: IDB + lockfile CAS
    imports: { dom: "./dom.wasm" }, // bare-name shortcut, optional
    loaders: [], // opt-in module loaders, optional
});

worker.onOutput((line) => console.log(line));

const { out, ms } = await worker.run(`
from dom import query, set_text
set_text(query("#app"), "hello")
`);

worker.dispose();
```

## API

### `createWorker(opts)` -> `Promise<Worker>`

Spawns a Web Worker, loads `compiler_lib.wasm` inside it, returns a proxy.

| Option | Type | Default | Description |
|---|---|---|---|
| `wasmUrl` | `string` | — | URL of `compiler_lib.wasm`. |
| `integrity` | `boolean` | `true` | When `true`, use IDB + lockfile to cache and verify fetched module bytes. Falls back to in-memory cache (with `console.warn`) if IDB is unavailable. |
| `imports` | `Record<string, string>` | `null` | Bare-name shortcut: maps Python bare names (`from <name> import ...`) to URLs of `.py` / `.wasm` modules. Replaces the need for a physical `packages.json` for simple projects. |
| `loaders` | `string[]` | `[]` | URLs of module loader plugins. Each loader is a `.js` file with a default export `{ match, load }`. See [Writing a loader](#writing-a-loader). |
| `mainThreadModules` | `Record<string, factory \| object>` | `{}` | Synthetic native modules whose handlers run on the main thread. Each entry registers `from <name> import ...` for Python. See [Main-thread modules](#main-thread-modules). |
| `version` | `string` | `null` | Optional lockfile version key. When present, mismatches with the stored version invalidate the cache before run. Useful to pin cache to a deploy/commit. |

### `Worker`

The returned object exposes:

| Member | Type | Description |
|---|---|---|
| `integrityActive` | `boolean` | `true` iff IDB cache opened successfully. Inspect after `createWorker` to detect silent fallback. |
| `loadMs` | `number` | Wall time to load + compile `compiler_lib.wasm`. |
| `run(src, opts?)` | `(string, {entryDir?, baseUrl?}) => Promise<{out, ms}>` | Execute a Python source string. The runtime does not auto-invoke `main` — scripts that define `async def main()` must drive it themselves with a trailing `run(main())`. Top-level scripts (no `main`) execute under the implicit module-body coroutine, so `receive()`, `sleep()`, etc. still work without wrapping. `entryDir` is a prefix joined to relative import specs; `baseUrl` overrides the base for URL resolution (defaults to the worker's `location.href`). Resolves with stdout (concatenated `print()` lines if no `onOutput`) and wall time. |
| `onOutput(handler)` | `(line: string) => void` | Streaming output callback fired once per `print()` line. |
| `reset()` | `() => Promise<void>` | Clear registered modules without rebooting the worker. |
| `clearCache()` | `() => Promise<void>` | Wipe IDB CAS + lockfile (or memory cache). Next run re-fetches everything. |
| `pushEvent(message)` | `(string) => void` | Wake a paused `receive()` in the running script with `message`. Fire-and-forget. Browser bridges fire `CustomEvent("edge-python-event")` on `window`, which `createWorker` routes through `pushEvent` automatically. |
| `dispose()` | `() => void` | Terminate the worker. Subsequent calls fail. |

## Writing a loader

A loader is a `.js` file with a default export:

```js
export default {
    /** Inspect the compiled module; return true if this loader handles it. */
    match(module) {
        const names = WebAssembly.Module.exports(module).map(e => e.name);
        return names.includes('my_marker_export');
    },

    /** Load the module and return its callable surface. */
    async load(module, ctx) {
        // ctx.compilerExports  — compiler_lib.wasm instance exports (wasm_alloc, host_edge_*, etc.)
        // ctx.rt  — handle codec helpers (decodeStr, encodeInt, ...)
        // ctx.fetchedSources  — Map of already-fetched spec -> bytes
        // ctx.loaders  — full loader list (in order)

        return {
            kind: 'wasmpdk' | 'capability',
            names: ['fn1', 'fn2', ...],
            fns: [fn1Impl, fn2Impl, ...],
        };
    },
};
```

Two valid `kind` values:

- **`wasmpdk`** — each `fn` is a wasm export with signature `(g_argv, argc, g_out) -> i32` reading from its own linear memory. Each fn must be annotated with `__edge_alloc` and `__edge_memory` (the built-in loader does this automatically). The dispatcher stages argv in guest memory and copies the result handle back.

- **`capability`** — each `fn` is a plain JS function `(handles: number[]) => number` taking u32 handles in compiler_lib's memory and returning a u32 result handle. The dispatcher calls it directly without staging.

The built-in Path A wasm-pdk loader is always tried last as fallback; custom loaders run first in order.

## Main-thread modules

The engine runs in a Web Worker, so handlers don't have access to `document`, `window`, or other main-thread-only globals. `mainThreadModules` solves that: a pure-JS module declares its handlers, the runtime synthesizes the native registration so Python can `from <name> import ...`, and each call defers to the main thread transparently. Python sees a regular synchronous call — no `await`, no event-loop juggling.

A module is a factory `(ctx) => handlers` or a plain `{name: handler}` object. The factory form receives `{ pushEvent }` so async callbacks (events, observers, file reads) can wake a paused `receive()` in the script.

```js
const dom = ({ pushEvent }) => {
    const nodes = [];
    const alloc = (n) => { nodes.push(n); return nodes.length - 1; };
    const node = (h) => nodes[h];

    return {
        query: (sel) => alloc(document.querySelector(sel)),
        set_text: (h, txt) => { node(h).textContent = txt; },
        bind_event: (h, type, msg) => {
            node(h).addEventListener(type, (e) => {
                pushEvent(JSON.stringify({ msg, type: e.type, target_id: e.target.id }));
            });
        },
    };
};

const worker = await createWorker({
    wasmUrl: "...",
    mainThreadModules: { dom },
});
```

```python
from dom import query, set_text, bind_event

bind_event(query("#btn"), "click", "click")
async def main():
    while True:
        receive()
        set_text(query("#btn"), "clicked")
```

Supported handle tags for shuttle: `None`, `bool`, `int` (i64, range-limited by JS Number), `float`, and string bytes. Opaque object references (DOM nodes, file objects, observers) should be modelled as integer IDs into a main-thread registry that your handlers own — the `alloc` / `node` pattern above.

Per-call overhead is a single `postMessage` round-trip (~0.1–0.4 ms in modern browsers). Suitable for UI-rate workloads (events, mutations, layout). For tight per-frame loops over thousands of fine-grained ops, prefer a Worker-side capability (Path A `.wasm`).

## Worker bootstrap

When the runtime is served from a different origin than the page (the common case: page on `demo.edgepython.com`, runtime on `runtime.edgepython.com`), Chromium rejects `new Worker(crossOriginUrl)` even with `type: 'module'`. `createWorker` works around this by spawning the Worker from a same-origin **Blob URL** that dynamically `import()`s the real cross-origin module. Same-origin imports use the direct path. No flag, no opt-in — `createWorker` picks the right strategy from `import.meta.url`.

The Blob bootstrap also buffers any `postMessage` that arrives before the imported `worker.js` installs its `onmessage` handler, so the initial `load` request can never be lost to a race.

## Module fetch lifecycle

`load` is called once per Worker; `run` can be called many times. The `compiler_lib.wasm` module is compiled once at `load` time and a **fresh instance** is created on each `run`, so VM state cannot leak between runs.

Module **source bytes** (`.py` / `.wasm` / `packages.json`) are cached across runs in the same Worker — the BFS prefetch skips specs it already fetched, and 404'd `packages.json` paths are remembered in a known-missing set so they aren't re-probed on every Run-button press. Use `clearCache()` to drop both caches and force a clean re-fetch.

## Layout

```
├── README.md
├── src
│   ├── cache
│   │   ├── idb.js
│   │   └── memory.js
│   ├── env.js
│   ├── fetch.js
│   ├── index.js
│   ├── native.js
│   ├── prefetch.js
│   ├── rt.js
│   └── specs.js
└── worker
    ├── engine.js
    └── worker.js
```

## Files

| Path | Purpose |
|---|---|
| `src/index.js` | Public API. `createWorker` factory (main-thread). |
| `worker/engine.js` | Internal orchestrator (Worker only). `load`, `run`, `pushEvent`, `reset`, `clearCache`, `dispose`, `setHostCallDelegate`. |
| `src/env.js` | The 4 `env.*` imports `compiler_lib` declares: `host_print`, `host_call_native`, `host_fetch_bytes`, `host_now_ns`. |
| `src/native.js` | Native module loader extension point + built-in Path A (wasm-pdk) loader + `nativeTable`. |
| `src/prefetch.js` | BFS over the dependency graph; pre-fetches and registers all `.py` / `.wasm` / `packages.json`. |
| `src/fetch.js` | CAS-backed fetch with lockfile integrity check. |
| `src/specs.js` | URL/spec helpers mirroring `compiler_lib::modules::packages::manifest`. |
| `src/rt.js` | Handle codec wrappers (`decodeStr`, `encodeInt`, ...) for loaders. |
| `src/cache/memory.js` | In-memory cache backend (per-Worker only). |
| `src/cache/idb.js` | IndexedDB cache backend (persistent across sessions). |
| `worker/worker.js` | Web Worker entry; postMessage protocol. |

## License

MIT OR Apache-2.0
