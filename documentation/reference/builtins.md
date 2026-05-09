---
title: "Built-in functions"
description: "Every built-in function in Edge Python with examples and outputs."
---

Edge Python ships with 50 built-in functions. They're first-class values: pass them around, store them in containers, alias them.

```python
# All built-ins are real values
fns = [abs, len, str]
print([f(-3) for f in fns])

p = print
p("aliased")
```

```text Output
[3, 2, '-3']
aliased
```

## Output

### print

`print(*args)` — write space-separated values to stdout, followed by a newline.

```python
print(1, 2, 3)
print("hello", "world")
print()
```

```text Output
1 2 3
hello world

```

### input

`input()` — Reads from host-provided input buffer. Native: reads stdin. WASM: requires `set_input` FFI. Returns empty string if no data.

## Numeric

### abs

`abs(x)` — absolute value.

```python
print(abs(-7))
print(abs(3.14))
print(abs(-2 ** 100))
```

```text Output
7
3.14
1267650600228229401496703205376
```

### round

`round(x)` or `round(x, n)` — banker's rounding (ties go to even).

```python
print(round(2.5))
print(round(0.5))
print(round(-1.5))
print(round(1.55, 1))
```

```text Output
2
0
-2
1.6
```

### min, max

Variadic, or accepting a single iterable.

```python
print(min(3, 1, 4))
print(max([3, 1, 4]))
print(min("hello"))
```

```text Output
1
4
e
```

### sum

`sum(iterable)` or `sum(iterable, start)`.

```python
print(sum([1, 2, 3]))
print(sum([1, 2, 3], 100))
print(sum(x * x for x in range(5)))
```

```text Output
6
106
30
```

### pow

`pow(base, exp)` or `pow(base, exp, mod)` for modular exponentiation.

```python
print(pow(2, 10))
print(pow(2, 10, 1000))
print(pow(7, 13, 19))
```

```text Output
1024
24
7
```

### divmod

`divmod(a, b)` — `(a // b, a % b)` as a tuple.

```python
print(divmod(7, 3))
print(divmod(-7, 3))
```

```text Output
(2, 1)
(-3, 2)
```

### bin, oct, hex

Format an integer as a base-2, base-8, or base-16 string with prefix.

```python
print(bin(10))
print(oct(8))
print(hex(255))
print(hex(-256))
```

```text Output
0b1010
0o10
0xff
-0x100
```

## Type conversion

### int

```python
print(int(3.9))
print(int("42"))
print(int(True))
```

```text Output
3
42
1
```

### float

```python
print(float(2))
print(float("3.14"))
```

```text Output
2.0
3.14
```

### str

```python
print(str(42))
print(str([1, 2, 3]))
print(str(None))
```

```text Output
42
[1, 2, 3]
None
```

### bool

```python
print(bool(0), bool(1))
print(bool([]), bool([0]))
print(bool(""), bool("x"))
```

```text Output
False True
False True
False True
```

### list, tuple, set, dict

```python
print(list("abc"))
print(tuple([1, 2, 3]))
print(set([1, 1, 2, 3]))
print(dict(a=1, b=2))
```

```text Output
['a', 'b', 'c']
(1, 2, 3)
{1, 2, 3}
{'a': 1, 'b': 2}
```

### chr, ord

Convert between integer code points and single-character strings.

```python
print(chr(65))
print(ord("A"))
```

```text Output
A
65
```

## Sequence

### len

```python
print(len("hello"))
print(len([1, 2, 3, 4]))
print(len({"a": 1, "b": 2}))
print(len(range(100)))
```

```text Output
5
4
2
100
```

### range

`range(stop)`, `range(start, stop)`, `range(start, stop, step)`. Lazy.

```python
print(list(range(5)))
print(list(range(2, 8)))
print(list(range(10, 0, -2)))
```

```text Output
[0, 1, 2, 3, 4]
[2, 3, 4, 5, 6, 7]
[10, 8, 6, 4, 2]
```

