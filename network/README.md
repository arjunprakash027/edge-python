# Edge Python Network

HTTP, WebSocket, and SSE shipped as a plain ESM module. Scripts see `network` as ordinary.

```python
from network import fetch_json, ws_open, ws_send

from "http://std.edgepython.com/json.wasm" import loads

# HTTP, yields and resumes when the response arrives. Composes with gather / with_timeout.
data = fetch_json("https://api.example.com/users")

# WebSocket, streaming, push-event pattern.
sock = ws_open("wss://example.com/socket", "msg")
ws_send(sock, "hello")
async def main():
    while True:
        ev = loads(receive())
        if ev["type"] == "message":
            print(ev["data"])
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

## Testing

Cases live in [`network.json`](network.json) and run through the shared runner at the repo root:

```bash
# One-time setup
deno run -A npm:playwright install chromium

# Run (from repo root)
HOSTCAP=network deno test --allow-all tests/
```

See [`tests/README.md`](../tests/README.md) for the corpus shape.

## API

### Conventions

- **HTTP handlers yield.** They return a Promise on the JS side; the runtime parks the coro in `WaitingHostCall` until it resolves, Python sees a sync-looking call that suspends. `gather()`, `with_timeout()`, and `run()` work over them with no `await`/`receive()`.
- **WebSocket/SSE use push-events** (like `dom`'s `bind_event`). Connections open with a `msg` tag; every wire event arrives via `receive()` as JSON.
- **Handles are integer IDs.**
- **Options are JSON strings**, `fetch(url, '{"method":"POST","body":"..."}')`.
- **Response bodies are strings.** Parse JSON with `json.loads`, `json` is auto-registered by the runtime.

### HTTP

```python
from network import fetch, fetch_text, fetch_json, abort_request
from "http://std.edgepython.com/json.wasm" import loads

# Full response object as a JSON string: {id, ok, status, headers, body}
resp = loads(fetch("https://api.example.com/users"))
if resp["ok"]:
    print(resp["body"])

# Convenience helpers: body string only. Raise on non-2xx.
text = fetch_text("https://example.com")
data = loads(fetch_json("https://api.example.com/users"))

# POST with options
resp = loads(fetch("https://api.example.com/users",
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
from "http://std.edgepython.com/json.wasm" import loads
from network import ws_open, ws_send, ws_close, ws_state

sock = ws_open("wss://example.com/socket", "ws")

async def main():
    while True:
        ev = loads(receive())
        if ev["type"] == "open":
            ws_send(sock, "hello")
        elif ev["type"] == "message":
            print(ev["data"])
        elif ev["type"] == "close":
            return

run(main())
```

Payload `type` values: `open`, `message`, `close`, `error`. `message` carries `data` for text frames (binary frames surface `binary: true` only, bidirectional bytes is a future addition).

`ws_open(url, msg, protocols_json?)`, `ws_send(handle, data)`, `ws_close(handle, code?, reason?)`, `ws_state(handle)`.

### Server-Sent Events

```python
from "http://std.edgepython.com/json.wasm" import loads
from network import sse_open, sse_close

stream = sse_open("/events", "sse")

async def main():
    while True:
        ev = loads(receive())
        if ev["type"] == "message":
            print(ev["data"])
        elif ev["type"] == "error":
            sse_close(stream)
            return

run(main())
```

`sse_open(url, msg, options_json?)`, `sse_close(handle)`, `sse_state(handle)`.

## How it works

`src/index.js` is a factory `(ctx) => handlers` (same shape as `dom`). Three slices (`http`, `ws`, `sse`) close over a shared `state` (handle tables for in-flight requests, sockets, SSE sources) and merge with `Object.assign`.

HTTP slice returns async handlers (`async (url) => { ... return body; }`); the runtime detects the Promise and parks in `WaitingHostCall` until resolved, same shape as `sleep()`. WS/SSE slices return sync handlers wiring DOM-style listeners into `ctx.pushEvent`.

## Performance

Per-handler cost is one `postMessage` round-trip per call; HTTP adds network latency on top. For many small same-host requests, prefer one larger request, same advice as plain JS.

## Distribution

JS sources only, `compiler_lib.wasm` and the runtime load from `runtime.edgepython.com`. No build step.

## License

MIT OR Apache-2.0
