# Edge Python DOM

DOM access shipped as a plain ESM module. Scripts see `dom` as ordinary — full surface: queries, mutation, events, forms, files, observers, animations, layout, media, SVG, dialog, fullscreen, pointer lock.

```python
from dom import query, set_text, bind_event

async def main():
    bind_event(query("#btn"), "click", "click")
    n = 0
    while True:
        receive()  # payload string; ignore content for a simple counter
        n += 1
        set_text(query("#btn"), f"clicked {n} times")
```

## Setup

```html
<script type="module">
    import { createWorker } from "https://runtime.edgepython.com/js/src/index.js";
    import { dom } from "./src/index.js";

    const worker = await createWorker({
        wasmUrl: "https://runtime.edgepython.com/js/compiler_lib.wasm",
        mainThreadModules: { dom },
    });
    await worker.run(await (await fetch("./script.py")).text());
</script>
```

Engine runs in a Web Worker; `dom` handlers run on the page's main thread (where `document` lives) via the runtime's deferred host-call mechanism. Python sees every call as synchronous.

## Testing

Cases live in [`dom.json`](dom.json) and run through the shared runner at the repo root:

```bash
# One-time setup
deno run -A npm:playwright install chromium

# Run (from repo root)
HOSTCAP=dom deno test --allow-all tests/
```

See [`tests/README.md`](../tests/README.md) for the corpus shape.

## API

**Conventions:**

- Handles are opaque integers — store, pass, never compute on them.
- Multi-result queries (`query_all`, `children`) return CSV strings of handles.
- Structured returns (`rect`, `validity`, `form_data`, `bbox`, event payloads) are JSON strings.
- Async results (events, FileReader, animation finishes, observer entries) arrive via `receive()`.
- Async payloads carry a correlation handle so the consumer can route results back to the originating call: `target_handle` (events, observers), `file_handle` (file_read_*), `animation_handle` (animate finish).
- Cleanup is automatic. Detaching nodes (`remove`, `replace_children`, `set_html`, `set_text`) sweeps the subtree's handles, event bindings, and active animations. `file_read_*` releases its file handle on completion. Animations auto-release on finish or cancel, except `iterations: "Infinity"` loops, which need `animation_dispose(h)`.

**Parsing JSON returns.** The runtime auto-registers `json` — `from json import loads, dumps` works with zero setup. Examples below assume it.

### Selection and traversal

```python
from dom import query, query_all, closest

card = query("#card")
form = closest(card, "form")
items = [int(h) for h in query_all("li.todo").split(",") if h]
```

`query`, `query_all`, `closest`, `matches`, `body`, `active_element`, `parent`, `children`, `first_child`, `last_child`, `next_sibling`, `prev_sibling`, `tag_name`.

### Creation and mutation

```python
from dom import create_element, set_text, append_child, replace_children

li = create_element("li")
set_text(li, "row")
append_child(query("#list"), li)

replace_children(query("#list"))  # atomic clear
```

`create_element`, `create_element_ns`, `append_child`, `insert_before`, `remove`, `replace_children`, `clone_node`.

### Content, attributes, classes

```python
from dom import set_text, set_html, toggle_class, set_data

set_text(query("#title"), "Hello")
set_html(query("#body"), "<p>Inline markup</p>")
toggle_class(query("#menu"), "open")
set_data(query("#row"), "id", "42")  # element.dataset.id = "42"
```

`get_text`, `set_text`, `get_html`, `set_html`, `get_attribute`, `set_attribute`, `remove_attribute`, `add_class`, `remove_class`, `toggle_class`, `has_class`, `get_data`, `set_data`.

### Style and layout

```python
from dom import set_style, rect, focus
import json

set_style(query("#box"), "transform", "translateX(20px)")
r = json.loads(rect(query("#box")))
print(r["w"], r["h"], r["x"], r["y"])
focus(query("#input"))
```

`set_style`, `get_style`, `get_computed_style`, `rect`, `offset_width`, `offset_height`, `client_width`, `client_height`, `scroll_top`, `set_scroll_top`, `scroll_into_view`, `focus`, `blur`.

### Events

```python
import json
from dom import bind_event

bind_event(query("#form"), "submit", "submit", '{"prevent_default": true}')

async def main():
    while True:
        ev = json.loads(receive())
        if ev["msg"] == "submit":
            print("submitted")
```

`bind_event(node, type, msg, options_json?)` dispatches a JSON detail on each fire. Payload fields: `msg`, `target_handle`, `type`, `target_id`, `target_tag`, `value`, `checked`, `key`, `code`, `button`, `x`, `y`, `movement_x`/`y`, `alt`/`ctrl`/`shift`/`meta`, plus `drop_files`, `drop_text`, `clipboard_text`, `clipboard_files`, `touches` when applicable. `target_handle` is the bound element (useful when sharing a `msg` across multiple bindings). Options JSON: `prevent_default`, `stop_propagation`, `once`, `capture`, `passive`.

Returns a binding handle for `unbind_event(h)`. Also: `dispatch_event(node, type, detail?)`, `click(node)` (native click — triggers file pickers and form behaviors).

### Forms

```python
import json
from dom import validity, set_custom_validity, form_data

email = query("#email")
v = json.loads(validity(email))
if v["type_mismatch"]:
    set_custom_validity(email, "Please use a valid email")

data = json.loads(form_data(query("#signup")))
# {"email": ["x@y.com"], "remember": ["on"]}
```

`get_value`, `set_value`, `get_checked`, `set_checked`, `form_submit`, `form_reset`, `form_data`, `is_valid`, `validity`, `report_validity`, `set_custom_validity`, `validation_message`.

