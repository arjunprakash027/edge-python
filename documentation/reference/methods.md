---
title: "Methods"
description: "Built-in methods on strings, bytes, lists, dicts, and sets."
---

Built-in methods on `str`, `bytes`, `list`, `dict`, `set`. Curated to cover common operations; rarely used variants are omitted (documented per section).

No methods on `int`, `float`, or `tuple`, primitive types stay dispatch-free. `(5).bit_length()`, `(255).to_bytes(...)`, `(3.14).is_integer()`, `(1, 2).count(1)` all raise `AttributeError`. For ints use [`int_from_bytes` / `int_to_bytes`](/reference/builtins#bytes_fromhex-int_from_bytes-int_to_bytes); for tuple counting use `sum(1 for x in t if x == v)`.

```python
# Methods are accessed with dot notation
print("hello".upper())
print([3, 1, 2].count(1))
print({"a": 1}.get("a"))
```

```text Output
HELLO
1
1
```

## String methods

### Case transforms

```python
print("hello".upper())
print("HELLO".lower())
print("hello world".capitalize())
print("hello world".title())
```

```text Output
HELLO
hello
Hello world
Hello World
```

### Whitespace

```python
print("  hi  ".strip())
print("  hi  ".lstrip())
print("  hi  ".rstrip())

# With a custom strip set
print("xxhelloxx".strip("x"))
```

```text Output
hi
hi  
  hi
hello
```

### Predicates

Three predicates: `isdigit` (ASCII), `isalpha` (Unicode), `isalnum` (Unicode). All return `False` on empty string.

```python
print("123".isdigit())
print("abc".isdigit())

print("abc".isalpha())
print("abc123".isalpha())

print("abc123".isalnum())
print("abc 123".isalnum())
```

```text Output
True
False
True
False
True
False
```

Not provided: `casefold`, `swapcase`, `isspace`, `isascii`, `isidentifier`, `isnumeric`, `isdecimal`, `islower`, `isupper`, `istitle`, `isprintable`. Write the predicate explicitly.

### Search and count

`startswith`, `endswith`, `find`, `count` take a single string arg, no `start` / `end` slice positions. `find` returns a code-point index (not byte offset), `-1` if missing. No `rfind`, `index`, `rindex`, combine `find` with reversal.

```python
print("hello".startswith("he"))
print("hello".endswith("lo"))
print("hello".find("ll"))
print("hello".find("z"))
print("hello".count("l"))
```

```text Output
True
True
2
-1
2
```

### Split, join, replace

`split` no-arg splits on whitespace runs; explicit separator splits on every occurrence. No `maxsplit`, no `rsplit`. `replace` always replaces all (no `count` cap). `splitlines` drops separators (no `keepends`).

```python
print("a,b,c".split(","))
print("hello world".split()) # any whitespace
print(",".join(["a", "b", "c"]))
print("hello".replace("l", "L"))
print("foobar".removeprefix("foo"))
print("foobar".removesuffix("bar"))
print("a\nb\nc".splitlines())
print("foo:bar:baz".partition(":"))
print("foo:bar:baz".rpartition(":"))
```

```text Output
['a', 'b', 'c']
['hello', 'world']
a,b,c
heLLo
bar
foo
['a', 'b', 'c']
('foo', ':', 'bar:baz')
('foo:bar', ':', 'baz')
```

### Padding

`center(width[, fill])` and `zfill(width)` measure in code points, not bytes, `'ñ'.center(5, '*')` produces `**ñ**` (5 visible chars). Not provided: `ljust`, `rjust`, `expandtabs`, `translate`, `maketrans`, `format`, `format_map`.

```python
print("abc".center(7, "-"))
print("42".zfill(5))
print("-42".zfill(5))
print("ñ".center(5, "*"))
```

```text Output
--abc--
00042
-0042
**ñ**
```

### Encoding

`s.encode([encoding])` -> bytes. Supports `"utf-8"`, `"utf8"`, `"ascii"` (errors on non-ASCII). Else `ValueError`. Default `"utf-8"`.

```python
print("café".encode())
print("hi".encode("ascii"))
```

```text Output
b'caf\xc3\xa9'
b'hi'
```

## Bytes methods

Small method set. `bytes.find` returns a byte offset (not a code-point index). `bytes.index` raises `ValueError` ("subsection not found") if absent. `split` needs an explicit separator (no whitespace-split mode). `bytearray` / `memoryview` unimplemented.

```python
b = b"\x48\x65\x6c\x6c\x6f"

print(b.decode()) # default utf-8
print(b.hex()) # no separator argument
print(b.startswith(b"He"))
print(b.endswith(b"lo"))
print(b.find(b"ll"))
print(b.count(b"l"))
print(b.replace(b"l", b"L"))
print(b"a,b,c".split(b","))
```

```text Output
Hello
48656c6c6f
True
True
2
2
b'HeLLo'
[b'a', b'b', b'c']
```

## List methods

### Pure (return a new value or query)

```python
xs = [1, 2, 3, 2]

print(xs.index(2))
print(xs.count(2))

ys = xs.copy()
ys.append(99)
print(xs) # original unchanged
print(ys)
```

```text Output
1
2
[1, 2, 3, 2]
[1, 2, 3, 2, 99]
```

### Mutating

Return `None`, mutate in place. `extend` accepts any iterable. `sort` has no `key=` / `reverse=`, sort by derived key via precomputed tuples.

```python
xs = [1, 2, 3]

xs.append(4)
print(xs)

xs.extend(range(5, 7)) # any iterable
print(xs)

xs.insert(0, 99)
print(xs)
```

```text Output
[1, 2, 3, 4]
[1, 2, 3, 4, 5, 6]
[99, 1, 2, 3, 4, 5, 6]
```

```python
xs = [1, 2, 3, 2]

xs.remove(2) # first occurrence
print(xs)

popped = xs.pop() # last
print(popped, xs)

popped = xs.pop(0) # by index
print(popped, xs)
```

```text Output
[1, 3, 2]
2 [1, 3]
1 [3]
```

```python
xs = [3, 1, 4, 1, 5]
xs.sort()
print(xs)

xs.reverse()
print(xs)

xs.clear()
print(xs)
```

```text Output
[1, 1, 3, 4, 5]
[5, 4, 3, 1, 1]
[]
```

## Dict methods

### Views

`keys`, `values`, `items` return concrete `list` snapshots, not live views. Dict mutations don't affect captured snapshots, intentional: live views are shared mutable state, conflicting with the functional paradigm.

```python
d = {"a": 1, "b": 2, "c": 3}

print(list(d.keys()))
print(list(d.values()))
print(list(d.items()))

# Snapshot is detached from the dict
k = d.keys()
d["d"] = 4
print(k) # ['a', 'b', 'c'] — pre-mutation
```

```text Output
['a', 'b', 'c']
[1, 2, 3]
[('a', 1), ('b', 2), ('c', 3)]
['a', 'b', 'c']
```

### Lookup with default

```python
d = {"a": 1}

print(d.get("a"))
print(d.get("z"))
print(d.get("z", 0))
```

```text Output
1
None
0
```

### Mutation

`update` accepts a `dict` or iterable of length-2 sequences. Kwargs form (`d.update(a=1)`) not supported. `popitem` returns the last-inserted entry; empty -> `ValueError`. No `clear` / `fromkeys`, use `d = {}` / a comprehension.

```python
d = {"a": 1}

d.update({"b": 2, "a": 99})
print(d)

# Iterable-of-pairs also works
d.update([("c", 3), ("d", 4)])
print(d)

removed = d.pop("a")
print(removed, d)

print(d.pop("missing", "fallback"))
```

```text Output
{'a': 99, 'b': 2}
{'a': 99, 'b': 2, 'c': 3, 'd': 4}
99 {'b': 2, 'c': 3, 'd': 4}
fallback
```

```python
d = {}
d.setdefault("a", 1)
d.setdefault("a", 999) # second call ignored
print(d)
```

```text Output
{'a': 1}
```

## Set methods

### Mutation

`remove` raises `KeyError` if absent; `discard` is silent. `pop` removes arbitrary element; empty -> `ValueError`. `update` accepts any iterable.

```python
s = {1, 2, 3}

s.add(4)
print(s)

s.remove(2) # raises KeyError if absent
s.discard(99) # silently ignores absent values
print(s)

popped = s.pop()
print(popped in {1, 3, 4})

s.update([4, 5, 6])
print(s)

s.clear()
print(s)
```

```text Output
{1, 2, 3, 4}
{1, 3, 4}
True
{1, 3, 4, 5, 6}
set()
```

### Algebra

`union`, `intersection`, `difference`, `symmetric_difference` return fresh sets; operator forms (`|`, `&`, `-`, `^`) and augmented assignment (`|=`, `&=`, `-=`, `^=`) work the same. In-place `*_update` variants not provided, use augmented form (`s &= other`).

```python
a = {1, 2, 3}
b = {3, 4, 5}

print(a | b)
print(a & b)
print(a - b)
print(a ^ b)

print(a.union([4, 5])) # accepts any iterable
print({1, 2}.issubset({1, 2, 3}))
print({1, 2, 3}.issuperset({1}))
print({1, 2}.isdisjoint({3, 4}))
```

```text Output
{1, 2, 3, 4, 5}
{3}
{1, 2}
{1, 2, 4, 5}
{1, 2, 3, 4, 5}
True
True
True
```

Comparison operators between sets follow subset / superset semantics, not total order:

```python
print({1, 2} <  {1, 2, 3}) # proper subset
print({1, 2} <= {1, 2}) # subset
print({1, 2} >= {1}) # superset
print({1, 2} <= {2, 3}) # disjoint sides -> False
```

```text Output
True
True
True
False
```

