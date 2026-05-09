## Edge Python Demo

Run Edge Python — a functional subset of CPython 3.13 syntax — directly in the browser. Edge Python is a single-pass SSA bytecode compiler with adaptive inline caching and pure-function template memoization, written in Rust and compiled to WebAssembly.

* **Demo:** *[demo.edgepython.com](https://demo.edgepython.com/)*
* **Docs:** *[edgepython.com](https://edgepython.com/)*

---

## Features

* **In-Browser Execution:** Fully client-side via WebAssembly and a Web Worker (UI thread stays responsive while user code runs).
* **Lightweight Code Editor:** Custom syntax highlighting, line numbering, and auto-indentation using CodeJar.
* **Small Footprint:** Total release payload around 100 KB (HTML + WASM + JS + Tailwind).
* **Fast Boot:** ~514 ms Time-To-Interactive on Cloudflare performance tests.

## Local Start

Because the page fetches the WebAssembly module and uses a Web Worker, you need to serve it through a local HTTP server — opening `index.html` over `file://` will fail with CORS / fetch errors.

```bash
python -m http.server 8000
```

Then open http://localhost:8000 in your browser.

> The page pulls the latest published `compiler_lib.wasm` from the GitHub release, so the demo and the compiler can ship independently.

### Project Structure

```bash
├── index.html
├── main.js
├── packages.json
├── README.md
├── static
│   └── {resource}.svg
├── style.css
├── tailwind.config.js
├── version.json
└── worker.js
```

### License

MIT OR Apache-2.0