# Edge Python, WebAssembly Demo

Static playground at [demo.edgepython.com](https://demo.edgepython.com/), runs Edge Python fully client-side via the [`runtime/`](../runtime/) package. Docs: [edgepython.com](https://edgepython.com/).

## Features

* Fully client-side via WebAssembly + Web Worker (UI stays responsive).
* Lightweight editor with custom highlighting, line numbers, auto-indent (built on CodeJar).
* Around 200 KB total payload (HTML + WASM + JS + Tailwind).
* 514 ms Time-To-Interactive (Cloudflare benchmark).

## Local start

The page fetches WASM and uses a Web Worker, so it must be served over HTTP (`file://` fails with CORS). [Live Server](https://marketplace.visualstudio.com/items?itemName=ritwickdey.LiveServer) is the easiest dev option. Runtime JS and `compiler_lib.wasm` pull from `runtime.edgepython.com`, demo, runtime, and compiler version independently.

### Cache and deploys

No HTTP cache headers, invalidation runs through `demo/version.json`. Each CI deploy writes a short content hash:

```json
{ "v": "<12-char hash>" }
```

Page fetches `version.json` with `cache: 'no-store'`, appends `?v=<hash>` to the WASM URL, and passes the hash to `createWorker(...)`. Runtime compares against IndexedDB; on mismatch wipes the IDB CAS + lockfile before running.

### WASM streaming

The WASM module is fetched once on the page (single network request); its `ReadableStream` body transfers to the Worker via `postMessage`. The Worker compiles with `WebAssembly.compileStreaming`, avoids the double-fetch + double-decode of the prior main-thread-then-worker path.

## Demo report

`format` in [`runtime/lib/format.py`](runtime/lib/format.py) renders class definitions with inheritance chain and dunders, the perceptron example shows the full object-model surface.

## Layout

```text
index.html, tailwind.config.js, version.json
css/style.css
js/main.js + js/main/{editor.js, highlighter.js}
runtime/{perceptron.py, lib/format.py}
static/*.svg
```

## License

MIT OR Apache-2.0
