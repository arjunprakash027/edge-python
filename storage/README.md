# Edge Python Storage

Persistent client-side storage for Edge Python — `localStorage`, `sessionStorage`, and `IndexedDB`. Shipped as a plain ESM module that registers with `createWorker`.

```python
from storage import local_set, local_get, idb_open, idb_put, idb_get
import json

local_set("theme", "dark")
print(local_get("theme"))   # → "dark"

db = idb_open("notes", 1, '{"stores":["items"]}')
idb_put(db, "items", "1", '{"title":"hello"}')
note = json.loads(idb_get(db, "items", "1"))
print(note["title"])        # → "hello"
```

## Setup

```html
<script type="module">
    import { createWorker } from "https://runtime.edgepython.com/js/src/index.js";
    import { storage } from "./src/index.js";

    const worker = await createWorker({
        wasmUrl: "https://runtime.edgepython.com/js/compiler_lib.wasm",
        mainThreadModules: { storage },
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

Open <http://127.0.0.1:8080/storage/web/>. No build step.

## API

### Conventions

- **Key-value handlers are sync.** `localStorage` and `sessionStorage` are blocking by spec; handlers return strings or `None`. No `await`, no `receive()`.
- **IndexedDB handlers are async (yielding host calls).** They return a Promise on the JS side; the runtime parks the calling coro in `WaitingHostCall` until it resolves. Python sees a sync-looking call that suspends, identical to `fetch()` in [`network/`](../network/README.md).
- **Values cross as JSON strings.** For `idb_put` / `idb_get`, encode/decode with `json.dumps` / `json.loads`. Storing structured objects directly would require crossing arbitrary types over the worker boundary — JSON is the same trade-off `dom`'s `animate` and `bind_event` make for options.
- **Key listings are JSON arrays, not CSV.** `local_keys()` / `session_keys()` / `idb_keys(...)` return a JSON-array string (because keys can contain commas). Parse with `json.loads`.
- **Handles are integer IDs** for IndexedDB; `local_*` / `session_*` need no handle (the global stores are addressed directly).

### localStorage / sessionStorage

```python
from storage import local_set, local_get, local_remove, local_clear, local_keys
import json

local_set("theme", "dark")
local_set("user", "ada")
print(local_get("theme"))           # → "dark"
print(local_get("missing"))         # → None
print(json.loads(local_keys()))     # → ["theme", "user"]
local_remove("user")
local_clear()
```

`local_get`, `local_set`, `local_remove`, `local_clear`, `local_keys`.

Same surface for `sessionStorage` with the `session_` prefix: `session_get`, `session_set`, `session_remove`, `session_clear`, `session_keys`. Difference is lifetime — sessionStorage clears when the tab closes; localStorage persists.

### IndexedDB

```python
from storage import idb_open, idb_put, idb_get, idb_delete, idb_keys, idb_close
import json

# Schema declares the object stores to create on first open / version bump.
db = idb_open("notes", 1, '{"stores":["items","tags"]}')

idb_put(db, "items", "1", json.dumps({"title": "hello", "ts": 1234}))
item = json.loads(idb_get(db, "items", "1"))
keys = json.loads(idb_keys(db, "items"))

idb_delete(db, "items", "1")
idb_close(db)
```

`idb_open`, `idb_put`, `idb_get`, `idb_delete`, `idb_keys`, `idb_close`.

### Concurrency (free from the scheduler)

Because IndexedDB handlers yield, they compose with the rest of the runtime's async primitives:

```python
# Parallel reads from multiple stores
items, tags = gather(idb_keys(db, "items"), idb_keys(db, "tags"))

# Deadline
try:
    item = with_timeout(0.5, idb_get(db, "items", "1"))
except TimeoutError:
    print("slow disk?")
```

## How it works

`storage/src/index.js` is a factory `() => handlers`, the same shape `dom` and `network` use. Two handler slices (`kv`, `idb`) close over a shared `state` (just a handle table for open IDBDatabase instances) and are merged with `Object.assign`.

The KV slice returns **synchronous handlers** that call `localStorage` / `sessionStorage` directly. The IDB slice returns **async handlers** that promisify each native `IDBRequest`; the runtime detects the Promise return and parks the coro until it resolves.

## License

MIT OR Apache-2.0
