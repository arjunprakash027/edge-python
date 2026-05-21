# Edge Python DOM

DOM access for Edge Python, shipped as a plain ESM module. Python scripts see `dom` as an ordinary module — full surface coverage: queries, mutation, events, forms, files, observers, animations, layout, media, SVG, and modern platform APIs (dialog, fullscreen, pointer lock).

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

The engine runs in a Web Worker; `dom` handlers run on the page's main thread (where `document` lives) via the runtime's deferred host-call mechanism. Python sees every call as synchronous.

## Quick start

```bash
git clone https://github.com/dylan-sutton-chavez/edge-python-capabilities
cd edge-python-capabilities
python3 -m http.server 8080
```

Open <http://127.0.0.1:8080/dom/web/>. No build step.

## Testing

A smoke test loads the demo in headless Chromium, clicks through a few interactions, and fails if any console error fires.

```bash
# Deno setup
curl -fsSL https://deno.land/install.sh | sh
source ~/.bashrc

#Cache the browser binary
deno run -A npm:playwright install chromium

# Run
cd edge-python-capabilities/dom
deno test --allow-all tests/dom.test.js
```

## API

**Conventions.**

- Handles are opaque integers — store them, pass them, never compute on them.
- Multi-result queries (`query_all`, `children`) return CSV strings of handles.
- Structured returns (`rect`, `validity`, `form_data`, `bbox`, event payloads) are JSON strings.
- All async results (events, FileReader, animation finishes, observer entries) arrive through `receive()`.

**Parsing JSON returns.** Edge Python has no stdlib `json` — declare one in your `packages.json` (e.g. `{ "imports": { "json": "https://runtime.edgepython.com/lib/json.py" } }`) to get `json.loads`. For simple dispatch by event tag you can skip it and substring-match the raw payload (`'"msg":"click"' in ev`) as shown in `web/palette.py`. Examples below assume `json` is mapped where they use it.

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

`bind_event(node, type, msg, options_json?)` dispatches a JSON detail on each fire. Payload fields: `msg`, `type`, `target_id`, `target_tag`, `value`, `checked`, `key`, `code`, `button`, `x`, `y`, `movement_x`/`y`, `alt`/`ctrl`/`shift`/`meta`, plus `drop_files`, `drop_text`, `clipboard_text`, `clipboard_files`, `touches` when applicable. Options JSON: `prevent_default`, `stop_propagation`, `once`, `capture`, `passive`.

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

`get_files`, `file_info`, `file_read_text`, `file_read_data_url`. Reads are async — the result arrives via `receive()`. Dropped and pasted files also surface as handles in `bind_event` payloads (`drop_files`, `clipboard_files`).

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

`observe_intersection`, `observe_resize`, `observe_mutations` and their `unobserve_*` counterparts. Each fires a payload through `receive()` per entry with observer-specific fields.

### Animations

```python
from dom import animate

animate(query("#spinner"),
    '[{"transform": "rotate(0deg)"}, {"transform": "rotate(360deg)"}]',
    '{"duration": 1000, "iterations": "Infinity"}')

animate(query("#toast"),
    '[{"opacity": 0}, {"opacity": 1}]',
    '{"duration": 200, "fill": "forwards"}',
    "toast_done")  # arrives via receive() on finish
```

Works on HTML and SVG elements. Returns a handle controlled by `animation_play`, `animation_pause`, `animation_cancel`, `animation_finish`, `animation_reverse`.

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

## How it works

The module is a factory `(ctx) => handlers`. `src/state.js` opens a fresh closure per `createWorker` with the handle tables (`nodes`, `bindings`, `files`, observers, animations) and the `alloc` / `node` / `allocList` helpers. Eight handler slices (`tree`, `style`, `events`, `forms`, `observers`, `animations`, `media`, `platform`) each return an object literal of named handlers that close over the shared state; `src/index.js` composes them with `Object.assign`. Async callbacks (event listeners, `FileReader`, animation `finished`, observer entries) call `ctx.pushEvent(jsonDetail)` to wake a paused `receive()` in the script.

Adding a handler is one entry in one slice. Nothing else changes.

## Performance

Per-handler cost is one `postMessage` round-trip between the Worker (engine) and the main thread (handlers): ~0.1–0.4 ms in modern browsers. Plenty of headroom for UI-rate workloads — events, mutations, layout — at hundreds of ops per frame.

Bad fit: tight per-frame loops with thousands of fine-grained ops, or pixel-precise renders. Pair with a `<canvas>` capability for the framebuffer path.

## Distribution

This repo serves only the JS sources. `compiler_lib.wasm` and the Edge Python runtime both come from `runtime.edgepython.com` at page load — no vendored copy here, no build step.

## License

MIT OR Apache-2.0
