# Edge Python — WebAssembly Demo for the Edge

The interactive playground at [demo.edgepython.com](https://demo.edgepython.com/) — a static page that runs Edge Python entirely client-side via the [`runtime/`](../runtime/) package. See the [docs](https://edgepython.com/) for the language itself.

## Features

* **In-Browser Execution:** Fully client-side via WebAssembly and a Web Worker (UI thread stays responsive while user code runs).
* **Lightweight Code Editor:** Custom syntax highlighting, line numbering, and auto-indentation built on CodeJar.
* **Small Footprint:** Around 200 KB total payload (HTML + WASM + JS + Tailwind).
* **Fast Boot:** 514 ms Time-To-Interactive on Cloudflare performance tests.

## Local Start

The page fetches the WebAssembly module and uses a Web Worker, so it must be served over HTTP — opening `index.html` via `file://` fails with CORS / fetch errors. For development, the [Live Server](https://marketplace.visualstudio.com/items?itemName=ritwickdey.LiveServer) VS Code extension is the easiest way to spin up a local server.

> The page pulls the runtime JS and `compiler_lib.wasm` from `runtime.edgepython.com`, so the demo, the runtime, and the compiler all ship and version independently.

### Cache and deploys

The demo no longer sets HTTP cache headers (the old `demo/_headers` was removed); cache invalidation is driven by `demo/version.json` instead. Every CI deploy writes a short content hash of the runtime and WASM into `version.json`:

```json
{ "v": "<12-char hash>" }
```

The page fetches `version.json` with `cache: 'no-store'` on each load, appends `?v=<hash>` to the WASM URL for HTTP-cache busting, and passes the same hash as `version` to `createWorker(...)`. The runtime compares it against the value stored in IndexedDB; on mismatch the IDB CAS and lockfile are wiped before the run, so stale bytes from an old deploy can never be served.

### WASM streaming

The WASM module is fetched **once** in the page (so the network panel shows a single request), and its `ReadableStream` body is transferred to the Worker via `postMessage`. The Worker compiles it with `WebAssembly.compileStreaming`, avoiding the double-fetch + double-decode that the previous main-thread-then-worker path introduced.

## Demo report

The `format` helper in [`runtime/lib/format.py`](runtime/lib/format.py) renders class definitions with their inheritance chain and dunder methods, so the perceptron example shows the full surface of Edge Python's object model when the report is generated.

## Layout

```text
├── css
│   └── style.css
├── index.html
├── js
│   ├── main
│   │   ├── editor.js
│   │   └── highlighter.js
│   └── main.js
├── README.md
├── runtime
│   ├── lib
│   │   └── format.py
│   └── perceptron.py
├── static
│   ├── album.svg
│   ├── favicon.svg
│   ├── github.svg
│   └── play.svg
├── tailwind.config.js
└── version.json
```

## License

MIT OR Apache-2.0
