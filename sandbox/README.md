# Sandbox

Shared browser shell for every host capability. `index.html` boots the upstream Edge Python runtime worker, resolves the capability's ESM factory via `?capability=<name>`, registers it as a `mainThreadModule`, and exposes `window.runHostCase(src, html?)` for the agnostic Deno test driver to call.

## Manual exploration

`index.html` is a headless loader, no UI. Drive it from devtools.

Serve the repo root and open the sandbox pointed at a capability:

```bash
python3 -m http.server 8000
# -> http://localhost:8000/sandbox/?capability=dom
```

Open devtools and call `runHostCase(src, html?)`:

```js
await runHostCase(
    `from dom import *\nb = body()\nel = create_element("p")\nset_text(el, "hi")\nappend_child(b, el)\nprint(get_text(el))`,
);
// { output: ["hi"], error: null }
```

Pass `html` (second arg) to seed `document.body.innerHTML` before the case runs.

## Automated tests

`run.test.js` next to `index.html` is the agnostic Playwright driver. It discovers capabilities by walking the repo root for `<cap>/<cap>.json` corpora and runs each through this sandbox. From the repo root:

```bash
deno test --allow-all sandbox/
```

`HOSTCAP=<cap>` narrows discovery to a single capability — CI uses it to fan out the matrix.

## Corpus shape

Each `<cap>/<cap>.json` is an array of cases. Per case:

| Field | Type | Purpose |
|---|---|---|
| `src` | string | Python source. Runner prepends `from <cap> import *\n`. |
| `output` | string[] | Expected stdout lines. |
| `error` | string | Expected substring of the Python traceback (use instead of `output`). |
| `html` | string | Optional `document.body.innerHTML` fixture. |
| `http_mocks` | `{url, body, status?, contentType?}[]` | `page.route` patterns fulfilled per call. |
| `ws_mocks` | `{url, echo?}[]` | `page.routeWebSocket` patterns; `echo: true` reflects messages back. |

Between cases the sandbox wipes `document.body`, `localStorage`, `sessionStorage`, and any IndexedDB databases, so each case starts from a fresh browser state.