### Files

```python
import json
from dom import bind_event, get_files, file_read_data_url, set_attribute

bind_event(query("#picker"), "change", "picked")

async def main():
    while True:
        ev = json.loads(receive())
        if ev["msg"] == "picked":
            for h in [int(x) for x in get_files(query("#picker")).split(",") if x]:
                file_read_data_url(h, "loaded")
        elif ev["msg"] == "loaded" and ev["ok"]:
            set_attribute(query("#preview"), "src", ev["data_url"])
```

`get_files`, `file_info`, `file_read_text`, `file_read_data_url`. Reads are async; result via `receive()` as `{msg, file_handle, ok, text | data_url}` (or `{msg, file_handle, ok: false, error}`). File handle releases on completion (one read per handle; re-pick or re-drop if you need to read again). Dropped and pasted files also surface as handles in `bind_event` payloads (`drop_files`, `clipboard_files`).

### Observers

```python
import json
from dom import observe_intersection

observe_intersection(query("#hero"), "visible", '{"threshold": 0.5}')

async def main():
    while True:
        ev = json.loads(receive())
        if ev["msg"] == "visible" and ev["intersecting"]:
            set_attribute(query("#hero img"), "src", "/large.jpg")
```

`observe_intersection`, `observe_resize`, `observe_mutations` and their `unobserve_*` counterparts. Each fires a payload through `receive()` per entry with observer-specific fields. All payloads carry `target_handle` (the handle passed to `observe_*`) so a shared `msg` can be routed back to its source observer.

### Animations

```python
from dom import animate

animate(query("#spinner"),
    '[{"transform": "rotate(0deg)"}, {"transform": "rotate(360deg)"}]',
    '{"duration": 1000, "iterations": "Infinity"}')

animate(query("#toast"),
    '[{"opacity": 0}, {"opacity": 1}]',
    '{"duration": 200, "fill": "forwards"}',
    "toast_done")  # {msg:"toast_done", animation_handle:<int>, ok:true} via receive() on finish
```

Works on HTML and SVG elements. Returns a handle controlled by `animation_play`, `animation_pause`, `animation_cancel`, `animation_finish`, `animation_reverse`. Handle auto-disposes on finish or cancel; for `iterations: "Infinity"` use `animation_dispose(h)` when done.

### Media (`<video>` and `<audio>`)

```python
from dom import media_play, media_pause, get_current_time, set_current_time, get_paused

vid = query("#movie")
if get_paused(vid):
    media_play(vid)
set_current_time(vid, 30.0)  # seek
```

`media_play`, `media_pause`, `get_current_time`, `set_current_time`, `get_duration`, `get_paused`, `set_volume`, `set_playback_rate`. Bind to `timeupdate` / `ended` / `loadedmetadata` for state events.

### Platform (dialog, fullscreen, pointer lock, SVG)

```python
from dom import show_modal, request_fullscreen, create_element_ns, append_child, set_attribute

show_modal(query("dialog"))
request_fullscreen(query("#game"))

SVG = "http://www.w3.org/2000/svg"
circle = create_element_ns(SVG, "circle")
set_attribute(circle, "cx", "50")
set_attribute(circle, "cy", "50")
set_attribute(circle, "r", "40")
append_child(query("svg"), circle)
```

`show_modal`, `dialog_close`. `request_fullscreen`, `exit_fullscreen`, `fullscreen_element`. `request_pointer_lock`, `exit_pointer_lock`. `bbox`, `path_length`, `point_at_length` (SVG geometric introspection).

### Errors

Async errors in DOM callbacks (event listeners, observer callbacks, swallowed promise rejections in `media_play` / `request_fullscreen` / `request_pointer_lock` / `animate`) go to the browser console by default. Bind a message to surface them as ordinary Python events:

```python
import json
from dom import bind_global_error

bind_global_error("err")

async def main():
    while True:
        ev = json.loads(receive())
        if ev["msg"] == "err":
            print(f"[{ev['where']}] {ev['error']}")
            continue
        # …rest of dispatch
```

Payload: `{msg, where, error, stack?}`. `where` identifies the call site (`event:click`, `media_play`, `observe_intersection`, `animate`, `request_fullscreen`, …). With no binding set, errors fall back to `console.error` with no Python-visible signal.

## How it works

Factory `(ctx) => handlers`. `src/state.js` opens a fresh closure per `createWorker` with handle tables (`nodes`, `bindings`, `files`, observers, animations), `alloc` / `node` / `allocList` helpers, and `cleanSubtree` (sweeps handles, bindings, and animations rooted in a detached element). Eight handler slices (`tree`, `style`, `events`, `forms`, `observers`, `animations`, `media`, `platform`) each return an object literal of named handlers closing over the shared state; `src/index.js` composes them with `Object.assign` and wraps async callbacks with `emitError` so failures surface via `bind_global_error`. Async callbacks call `ctx.pushEvent(jsonDetail)` to wake a paused `receive()`.

Adding a handler is one entry in one slice. Nothing else changes.

## Performance

Per-handler cost is one `postMessage` round-trip (~0.1–0.4 ms in modern browsers) — plenty for UI-rate workloads (events, mutations, layout) at hundreds of ops/frame.

Bad fit: tight per-frame loops with thousands of fine-grained ops, or pixel-precise renders. Pair with a `<canvas>` capability for the framebuffer path.

## Distribution

JS sources only — `compiler_lib.wasm` and the runtime load from `runtime.edgepython.com` at page load. No vendored copy, no build step.

## License

MIT OR Apache-2.0