### sorted

Returns a new sorted list.

```python
print(sorted([3, 1, 4, 1, 5]))
print(sorted("hello"))
```

```text Output
[1, 1, 3, 4, 5]
['e', 'h', 'l', 'l', 'o']
```

### reversed

Returns a list of elements in reverse order.

```python
print(reversed([1, 2, 3]))
print(reversed("abc"))
```

```text Output
[3, 2, 1]
['c', 'b', 'a']
```

### enumerate

Pairs each element with its index.

```python
for i, v in enumerate(["a", "b", "c"]):
    print(i, v)
```

```text Output
0 a
1 b
2 c
```

### zip

Pairs elements from N iterables, truncating to the shortest.

```python
for a, b in zip([1, 2, 3], ["x", "y", "z"]):
    print(a, b)

print(list(zip([1, 2], [3, 4], [5, 6])))
```

```text Output
1 x
2 y
3 z
[(1, 3, 5), (2, 4, 6)]
```

### next

`next(iterator)` retrieves the next item from an iterator. Raises `StopIteration` if exhausted.

```python
it = iter([10, 20, 30])
print(next(it))
print(next(it))
print(next(it))
```

```text Output
10
20
30
```

### iter

`iter(x)` returns a fresh iterator over any iterable (list, tuple, set, dict, range, str). The original collection is never mutated — `iter()` materialises a copy that `next()` drains front-to-back.

```python
it = iter([1, 2, 3])
print(next(it))
print(next(it))

# Strings yield characters
chars = iter("abc")
print(next(chars))
```

```text Output
1
2
a
```

### map

`map(fn, iterable)` applies `fn` to each item and returns a list. Eager — the full list materialises immediately, suitable for pipelines into `sum`, `list`, `max`, etc.

```python
print(list(map(lambda x: x * 2, [1, 2, 3])))
print(sum(map(lambda x: x * x, range(5))))

def normalize(s):
    return s.strip().lower()

print(list(map(normalize, ["  Hi ", "WORLD"])))
```

```text Output
[2, 4, 6]
30
['hi', 'world']
```

### filter

`filter(pred, iterable)` keeps items where `pred(item)` is truthy. Returns a list. A `None` predicate filters by truthiness directly (equivalent to `lambda x: x`).

```python
print(list(filter(lambda x: x > 2, [1, 2, 3, 4])))

# `None` keeps any truthy value
print(list(filter(None, [0, 1, "", "hi", [], [1]])))
```

```text Output
[3, 4]
[1, 'hi', [1]]
```

### import_module

`import_module(name)` returns a module value previously brought into scope via a static `import` or `from ... import` statement. Lets the script choose at runtime which of several pre-imported modules to use, without keeping a manual dispatch dict.

```python
import prod_handler
import dev_handler

def handle(env, request):
    return import_module(env + "_handler").handle(request)

handle("prod", req)
handle("dev",  req)
```

The candidate modules **must be imported statically** somewhere — `import_module` is a runtime *lookup*, not a runtime *fetch*. This preserves the lockfile and integrity guarantees: every module the script can ever reach is known and verified at compile time. Calling `import_module(name)` where `name` was never imported raises `NameError`; calling it on a non-module global (e.g. a builtin function) raises `TypeError`.

If you want truly dynamic loading patterns from CPython (`importlib.import_module`, `__import__`), they don't exist here by design — the static-import + runtime-dispatch shape above replaces them.

### bytes

Three forms:

- `bytes()` → empty `bytes`
- `bytes(n)` where `n` is an int → `n` zero bytes
- `bytes(iterable)` of ints in `0..=255` → bytes with those values
- `bytes(s, encoding)` where `s` is a `str` → encoded bytes (`"utf-8"` or `"ascii"`)

```python
print(bytes())
print(bytes(4))
print(bytes([72, 101, 108, 108, 111]))
print(bytes("café", "utf-8"))
```

