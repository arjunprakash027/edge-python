# Edge Python Capabilities

Official `.wasm` capability packages for [Edge Python](https://edgepython.com). Each capability is a self-contained module that embeds its host-side bridge code (JS) and exposes it to Python through the capability protocol — no custom embedder, no client-side glue. Capabilities compose at load time, by URL.

## Layout

```
edge-python-capabilities/
├── Cargo.toml          # workspace root
├── dom/                # DOM bindings for browser interaction
│   ├── Cargo.toml
│   ├── src/
│   └── web/            # demo page
└── target/             # shared build output (gitignored)
```

Each top-level folder is one capability and one workspace member. Planned siblings include `requests/` (networking) and others.

## Build

Requires Rust with the `wasm32-unknown-unknown` target.

```bash
cargo build --release
```

Run from the workspace root. Per-capability artifacts land in `target/wasm32-unknown-unknown/release/`, e.g. `edge_python_dom.wasm`.

To build a single capability:

```bash
cargo build --release -p edge-python-dom
```

## Capabilities

| Folder | Crate              | Description                                  |
|--------|--------------------|----------------------------------------------|
| `dom`  | `edge-python-dom`  | Browser DOM access — see [`dom/README.md`](dom/README.md) |

## License

MIT OR Apache-2.0
