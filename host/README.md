# Edge Python Host

Official JS modules for [Edge Python](https://edgepython.com) exposing host APIs (DOM, network, storage and more) to Python scripts. Each capability is a plain ESM registered with `createWorker` via `mainThreadModules`, no `.wasm`, no Rust, no custom embedder.

## Layout

```
├── dom
│   └── src
├── network
│   └── src
├── storage
│   └── src
├── time
│   └── src
└── tests
```

One folder per capability. Each ships a `<name>/<name>.json` corpus; the shared runner in `tests/` walks for them and drives every case through headless Chromium.

## Usage

```html
<script type="module">
    import { createWorker } from "https://cdn.edgepython.com/runtime/src/index.js";
    import { dom } from "./dom/src/index.js";

    const worker = await createWorker({
        wasmUrl: "https://cdn.edgepython.com/compiler.wasm",
        mainThreadModules: { dom },
    });
    await worker.run(await (await fetch("./script.py")).text());
</script>
```

## Packages

| Folder | Description |
|--------|-------------|
| `dom`     | Browser DOM access, see [`dom/README.md`](dom/README.md) |
| `network` | HTTP fetch, WebSocket, SEE, see [`network/README.md`](network/README.md) |
| `storage` | localStorage, sessionStorage, IndexedDB, see [`storage/README.md`](storage/README.md) |
| `time`    | Clocks, sleep, calendar formatting, see [`time/README.md`](time/README.md) |

## License

MIT OR Apache-2.0
