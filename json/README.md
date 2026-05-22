# Edge Python JSON

JSON serialization, deserialization, and round-tripping shipped as a `.wasm` plugin. Scripts see `json` as ordinary.

```python
from json import dumps, loads

# Parse — JSON text -> native Python value.
data = loads('{"name":"ada","tags":["math","cs"],"score":91.5}')
print(data["name"])
print(data["tags"][0])

# Serialize — native value -> JSON text. Round-trips canonical shapes verbatim.
print(dumps({"k": [1, 2, 3], "ok": True}))
```

## API

### Conventions

- **Types map straight through.** `null↔None`, `true/false↔bool`, JSON numbers split into `int` (i128) and `float` (f64), strings are UTF-8, arrays are `list`, objects are `dict`. Python `tuple`/`set` serialize as JSON arrays but parse back as `list`.
- **Errors raise typed exceptions.** Parser failures surface as `ValueError` with the byte offset; passing a non-serializable value (e.g. a class instance, `bytes`) to `dumps` raises `TypeError` naming the offending type.
- **Compact output.** `dumps` emits no whitespace and preserves dict insertion order. Integer-valued floats keep a trailing `.0` so a round-trip through `loads` returns the same Python type.
- **Strict input.** `loads` rejects trailing data, trailing commas in arrays/objects, control characters inside strings, and unpaired UTF-16 surrogates in `\uXXXX` escapes.

### loads

```python
from json import loads

loads("null")                                # -> None
loads("42")                                  # -> 42 (int)
loads("9223372036854775808")                 # -> 2**63 (int via i128 wire tag)
loads("1.5e3")                               # -> 1500.0 (float)
loads('"a\\nb"')                             # -> "a\nb"
loads("[1,2,3]")                             # -> [1, 2, 3]
loads('{"k":"v","n":1}')                     # -> {"k": "v", "n": 1}

# Errors carry the byte offset.
try:
    loads("[1,]")
except ValueError as e:
    print(e)                                 # -> unexpected token at byte 3
```

### dumps

```python
from json import dumps

dumps(None)                                  # -> 'null'
dumps(True)                                  # -> 'true'
dumps(42)                                    # -> '42'
dumps(1.5)                                   # -> '1.5'
dumps(1.0)                                   # -> '1.0'
dumps("hello")                               # -> '"hello"'
dumps([1, 2, 3])                             # -> '[1,2,3]'
dumps((1, 2, 3))                             # -> '[1,2,3]'
dumps({"a": 1, "b": None})                   # -> '{"a":1,"b":null}'

# Non-serializable values raise TypeError.
try:
    dumps(object())
except TypeError as e:
    print(e)                                 # -> 'object' is not JSON-serializable
```

### Round-tripping

```python
from json import loads, dumps

dumps(loads('[1,2,3]'))                      # -> '[1,2,3]'
dumps(loads('{"k":"v"}'))                    # -> '{"k":"v"}'
```

Idempotent for canonical shapes — output of `dumps` parsed by `loads` then re-`dumps`'d returns the same text.

## How it works

The crate compiles to `wasm32-unknown-unknown` (`cdylib`) against the [wasm-pdk](https://github.com/dylan-sutton-chavez/edge-python/tree/main/wasm-pdk) `v0.1.0` ABI. Hosts resolve `from json import …` by fetching `json.wasm` (via a `packages.json` alias or quoted URL) and treating its exports as native bindings.

`loads` builds Python values entirely through the handle ABI: `Handle::new_dict` / `new_list` for composites, `Handle::set_item` and `Handle::call("append", …)` to populate them, primitives via `encode(Value::…)`. `dumps` walks the input handle with `type_of` + `iter` / `iter_next` and reads dict values via `get_item`. No special-cased compiler hooks — the same wire format any other plugin uses.

## Performance

Single-pass tokenizer + recursive-descent parser; ~93 KB stripped, no allocations in the hot path beyond the values being constructed. Serializer reuses a single growing `String` buffer. For multi-megabyte payloads the bottleneck is the handle round-trip per primitive (the runtime's wire ABI cost), not parsing.

Plugin memory is recycled per call: each `loads`/`dumps` returns its scratch allocations to a static 4 MB pool, so long-running workers (JSONL streaming, polling loops) stay flat rather than growing. The current upstream wasm-pdk ABI leaks ~8 bytes per host call (`__edge_alloc` boxed-slice), capping any single worker session at ~500 k plugin calls before the pool exhausts.

## Distribution

Pre-built `.wasm` published with each release on the `edge-python-stdpkg` GitHub releases page. Host runtimes resolve the URL via their cache (browser worker auto-caches in IndexedDB; other hosts can mirror).

## License

MIT OR Apache-2.0
