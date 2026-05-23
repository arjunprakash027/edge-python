# Edge Python Host

Official JS modules for [Edge Python](https://edgepython.com) exposing host APIs (DOM, network, storage) to Python scripts. Each capability is a plain ESM registered with `createWorker` via `mainThreadModules` — no `.wasm`, no Rust, no custom embedder.

## Layout

```
dom/      — src/, dom.json, README.md
network/  — src/, network.json, README.md
storage/  — src/, storage.json, README.md
sandbox/  — shared browser shell + agnostic Deno + Playwright runner
static/
```

One folder per capability. Each ships a `<name>/<name>.json` corpus; the shared sandbox at the repo root walks for them and drives every case through headless Chromium.

## Usage

```html
<script type="module">
    import { createWorker } from "https://runtime.edgepython.com/js/src/index.js";
    import { dom } from "./dom/src/index.js";

    const worker = await createWorker({
        wasmUrl: "https://runtime.edgepython.com/js/compiler_lib.wasm",
        mainThreadModules: { dom },
    });
    await worker.run(await (await fetch("./script.py")).text());
</script>
```

## Packages

| Folder | Description |
|--------|-------------|
| `dom`     | Browser DOM access — see [`dom/README.md`](dom/README.md) |
| `network` | HTTP fetch, WebSocket, SSE — see [`network/README.md`](network/README.md) |
| `storage` | localStorage, sessionStorage, IndexedDB — see [`storage/README.md`](storage/README.md) |

## License

MIT OR Apache-2.0