```text Output
b''
b'\x00\x00\x00\x00'
b'Hello'
b'caf\xc3\xa9'
```

See [Bytes](/language/data-types#bytes) in the data-types reference for the literal syntax (`b"..."`), indexing, slicing, and methods.

## Logical reductions

### all, any

```python
print(all([1, 2, 3]))
print(all([1, 0, 3]))
print(all([]))           # vacuous truth

print(any([0, 0, 1]))
print(any([0, 0, 0]))
print(any([]))
```

```text Output
True
False
True
True
False
False
```

## Type and identity

### type

```python
print(type(42))
print(type("hi"))
print(type([1, 2]))
print(type(print))
```

```text Output
<class 'int'>
<class 'str'>
<class 'list'>
<class 'builtin_function_or_method'>
```

### isinstance

```python
print(isinstance(42, int))
print(isinstance(True, int))           # bool is a subtype of int
print(isinstance("x", (int, str)))     # tuple of types
```

```text Output
True
True
True
```

### callable

```python
print(callable(print))
print(callable(lambda x: x))
print(callable(42))
print(callable("hello"))
```

```text Output
True
True
False
False
```

### id, hash

`id(x)` returns a unique identifier for the value. `hash(x)` returns a hash for hashable values.

```python
x = 42
print(id(x) == id(x))
print(hash("hello") == hash("hello"))
print(hash((1, 2, 3)) == hash((1, 2, 3)))
```

```text Output
True
True
True
```

```python
# Lists, dicts, sets are unhashable
try:
    hash([1, 2, 3])
except TypeError:
    print("unhashable")
```

```text Output
unhashable
```

## Representation

### repr

The "developer-readable" form. Quotes strings; renders containers with their elements as `repr`.

```python
print(repr("hello"))
print(repr(42))
print(repr([1, "two", 3]))
```

```text Output
'hello'
42
[1, 'two', 3]
```

### format

`format(value)` returns the display form. `format(value, spec)` applies the same format-spec mini-language as f-strings (`[[fill]align][sign][#][0][width][,][.precision][type]`).

```python
print(format(42))
print(format(42, "05d"))
print(format(3.14159, ".2f"))
print(format(255, "#x"))
print(format("hi", ">10"))
```

```text Output
42
00042
3.14
0xff
        hi
```

## Attribute access

`getattr` and `hasattr` work against the built-in method tables on strings, lists, and dicts. User-defined class attributes are also supported.

### getattr

```python
m = getattr("hello", "upper")
print(m())
print(getattr("hello", "missing", "default"))
```

```text Output
HELLO
default
```

### hasattr

```python
print(hasattr("hello", "upper"))
print(hasattr([1, 2], "append"))
print(hasattr("hello", "missing"))
```

```text Output
True
True
False
```

### setattr, delattr

`setattr(obj, name, value)` stores an attribute on a user instance. `delattr(obj, name)` removes one. Both target instances of user-defined classes; builtin types do not have a writable attribute table.

```python
class Box:
    def __init__(self):
        pass

b = Box()
setattr(b, "x", 42)
print(b.x)
delattr(b, "x")
print(hasattr(b, "x"))
```

```text Output
42
False
```

### slice

`slice(stop)`, `slice(start, stop)`, or `slice(start, stop, step)` builds a reusable slice value that can be used as a sequence index.

```python
xs = [10, 20, 30, 40, 50]
s = slice(1, 4)
print(xs[s])
print(xs[slice(0, 5, 2)])
```

```text Output
[20, 30, 40]
[10, 30, 50]
```

### vars

`vars(instance)` returns a snapshot of the instance's `__dict__`. `vars(module)` returns a dict of the module's exported names.

```python
class P:
    def __init__(self):
        self.x = 1
        self.y = 2

p = P()
print(vars(p))
```

```text Output
{'x': 1, 'y': 2}
```

## Async

### run

`run(coroutine)` — run a coroutine to completion using the cooperative event loop.

### sleep

`sleep(seconds)` — suspend the current coroutine for the given duration. Only valid inside `async def`.

### receive

`receive()` — suspend the current coroutine until a message is available from the host. Only valid inside `async def`.

### gather

`gather(*coros)` — concurrent fan-out. Adds every argument to the scheduler, drains until each is terminal, returns a list of their results in argument order. First error cancels remaining peers and propagates.

```python
async def task(n):
    return n * 2

print(gather(task(1), task(2), task(3)))
```

```text Output
[2, 4, 6]
```

### with_timeout

`with_timeout(seconds, coro)` — runs `coro` to completion or raises `TimeoutError` if the deadline elapses first. The coroutine is cancelled on timeout.

```python
async def slow():
    sleep(10)
    return "never"

try:
    with_timeout(0.1, slow())
except TimeoutError:
    print("timed out")
```

```text Output
timed out
```

### cancel

`cancel(coro)` — flag a coroutine registered with the scheduler for cancellation. The next scheduler tick stops it. Cancellation is cooperative and silent: the coroutine body does not observe a raised `CancelledError`. For deadline-driven cancellation that propagates as an exception, use `with_timeout`.

## Built-in summary

| Function     | Arity      | Notes                                      |
|--------------|------------|--------------------------------------------|
| `print`      | variadic   | space-separated, newline                   |
| `input`      | 0          | reads from host-provided buffer            |
| `abs`        | 1          | int / float                                |
| `round`      | 1 or 2     | banker's rounding                          |
| `min`        | variadic   | or single iterable                         |
| `max`        | variadic   | or single iterable                         |
| `sum`        | 1 or 2     | optional start                             |
| `pow`        | 2 or 3     | 3-arg = modular                            |
| `divmod`     | 2          | returns `(q, r)`                           |
| `bin`        | 1          | `0b...` prefix                             |
| `oct`        | 1          | `0o...` prefix                             |
| `hex`        | 1          | `0x...` prefix                             |
| `int`        | 0 or 1     | parse / truncate                           |
| `float`      | 0 or 1     | parse / cast                               |
| `str`        | 0 or 1     | display form                               |
| `bool`       | 0 or 1     | truthiness                                 |
| `list`       | 0 or 1     | from any iterable                          |
| `tuple`      | 0 or 1     | from any iterable                          |
| `set`        | 0 or 1     | from any iterable                          |
| `dict`       | variadic   | kwargs and/or single mapping               |
| `chr`        | 1          | int -> 1-char string                        |
| `ord`        | 1          | 1-char string -> int                        |
| `len`        | 1          | element count                              |
| `range`      | 1, 2, or 3 | lazy integer sequence                      |
| `sorted`     | 1 or 2     | optional `key=` callable                   |
| `reversed`   | 1          | reversed as list                           |
| `enumerate`  | 1          | (index, value) pairs                       |
| `zip`        | variadic   | parallel iteration                         |
| `all`        | 1          | logical AND over iterable                  |
| `any`        | 1          | logical OR over iterable                   |
| `type`       | 1          | type name                                  |
| `isinstance` | 2          | type or tuple of types                     |
| `callable`   | 1          | True for functions, lambdas, types, builtins |
| `id`         | 1          | unique identifier                          |
| `hash`       | 1          | hash for hashable values                   |
| `repr`       | 1          | developer-readable form                    |
| `format`     | 1 or 2     | applies the same format-spec mini-language as f-strings |
| `getattr`    | 2 or 3     | bound method or default                    |
| `hasattr`    | 2          | True if method exists                      |
| `next`       | 1 or 2     | next item from iterator                    |
| `run`        | 1          | run coroutine to completion                |
| `sleep`      | 1          | suspend coroutine for duration             |
| `receive`    | 0          | suspend coroutine until message available  |
