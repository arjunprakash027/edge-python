# Edge Python JSON

JSON serialization, deserialization, and round-tripping shipped as a `.wasm` plugin. Scripts see `json` as ordinary.

```python
from json import dumps, loads

# Parse — JSON text -> native Python value.
data = loads('{"name":"ada","tags":["math","cs"],"score":91.5}')
print(data["name"])
print(data["tags"][0])

# Serialize — native value -> JSON text.
print(dumps({"k": [1, 2, 3], "ok": True}))
```

## API

### Conventions

- **Types map straight through.** `null<->None`, `true/false<->bool`, JSON numbers split into `int` (i128) and `float` (f64), strings are UTF-8, arrays are `list`, objects are `dict`. Python `tuple`/`set` serialize as JSON arrays but parse back as `list`.
- **Errors raise typed exceptions.** Parser failures surface as `ValueError` with the byte offset; non-serializable values raise `TypeError` naming the offending type unless `default=` is supplied.
- **Compact output by default.** No whitespace, dict insertion order preserved. Integer-valued floats keep a trailing `.0` so a round-trip through `loads` returns the same Python type.
- **CPython parity.** Both functions accept the full kwargs of `json.dumps` / `json.loads` from the Python standard library; defaults match CPython.

### loads

```python
loads(s, *,
      cls=None, object_hook=None, parse_float=None,
      parse_int=None, parse_constant=None, object_pairs_hook=None)
```

```python
loads("null")                                # -> None
loads("42")                                  # -> 42 (int)
loads("9223372036854775808")                 # -> 2**63 (int via i128 wire tag)
loads("1.5e3")                               # -> 1500.0 (float)
loads('"a\\nb"')                             # -> "a\nb"
loads("[1,2,3]")                             # -> [1, 2, 3]
loads('{"k":"v","n":1}')                     # -> {"k": "v", "n": 1}
loads("NaN")                                 # -> float('nan')
loads("Infinity")                            # -> float('inf')

try:
    loads("[1,]")
except ValueError as e:
    print(e)                                 # -> unexpected token at byte 3
```

Each kwarg fires at the matching production and replaces the default decoding for that node.

| Kwarg | Type | Behaviour |
| --- | --- | --- |
| `object_hook` | `callable(dict) -> any` | Called with every completed object; the return value replaces the dict. |
| `object_pairs_hook` | `callable(list[[key, val]]) -> any` | Wins over `object_hook` when both are set. Receives a list of `[key, value]` lists (no `__new_tuple__` in the ABI yet). |
| `parse_float` | `callable(str) -> any` | Receives the raw float source token; return value used as-is. |
| `parse_int` | `callable(str) -> any` | Receives the raw int source token; return value used as-is. |
| `parse_constant` | `callable(str) -> any` | Fires for `"NaN"`, `"Infinity"`, `"-Infinity"`. |
| `cls` | callable | Reserved for an alternate decoder class (currently behaves like default decoding; pass a custom hook combination instead). |

```python
loads("42", parse_int=lambda s: "int:" + s)              # -> "int:42"
loads('{"x":1}', object_hook=lambda d: d["x"])            # -> 1
loads('{"a":1,"b":2}', object_pairs_hook=lambda p: len(p)) # -> 2
loads("NaN", parse_constant=lambda s: "CONST:" + s)       # -> "CONST:NaN"
```

### dumps

```python
dumps(obj, *,
      skipkeys=False, ensure_ascii=True, check_circular=True,
      allow_nan=True, cls=None, indent=None, separators=None,
      default=None, sort_keys=False)
```

```python
dumps(None)                                  # -> 'null'
dumps(True)                                  # -> 'true'
dumps(42)                                    # -> '42'
dumps(1.5)                                   # -> '1.5'
dumps(1.0)                                   # -> '1.0'
dumps("hello")                               # -> '"hello"'
dumps([1, 2, 3])                             # -> '[1,2,3]'
dumps((1, 2, 3))                             # -> '[1,2,3]'
dumps({"a": 1, "b": None})                   # -> '{"a":1,"b":null}'
```

