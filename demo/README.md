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

> The page pulls the latest published `compiler_lib.wasm` from the GitHub release, so the demo and the compiler ship independently.

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
