## Edge Python Demo

Run Edge Python directly in the browser — a sandboxed Python subset with classes, async/await, structural pattern matching, and `packages.json` imports. The compiler is a single-pass SSA bytecode VM with adaptive inline caching and pure-function memoization, written in Rust and compiled to WebAssembly.

* **Demo:** *[demo.edgepython.com](https://demo.edgepython.com/)*
* **Docs:** *[edgepython.com](https://edgepython.com/)*

---

## Features

* **In-Browser Execution:** Fully client-side via WebAssembly and a Web Worker (UI thread stays responsive while user code runs).
* **Lightweight Code Editor:** Custom syntax highlighting, line numbering, and auto-indentation built on CodeJar.
* **Small Footprint:** ~200 KB total payload (HTML + WASM + JS + Tailwind).
* **Fast Boot:** ~514 ms Time-To-Interactive on Cloudflare performance tests.

## Local Start

The page fetches the WebAssembly module and uses a Web Worker, so it must be served over HTTP — opening `index.html` via `file://` fails with CORS / fetch errors.

```bash
python -m http.server 8000
```

Then open http://localhost:8000 in your browser.

> The page pulls the latest published `compiler_lib.wasm` from the GitHub release, so the demo and the compiler ship independently.

### Project Structure

```bash
├── css
│   └── style.css
├── index.html
├── js
│   ├── edge.js
│   ├── main
│   │   ├── editor.js
│   │   └── highlighter.js
│   ├── main.js
│   ├── worker
│   │   ├── fetch.js
│   │   ├── idb.js
│   │   ├── native.js
│   │   ├── prefetch.js
│   │   └── specs.js
│   └── worker.js
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