| Kwarg | Default | Behaviour |
| --- | --- | --- |
| `indent` | `None` | Integer pretty-print width. With indent, key separator becomes `": "` by default. |
| `sort_keys` | `False` | Sort dict keys ASCII-lexicographically before emit. |
| `ensure_ascii` | `True` | Escape characters >= U+0080 as `\uXXXX` (surrogate pair for code points beyond the BMP). When `False`, emit the UTF-8 bytes directly. |
| `check_circular` | `True` | Bound recursive nesting at 200 levels; deeper structures raise `ValueError("Circular reference detected")` instead of overflowing the host stack. (Set `False` if you really want deep linear trees.) |
| `allow_nan` | `True` | Emit `NaN` / `Infinity` / `-Infinity` for non-finite floats. With `False`, non-finite values raise `ValueError`. |
| `skipkeys` | `False` | Silently skip non-`str` dict keys instead of raising `TypeError`. |
| `separators` | `(",", ":")` compact, `(",", ": ")` with indent | Two-element tuple `(item_sep, key_sep)`. |
| `default` | `None` | Callable that receives any non-serializable value; its return is re-serialized. |
| `cls` | `None` | Encoder class with an `.encode(obj)` method; if supplied, the whole walk is delegated to it. |

```python
dumps([1, 2, 3], indent=2)
# [
#   1,
#   2,
#   3
# ]

dumps({"b": 2, "a": 1}, sort_keys=True)             # -> '{"a":1,"b":2}'
dumps("héllo", ensure_ascii=False)                  # -> '"héllo"'
dumps({1: 2, "k": 3}, skipkeys=True)                # -> '{"k":3}'
dumps(float("nan"))                                  # -> 'NaN'
dumps(float("nan"), allow_nan=False)                # -> ValueError
dumps([1, 2], separators=(" | ", " = "))            # -> '[1 | 2]'

class X: pass
dumps(X(), default=lambda o: "obj")                  # -> '"obj"'
```

### Round-tripping

```python
dumps(loads('[1,2,3]'))                      # -> '[1,2,3]'
dumps(loads('{"k":"v"}'))                    # -> '{"k":"v"}'
```

Idempotent for canonical shapes, output of `dumps` parsed by `loads` then re-`dumps`'d returns the same text.

## How it works

The crate compiles to `wasm32-unknown-unknown` (`cdylib`) against the [wasm-pdk](https://github.com/dylan-sutton-chavez/edge-python/tree/main/wasm-pdk) `v0.1.0` ABI. Hosts resolve `from json import ...` by fetching `json.wasm` (via a `packages.json` alias or quoted URL) and treating its exports as native bindings.

`loads` builds Python values entirely through the handle ABI: `Handle::new_dict` / `new_list` for composites, `Handle::set_item` and `Handle::call("append", ...)` to populate them, primitives via `encode(Value::...)`. `dumps` walks the input handle with `type_of`, `iter`, `len`, `get_item`. Hooks (`object_hook`, `default`, `parse_*`, etc.) are forwarded to the caller's Python callable via the `Handle::call("__call__", args)` shorthand the runtime exposes for invoking any callable.

## Performance

Single-pass tokenizer + recursive-descent parser; around 95 KB stripped. The serializer reuses a single growing `String` buffer per call. For multi-megabyte payloads the bottleneck is the handle round-trip per primitive (the runtime's wire ABI cost), not parsing.

Plugin memory is recycled per call: each `loads`/`dumps` returns its scratch allocations to a static 4 MB pool, so long-running workers (JSONL streaming, polling loops) stay flat rather than growing. The upstream wasm-pdk ABI leaks around 8 bytes per host call (`__edge_alloc` boxed-slice), so a single worker session caps at roughly 500 k plugin calls before the pool exhausts; recycle the worker periodically for unbounded streaming.

## Distribution

Pre-built `.wasm` published with each release on the `edge-python-stdpkg` GitHub releases page. Host runtimes resolve the URL via their cache (browser worker auto-caches in IndexedDB; other hosts can mirror).

## License

MIT OR Apache-2.0
