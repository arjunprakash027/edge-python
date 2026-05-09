---
title: "Data types"
description: "Numbers, strings, sequences, mappings, sets, and None."
---

## Type checks

```python
print(type(42))
print(type(3.14))
print(type("hi"))
print(type([1, 2]))
print(type((1, 2)))
print(type({1, 2}))
print(type({"a": 1}))
print(type(None))
print(type(True))
```

```text Output
<class 'int'>
<class 'float'>
<class 'str'>
<class 'list'>
<class 'tuple'>
<class 'set'>
<class 'dict'>
<class 'NoneType'>
<class 'bool'>
```

```python
print(isinstance(42, int))
print(isinstance(True, int))         # bools are ints
print(isinstance(42, (str, int)))    # tuple of types
```

```text Output
True
True
True
```

## Integer

47-bit signed inline. The full range is `-140_737_488_355_328` to `140_737_488_355_327` (±2⁴⁷). Edge Python does **not** have arbitrary-precision (bigint) integers — this is a NaN-boxing tradeoff that keeps int arithmetic to one ALU instruction with no heap allocation. Any arithmetic that escapes the range raises `OverflowError`; literals outside the range are rejected by the parser. Complex literals (`1j`, `2+3j`) are unsupported.

```python
# Inside the supported range
print(2 ** 46)
print(2 ** 46 - 1)

try:
    print(2 ** 47)
except OverflowError:
    print("overflow")
```

```text Output
70368744177664
70368744177663
overflow
```

```python
# Modular exponentiation
print(pow(7, 13, 19))
print(divmod(17, 5))
```

```text Output
7
(3, 2)
```

## Float

IEEE-754 double precision. Mixed arithmetic with int coerces to float.

```python
print(0.1 + 0.2 == 0.3)
print(-0.0 == 0.0)
print(1 / 3)
print(round(2.5))      # banker's rounding
print(round(0.5))
print(round(1.55, 1))
```

```text Output
False
True
0.3333333333333333
2
0
1.6
```

## String

Strings are immutable. Indexing returns a single-character string.

```python
s = "hello"
print(s[0], s[-1])
print(s[1:4])
print(len(s))
print(s + " world")
print(s * 2)
print("ll" in s)
```

```text Output
h o
ell
5
hello world
hellohello
True
```

Iteration yields characters:

```python
for ch in "abc":
    print(ch)
```

```text Output
a
b
c
```

`len(s)` and the padding methods `str.center` / `str.zfill` measure in Unicode code points, not raw bytes — `'ñ'.center(5, '*')` produces `'**ñ**'`, five visual characters wide.

## Bytes

Immutable sequence of bytes (each 0–255). Distinct from `str`: stores raw octets, not Unicode. Indexing returns an `int`, not a single-byte slice.

```python
data = b"hello"
print(data)
print(type(data))
print(len(data))
print(data[0])           # int — the byte value
print(data[1:4])         # bytes — slice
```

```text Output
b'hello'
<class 'bytes'>
5
104
b'ell'
```

```python
# Hex escapes for arbitrary bytes
raw = b"\x00\x01\xff"
print(raw)
print(raw.hex())
```

```text Output
b'\x00\x01\xff'
0001ff
```

```python
# Iteration yields ints, not bytes
for byte in b"abc":
    print(byte)
```

```text Output
97
98
99
```

```python
# Constructors
print(bytes())                  # empty
print(bytes(3))                 # zero-filled, length 3
print(bytes([65, 66, 67]))      # from int iterable
print(bytes("hi", "utf-8"))     # encoded string
```

```text Output
b''
b'\x00\x00\x00'
b'ABC'
b'hi'
```

```python
# Round-tripping with str
s = "Edge Python"
encoded = s.encode("utf-8")
decoded = encoded.decode("utf-8")
print(encoded, decoded)
```

```text Output
b'Edge Python' Edge Python
```

`bytes` is hashable (works as dict key, set member) and comparable to other `bytes` values; `bytes == str` is always `False`, even when the bytes are valid UTF-8 of the string. Supported methods: `decode`, `hex`, `startswith`, `endswith`. Encoding names recognised by `encode`/`decode`/`bytes(s, ...)`: `"utf-8"` (default) and `"ascii"`.

## List

Mutable sequence.

```python
xs = [1, 2, 3]
xs[0] = 99
xs.append(4)
print(xs)
print(len(xs))

# Aliasing — both names see mutation
ys = xs
ys.append(5)
print(xs)
```

