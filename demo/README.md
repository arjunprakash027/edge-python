## Edge Python — WebAssembly Demo for the Edge

Run Edge Python directly in the browser — a sandboxed Python subset with classes, async/await, structural pattern matching, and `packages.json` imports. The compiler is a single-pass SSA bytecode (linear time complexity), VM with adaptive inline caching and pure-function memoization, written in Rust and compiled to WebAssembly.

* **Demo:** *[demo.edgepython.com](https://demo.edgepython.com/)*
* **Docs:** *[edgepython.com](https://edgepython.com/)*

---

## Features

* **In-Browser Execution:** Fully client-side via WebAssembly and a Web Worker (UI thread stays responsive while user code runs).
* **Lightweight Code Editor:** Custom syntax highlighting, line numbering, and auto-indentation built on CodeJar.
* **Small Footprint:** Around 200 KB total payload (HTML + WASM + JS + Tailwind).
* **Fast Boot:** 514 ms Time-To-Interactive on Cloudflare performance tests.

## Local Start

The page fetches the WebAssembly module and uses a Web Worker, so it must be served over HTTP, opening `index.html` via `file://` fails with CORS / fetch errors. I recomend to to initialize the localhost for development using the [Live Server](https://marketplace.visualstudio.com/items?itemName=ritwickdey.LiveServer) visual studio code extension.

> The page pulls the runtime JS and `compiler_lib.wasm` from `cdn.edgepython.com`, so the demo, the runtime, and the compiler all ship and version independently.

### Cache and deploys

The demo no longer sets HTTP cache headers (the old `demo/_headers` was removed); cache invalidation is driven by `demo/version.json` instead. Every CI deploy writes a short content hash of the runtime and WASM into `version.json`:

```json
{ "v": "<12-char hash>" }
```

The page fetches `version.json` with `cache: 'no-store'` on each load, appends `?v=<hash>` to the WASM URL for HTTP-cache busting, and passes the same hash as `version` to `createWorker(...)`. The runtime compares it against the value stored in IndexedDB; on mismatch the IDB CAS and lockfile are wiped before the run, so stale bytes from an old deploy can never be served.

### WASM streaming

The WASM module is fetched **once** in the page (so the network panel shows a single request), and its `ReadableStream` body is transferred to the Worker via `postMessage`. The Worker compiles it with `WebAssembly.compileStreaming`, avoiding the double-fetch + double-decode that the previous main-thread-then-worker path introduced.

### Demo report

The runtime's `format` helper (in `runtime/lib/format.py`) renders class definitions with their inheritance chain and dunder methods, so the perceptron example shows the full surface of Edge Python's object model when the report is generated.

### Project Structure

```bash
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

### License

MIT OR Apache-2.0
