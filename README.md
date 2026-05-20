# Edge Python Capabilities

Official JS modules for [Edge Python](https://edgepython.com) that expose host APIs (DOM, …) to Python scripts. Each capability is a plain ESM that registers with `createWorker` via `mainThreadModules` — no `.wasm`, no Rust, no custom embedder.

## Layout

```
edge-python-capabilities/
├── dom/
│   ├── src/
│   ├── web/
│   └── README.md
└── static/
```

Each top-level folder is one capability. Planned siblings include `requests/` (networking) and others.

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

## Capabilities

| Folder | Description |
|--------|-------------|
| `dom`  | Browser DOM access — see [`dom/README.md`](dom/README.md) |

## License

MIT OR Apache-2.0
