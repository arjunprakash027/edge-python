---
title: "Methods"
description: "Built-in methods on strings, bytes, lists, dicts, and sets."
---

Edge Python provides built-in methods on `str`, `bytes`, `list`, `dict`, and `set`. The set is curated to cover common operations; rarely used variants are omitted. Methods that aren't here are intentional omissions documented under each section.

There are **no methods on `int`, `float`, or `tuple`** — primitive types stay dispatch-free. `(5).bit_length()`, `(255).to_bytes(...)`, `(3.14).is_integer()`, `(1, 2).count(1)`, `(1, 2).index(2)` all raise `AttributeError`. For ints, use the [`int_from_bytes` / `int_to_bytes`](/reference/builtins#bytes_fromhex-int_from_bytes-int_to_bytes) free functions; for tuple element counting, use `sum(1 for x in t if x == v)` or convert to a list first.

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

Three predicates are supported: `isdigit` (ASCII digits only), `isalpha` (Unicode alphabetic), `isalnum` (Unicode alphanumeric). All three return `False` on the empty string.

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

`casefold`, `swapcase`, `isspace`, `isascii`, `isidentifier`, `isnumeric`, `isdecimal`, `islower`, `isupper`, `istitle`, `isprintable` are not provided — write the predicate explicitly when you need it.

### Search and count

`startswith`, `endswith`, `find`, and `count` take a single string argument — there are no `start` / `end` slice positions. `find` returns a code-point index (not byte offset) and `-1` when the substring is missing. `rfind`, `index`, and `rindex` are not provided; combine `find` with reversal or write the loop explicitly.

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

`split` with no argument splits on runs of whitespace; with an explicit separator it splits on every occurrence. There is no `maxsplit` argument, no `rsplit`. `replace` always replaces every occurrence — there is no `count` cap. `splitlines` does **not** accept a `keepends` flag; line separators are dropped.

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

`center(width[, fill])` and `zfill(width)` measure padding in **code points**, not bytes — `'ñ'.center(5, '*')` produces `**ñ**` (5 visible characters). `ljust`, `rjust`, `expandtabs`, `translate`, `maketrans`, and `format` / `format_map` are not provided.

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

`s.encode([encoding])` produces `bytes`. Supported encoding names are `"utf-8"`, `"utf8"`, and `"ascii"` (which errors on non-ASCII content). Anything else raises `ValueError`. Default is `"utf-8"`.

```python
print("café".encode())
print("hi".encode("ascii"))
```

```text Output
b'caf\xc3\xa9'
b'hi'
```

## Bytes methods

`bytes` has its own small set of methods. `bytes.find` returns a **byte** offset (not a code-point index). `bytes.index` raises `ValueError` ("subsection not found") when the subsection is absent. `split` requires an explicit separator — there is no whitespace-split mode. `bytearray` and `memoryview` are not implemented.

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

These return `None` and modify the list in place. `extend` accepts any iterable (list, tuple, set, frozenset, dict (yields keys), range, bytes, str, generator, coroutine). `sort` has no `key=` or `reverse=` keyword — sort by a derived key by precomputing tuples.

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

`keys`, `values`, and `items` return **concrete `list` snapshots**, not live views. Mutating the dict afterwards does not affect a previously captured snapshot — and that is intentional. Live views constitute shared mutable state, which conflicts with the functional paradigm; an immutable snapshot guarantees the captured collection cannot mutate underneath the consumer.

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

`update` accepts either a `dict` **or** an iterable of length-2 sequences (`[(key, value), ...]`). The kwargs form `d.update(a=1)` is **not** supported. `popitem` removes and returns the last-inserted entry (insertion order); on an empty dict it raises `ValueError`. `clear` and `fromkeys` are not provided — use `d = {}` and a comprehension respectively.

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

`remove` raises `KeyError` when the element is absent; `discard` is the silent variant. `pop` removes an arbitrary element and raises `ValueError` on an empty set. `update` accepts any iterable.

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

`union`, `intersection`, `difference`, and `symmetric_difference` return a fresh set; their operator forms (`|`, `&`, `-`, `^`) work the same way and accept augmented assignment (`|=`, `&=`, `-=`, `^=`). The in-place variants `intersection_update`, `difference_update`, and `symmetric_difference_update` are **not** provided — assign the augmented form back (`s &= other`).

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