```text Output
[99, 2, 3, 4]
4
[99, 2, 3, 4, 5]
```

```python
# Equality is structural
print([1, 2, 3] == [1, 2, 3])
print([1, [2, 3]] == [1, [2, 3]])
```

```text Output
True
True
```

```python
# Slice assignment (step=1) resizes the list in place
xs = [1, 2, 3, 4, 5]
xs[1:3] = [20, 30, 40]
print(xs)

# Slice deletion
del xs[2:4]
print(xs)

# Insertion via empty slice
xs[1:1] = [99]
print(xs)
```

```text Output
[1, 20, 30, 40, 4, 5]
[1, 20, 4, 5]
[1, 99, 20, 4, 5]
```

## Tuple

Immutable sequence. The fastest container for fixed-size data and the only one usable as a dict key in mixed-type cases.

```python
t = (1, 2, 3)
print(t[0])
print(t + (4, 5))
print((1,))         # one-element needs trailing comma
print(())           # empty
```

```text Output
1
(1, 2, 3, 4, 5)
(1,)
()
```

## Dict

Insertion-ordered mapping. Keys must be hashable: numbers, strings, bytes, booleans, `None`, frozensets, and tuples whose elements are themselves hashable. Mutable containers (`list`, `dict`, `set`) raise `TypeError: unhashable type` if used as a key. Numerically equal keys (`1` and `1.0`, or `True` and `1`) collapse to a single slot — the second insertion overwrites the first.

```python
d = {"a": 1, "b": 2}
print(d["a"])
d["c"] = 3
print(d)
print(list(d.keys()))
print(list(d.values()))
print(list(d.items()))
```

```text Output
1
{'a': 1, 'b': 2, 'c': 3}
['a', 'b', 'c']
[1, 2, 3]
[('a', 1), ('b', 2), ('c', 3)]
```

```python
# Iteration yields keys
for k in {"x": 1, "y": 2}:
    print(k)
```

```text Output
x
y
```

## Set

Unordered collection of hashable values, no duplicates. Supports the standard mutators (`add`, `remove`, `discard`, `pop`, `clear`, `update`) and algebraic operators / methods (`|` `&` `-` `^` and `union`, `intersection`, `difference`, `symmetric_difference`); see [Methods](/reference/methods) for the full list.

```python
s = {1, 2, 2, 3}
s.add(4)
print(s)
print(len(s))

# Empty set literal is set(), not {}
print(set())
print(type({}))     # this is a dict

# Algebra
print({1, 2, 3} | {3, 4})
print({1, 2, 3} & {2, 3, 4})
print({1, 2} <= {1, 2, 3})   # subset
```

```text Output
{1, 2, 3, 4}
4
set()
<class 'dict'>
{1, 2, 3, 4}
{2, 3}
True
```

## Range

Lazy integer sequence. `range(stop)`, `range(start, stop)`, `range(start, stop, step)`.

```python
print(list(range(5)))
print(list(range(2, 8)))
print(list(range(0, 10, 2)))
print(list(range(10, 0, -1)))
```

```text Output
[0, 1, 2, 3, 4]
[2, 3, 4, 5, 6, 7]
[0, 2, 4, 6, 8]
[10, 9, 8, 7, 6, 5, 4, 3, 2, 1]
```

## NoneType

Single value, single type.

```python
print(None)
print(None is None)
print(type(None))
```

```text Output
None
True
<class 'NoneType'>
```

## Ellipsis

`...` is a true singleton of type `ellipsis`. It compares equal only to itself and is distinct from the string `'...'`.

```python
print(...)
print(... is ...)
print(type(...))
print(... == '...')
```

```text Output
Ellipsis
True
<class 'ellipsis'>
False
```

## Conversions

```python
print(int("42"))
print(int(3.7))         # truncates toward zero
print(int(True))
print(float("3.14"))
print(str(42))
print(str([1, 2]))
print(bool([]))         # empty is falsy
print(bool([0]))        # non-empty is truthy
print(list("abc"))
print(tuple([1, 2, 3]))
print(set([1, 1, 2]))
```

```text Output
42
3
1
3.14
42
[1, 2]
False
True
['a', 'b', 'c']
(1, 2, 3)
{1, 2}
```

## Truthy and falsy

These values are falsy. Everything else is truthy.

| Falsy values        |
|---------------------|
| `None`              |
| `False`             |
| `0`, `0.0`          |
| `""` (empty string) |
| `[]`, `()`          |
| `{}`, `set()`       |
| `range(0)`          |