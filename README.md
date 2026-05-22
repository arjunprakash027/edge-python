# Edge Python Host

Official JS modules for [Edge Python](https://edgepython.com) exposing host APIs (DOM, network, storage) to Python scripts. Each capability is a plain ESM registered with `createWorker` via `mainThreadModules` — no `.wasm`, no Rust, no custom embedder.

## Layout

```
dom/      — src/, web/, tests/, README.md
network/  — src/, web/, tests/, README.md
storage/  — src/, web/, tests/, README.md
static/
```

One folder per capability.

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
