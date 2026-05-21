# Edge Python Network

HTTP, WebSocket, and Server-Sent Events for Edge Python, shipped as a plain ESM module. Python scripts see `network` as an ordinary module.

```python
from network import fetch_json, ws_open, ws_send

# HTTP — yields and resumes when the response arrives. Composes with gather / with_timeout.
data = fetch_json("https://api.example.com/users")

# WebSocket — streaming, push-event pattern.
sock = ws_open("wss://example.com/socket", "msg")
ws_send(sock, "hello")
async def main():
    while True:
        ev = receive()
        if '"type":"message"' in ev:
            print(ev)
```

## Setup

```html
<script type="module">
    import { createWorker } from "https://runtime.edgepython.com/js/src/index.js";
    import { network } from "./src/index.js";

    const worker = await createWorker({
        wasmUrl: "https://runtime.edgepython.com/js/compiler_lib.wasm",
        mainThreadModules: { network },
    });
    await worker.run(await (await fetch("./script.py")).text());
</script>
```

## Quick start

```bash
git clone https://github.com/dylan-sutton-chavez/edge-python-capabilities
cd edge-python-capabilities
python3 -m http.server 8080
```

Open <http://127.0.0.1:8080/network/web/>. No build step.

## API

### Conventions

- **HTTP handlers are yielding host calls.** They return a Promise on the JS side and the runtime parks the coro in `WaitingHostCall` until it resolves — Python sees a sync-looking call that suspends. `gather()`, `with_timeout()`, and `run()` work over them automatically without `await`/`receive()` boilerplate.
- **WebSocket and SSE use the push-event pattern** (same as `dom`'s `bind_event`). Connections are opened with a `msg` tag; every wire event arrives in Python via `receive()` carrying that tag.
- **Handles are integer IDs** — store them, pass them, never compute on them.
- **Options are JSON strings** — `fetch(url, '{"method":"POST","body":"..."}')`. Mirrors `bind_event` and `animate` in `dom`.
- **All response bodies arrive as strings.** For JSON, parse with `json.loads` (declare the module in `packages.json` if you haven't yet — see [the dom README](../dom/README.md#api) for the import map snippet).

### HTTP

```python
from network import fetch, fetch_text, fetch_json, abort_request
import json

# Full response object as a JSON string: {id, ok, status, headers, body}
resp = json.loads(fetch("https://api.example.com/users"))
if resp["ok"]:
    print(resp["body"])

# Convenience helpers: body string only. Raise on non-2xx.
text = fetch_text("https://example.com")
data = json.loads(fetch_json("https://api.example.com/users"))

# POST with options
resp = json.loads(fetch("https://api.example.com/users",
    '{"method":"POST","body":"{\\"name\\":\\"ada\\"}","headers":{"Content-Type":"application/json"}}'))

# Abort an in-flight request by its `id` (returned in the full response object)
abort_request(resp["id"])
```

`fetch`, `fetch_text`, `fetch_json`, `abort_request`.

### Concurrency (free from the scheduler)

```python
# Three requests in parallel; returns when all three are done.
results = gather(
    fetch("https://api.example.com/a"),
    fetch("https://api.example.com/b"),
    fetch("https://api.example.com/c"),
)

# Deadline; raises TimeoutError if the response doesn't arrive in time.
try:
    body = with_timeout(2.0, fetch_text("https://slow.example.com"))
except TimeoutError:
    print("too slow")
```

### WebSocket

```python
from network import ws_open, ws_send, ws_close, ws_state

sock = ws_open("wss://example.com/socket", "ws")

async def main():
    while True:
        ev = receive()
        if '"type":"open"' in ev:
            ws_send(sock, "hello")
        elif '"type":"message"' in ev:
            print(ev)   # {"msg":"ws","type":"message","data":"..."}
        elif '"type":"close"' in ev:
            return

run(main())
```

Payload `type` values: `open`, `message`, `close`, `error`. `message` carries `data` for text frames (binary frames surface `binary: true` only — bidirectional bytes is a future addition).

`ws_open(url, msg, protocols_json?)`, `ws_send(handle, data)`, `ws_close(handle, code?, reason?)`, `ws_state(handle)`.

### Server-Sent Events

```python
from network import sse_open, sse_close

stream = sse_open("/events", "sse")

async def main():
    while True:
        ev = receive()
        if '"type":"message"' in ev:
            print(ev)   # {"msg":"sse","type":"message","data":"...","event_id":"..."}
        elif '"type":"error"' in ev:
            sse_close(stream)
            return

run(main())
```

`sse_open(url, msg, options_json?)`, `sse_close(handle)`, `sse_state(handle)`.

## How it works

`network/src/index.js` is a factory `(ctx) => handlers`, the same shape `dom` uses. Three handler slices (`http`, `ws`, `sse`) close over a shared `state` (handle tables for in-flight requests, sockets, SSE sources) and are merged with `Object.assign`.

The HTTP slice returns **async handlers** (`async (url) => { ... return body; }`). The runtime detects the returned Promise and parks the calling coro in `WaitingHostCall` until it resolves — equivalent to how `sleep()` parks until a deadline. WS/SSE slices return synchronous handlers that wire DOM-style listeners into `ctx.pushEvent`, mirroring `bind_event` exactly.

## Performance

Per-handler cost is one `postMessage` round-trip per call. HTTP handlers add the network latency on top (the dominant cost). For pipelines that do many small same-host requests, prefer one larger request over many small ones — same advice as in plain JS.

## Distribution

This repo serves only the JS sources. `compiler_lib.wasm` and the Edge Python runtime both come from `runtime.edgepython.com` at page load — no vendored copy here, no build step.

## License

MIT OR Apache-2.0
