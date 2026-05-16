# Edge Python Runtime

The JavaScript half of Edge Python: hosts `compiler_lib.wasm` in a Web Worker, resolves and registers `.py` / `.wasm` modules, dispatches native calls. The Rust half is the `compiler_lib.wasm` artifact published with each release.

## Install

No install. Import directly from jsdelivr; the tag pins the version:

```js
import { createWorker } from "https://cdn.jsdelivr.net/gh/dylan-sutton-chavez/edge-python@v0.1.0/runtime/src/index.js";
```

For local development against a checkout of this repo, import the relative path:

```js
import { createWorker } from "../../runtime/src/index.js";
```

The standard dev/prod switch pattern picks one or the other based on `location.hostname`.

## Usage

```js
const worker = await createWorker({
    wasmUrl: "./compiler_lib.wasm",
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

### `createWorker(opts)` → `Promise<Worker>`

Spawns a Web Worker, loads `compiler_lib.wasm` inside it, returns a proxy.

| Option | Type | Default | Description |
|---|---|---|---|
| `wasmUrl` | `string` | — | URL of `compiler_lib.wasm`. |
| `integrity` | `boolean` | `true` | When `true`, use IDB + lockfile to cache and verify fetched module bytes. Falls back to in-memory cache (with `console.warn`) if IDB is unavailable. |
| `imports` | `Record<string, string>` | `null` | Bare-name shortcut: maps Python bare names (`from <name> import ...`) to URLs of `.py` / `.wasm` modules. Replaces the need for a physical `packages.json` for simple projects. |
| `loaders` | `string[]` | `[]` | URLs of module loader plugins. Each loader is a `.js` file with a default export `{ match, load }`. See [Writing a loader](#writing-a-loader). |
| `version` | `string` | `null` | Optional lockfile version key. When present, mismatches with the stored version invalidate the cache before run. Useful to pin cache to a deploy/commit. |

### `Worker`

The returned object exposes:

| Member | Type | Description |
|---|---|---|
| `integrityActive` | `boolean` | `true` iff IDB cache opened successfully. Inspect after `createWorker` to detect silent fallback. |
| `loadMs` | `number` | Wall time to load + compile `compiler_lib.wasm`. |
| `run(src, opts?)` | `(string, {entryDir?, baseUrl?}) => Promise<{out, ms}>` | Execute a Python source string. Resolves with stdout (concatenated `print()` lines if no `onOutput`) and wall time. |
| `onOutput(handler)` | `(line: string) => void` | Streaming output callback fired once per `print()` line. |
| `reset()` | `() => Promise<void>` | Clear registered modules without rebooting the worker. |
| `clearCache()` | `() => Promise<void>` | Wipe IDB CAS + lockfile (or memory cache). Next run re-fetches everything. |
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
        // ctx.fetchedSources  — Map of already-fetched spec → bytes
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

See `loaders/capability-bridge.js` for a complete example.

## Architecture

```
├── loaders
│   └── capability-bridge.js
├── README.md
├── src
│   ├── cache
│   │   ├── idb.js
│   │   └── memory.js
│   ├── engine.js
│   ├── env.js
│   ├── fetch.js
│   ├── index.js
│   ├── native.js
│   ├── prefetch.js
│   ├── rt.js
│   └── specs.js
└── worker
    └── worker.js
```

## Files

| Path | Purpose |
|---|---|
| `src/index.js` | Public API. `createWorker` factory (main-thread). |
| `src/engine.js` | Orchestrator (runs in worker). `load`, `run`, `reset`, `clearCache`, `dispose`. |
| `src/env.js` | The 3 `env.*` imports `compiler_lib` declares: `host_print`, `host_call_native`, `host_fetch_bytes`. |
| `src/native.js` | Native module loader extension point + built-in Path A (wasm-pdk) loader + `nativeTable`. |
| `src/prefetch.js` | BFS over the dependency graph; pre-fetches and registers all `.py` / `.wasm` / `packages.json`. |
| `src/fetch.js` | CAS-backed fetch with lockfile integrity check. |
| `src/specs.js` | URL/spec helpers mirroring `compiler_lib::modules::packages::manifest`. |
| `src/rt.js` | Handle codec wrappers (`decodeStr`, `encodeInt`, ...) for loaders. |
| `src/cache/memory.js` | In-memory cache backend (per-Worker only). |
| `src/cache/idb.js` | IndexedDB cache backend (persistent across sessions). |
| `worker/worker.js` | Web Worker entry; postMessage protocol. |
| `loaders/capability-bridge.js` | Path D loader: capability modules with embedded JS bridge. Opt-in. |

## License

MIT OR Apache-2.0
