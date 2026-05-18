---
title: "Built-in functions"
description: "Every built-in function in Edge Python with examples and outputs."
---

Edge Python provides 60 built-in functions. They are first-class values: pass them as arguments, store them in containers, alias them.

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

Edge Python is functional-first. Introspection helpers (`eval`, `exec`, `compile`, `dir`, `ascii`, `help`, `__import__`, `breakpoint`, `open`, `issubclass`) are intentionally absent, the static-import contract and the lack of a writable global module table make them either impossible to implement or inconsistent with the paradigm. `staticmethod` and `classmethod` are omitted (use the namespace pattern or free functions); `super` and `property` are supported; see [`/language/classes`](/language/classes) and [`/language/dunders`](/language/dunders).

## Output

### print

`print(*args)` — write space-separated values to stdout, followed by a newline. No `sep`, `end`, `file`, or `flush` keyword arguments — pass a pre-joined string if you need a custom separator.

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

`input()` — read one line from the host's input buffer. Native build: reads stdin. WASM build: drains a buffer the host wrote via `set_input`. Returns the empty string if the buffer is empty. There is no prompt argument.

## Numeric

### abs

`abs(x)` — absolute value of an int or float. `abs("hello")` raises `TypeError` ("abs() requires a number"). Works on both inline ints and `LongInt`-promoted i128 values; literals beyond ±2¹²⁷ are rejected at parse time before `abs` sees them.

```python
print(abs(-7))
print(abs(3.14))
```

```text Output
7
3.14
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

Variadic, or accepting a single iterable. Empty input raises `ValueError`. There is no `key=` or `default=` keyword.

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

`sum(iterable)` or `sum(iterable, start)`. `sum([])` returns `0`.

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

`pow(base, exp)` or `pow(base, exp, mod)` for modular exponentiation. The 3-arg form requires int operands and a non-negative exponent (`pow(a, b, 0)` raises `ZeroDivisionError`; `pow(a, -1, m)` raises `ValueError`). The modulus must be `< 2^63` — larger moduli would overflow i128 during the internal `(result * base) % m` step and raise `ValueError("pow() modulus too large; must be < 2^63")`.

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

`int(x)` — single-argument constructor. Accepts `int`, `bool`, `float` (truncates toward zero), or a numeric string. Strings outside that shape raise `ValueError` ("int(): invalid literal"). Integers up to ±2¹²⁷ are supported (inline 47-bit + `LongInt` i128); values beyond that raise `OverflowError`. **There is no `int(x, base)` form** — parse hex/oct/bin strings yourself or use the `0x`/`0o`/`0b` literal syntax.

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

`float(x)` — accepts `int`, `bool`, `float`, or a string. Strings recognise `inf`, `-inf`, `nan` (case-insensitive) in addition to numeric forms.

```python
print(float(2))
print(float("3.14"))
print(float("inf"))
```

```text Output
2.0
3.14
inf
```

### str

`str(x)` — display form. `str()` with no argument returns the empty string.

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

### list, tuple, set, frozenset, dict

`list`, `tuple`, `set`, and `frozenset` accept any iterable — `list`, `tuple`, `set`, `frozenset`, `dict` (yields keys), `range`, `bytes`, `str`, generator, coroutine. They share a single `extract_iter` helper, so the constructors are interchangeable for any iterable input.

```python
print(list("abc"))
print(tuple(range(3)))
print(set({"a": 1, "b": 2})) # iterates dict keys
print(frozenset(b"\x01\x02\x03"))
print(dict(a=1, b=2))
```

```text Output
['a', 'b', 'c']
(0, 1, 2)
{'a', 'b'}
frozenset({1, 2, 3})
{'a': 1, 'b': 2}
```

`dict` also accepts a single mapping or kwargs; it does not currently accept the iterable-of-pairs constructor form (`dict([('a', 1)])`) — use a literal or `dict.update` with the pair list instead.

### chr, ord

Convert between integer code points and single-character strings. `chr` accepts the full Unicode range (`chr(0x1F600)` returns `"😀"`); negative inputs raise `ValueError`. `ord` requires a length-1 string; `ord(b'A')` is **not** accepted.

```python
print(chr(65))
print(ord("A"))
print(chr(0x1F600))
```

```text Output
A
65
😀
```

## Sequence

### len

Element count for `str` (Unicode code points), `bytes`, `list`, `tuple`, `dict`, `set`, `frozenset`, `range`. Anything else raises `TypeError`.

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

`range(stop)`, `range(start, stop)`, `range(start, stop, step)`. Lazy. `step` of zero raises `ValueError`; non-int arguments raise `TypeError`.

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

Returns a new sorted list. Currently no `key=` or `reverse=` keyword — sort by a derived value via a precomputed list of `(key, value)` tuples.

```python
print(sorted([3, 1, 4, 1, 5]))
print(sorted("hello"))
```

```text Output
[1, 1, 3, 4, 5]
['e', 'h', 'l', 'l', 'o']
```

### reversed

Returns a **list** of elements in reverse order — eager, not a lazy iterator. For strings, the result is a list of length-1 strings; for finite inputs this is operationally equivalent to a lazy iterator.

```python
print(reversed([1, 2, 3]))
print(reversed("abc"))
```

```text Output
[3, 2, 1]
['c', 'b', 'a']
```

### enumerate

Pairs each element with its index, returning a list of `(i, value)` tuples. There is no `start=` keyword — add the offset yourself.

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

Pairs elements from N iterables, truncating to the shortest. No `strict=` keyword — pre-validate lengths if needed.

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

`next(iterator)` retrieves the next item from an iterator. Raises `StopIteration` when exhausted. The two-argument `next(it, default)` form is **not** supported.

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

`iter(x)` returns a fresh iterator over any iterable (list, tuple, set, dict, range, str, bytes, frozenset). The original collection is never mutated — `iter()` materialises a snapshot that `next()` drains front-to-back. The two-argument `iter(callable, sentinel)` form is **not** supported.

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

Truly dynamic loading patterns (`importlib.import_module`, `__import__`) do not exist here by design — the static-import + runtime-dispatch shape above replaces them.

### bytes

Four forms:

- `bytes()` -> empty `bytes`
- `bytes(n)` where `n` is an int -> `n` zero bytes
- `bytes(iterable)` of ints in `0..=255` -> bytes with those values
- `bytes(s, encoding)` where `s` is a `str` -> encoded bytes (`"utf-8"`, `"utf8"`, or `"ascii"` only — anything else raises `ValueError`)

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

### bytes_fromhex, int_from_bytes, int_to_bytes

Edge Python exposes these as **free functions** rather than int/bytes methods. The functional-first paradigm prefers free functions over bound methods on primitive types — there are no `int` or `float` methods at all (no `(5).bit_length()`, no `(255).to_bytes(2, 'big')`).

- `bytes_fromhex(s)` — parse a hex string into bytes. Whitespace inside is ignored; non-hex characters raise `ValueError`.
- `int_from_bytes(b, order)` — `order` is `"big"` or `"little"`. Unsigned only — the high bit is **never** treated as a sign bit.
- `int_to_bytes(n, length, order)` — `n` must be non-negative, `length` ≤ 8. Accepts inline ints or `LongInt`-promoted values; raises `OverflowError` if `n` doesn't fit in `length` bytes.

```python
print(bytes_fromhex("48656c6c6f"))
print(int_from_bytes(b"\x01\x00", "big"))
print(int_to_bytes(255, 2, "big"))
```

```text Output
b'Hello'
256
b'\x00\xff'
```

## Logical reductions

### all, any

```python
print(all([1, 2, 3]))
print(all([1, 0, 3]))
print(all([])) # vacuous truth

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

`type(x)` returns the class-name string `"<class 'name'>"`. It is **not** a class object — there is no `type(...)` constructor form, no metaclass, no introspection. Use it for display and equality checks.

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

`isinstance(obj, X)` accepts built-in types, exception classes, user-defined `Class` objects, and tuples of any of those. The second argument must be one of those — passing a string (`isinstance(x, "str")`) raises `TypeError`. `bool` is a subtype of `int`. For exception classes, the standard subclass hierarchy is consulted (e.g. `isinstance(e, Exception)` is true for any built-in exception). For user classes, it walks the inheritance chain — `isinstance(sub_instance, Base)` is `True` when `Sub` derives from `Base`.

```python
print(isinstance(42, int))
print(isinstance(True, int)) # bool is a subtype of int
print(isinstance("x", (int, str))) # tuple of types
```

```text Output
True
True
True
```

There is no `issubclass` builtin — flat class layout means there's nothing to walk.

### callable

True for user functions, lambdas, bound methods, type objects (callable as constructors), and native built-ins. False for everything else, including instances — there is no `__call__` dispatch.

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

`id(x)` returns a stable identifier (the NaN-box bit pattern masked to int range). `hash(x)` returns a hash for hashable values; `hash(1) == hash(1.0)` so int and float keys collapse to the same dict slot.

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

Mutable containers used as dict keys or set members raise `TypeError("unhashable type")` at insertion — caught at `store_item`, `BuildDict`, and `build_set`.

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

`getattr` and `hasattr` consult the built-in method table for primitive types (str/list/dict/set/bytes) and the instance `__dict__` on user-class instances. They do **not** walk user-class method definitions — `hasattr(MyClass(), 'my_method')` returns `False`. The functional pattern is to call functions with values, not look up methods reflectively.

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

### globals, locals

`globals()` returns a fresh dict snapshot of the module-level bindings: every name registered as a builtin or type, plus every top-level user assignment. `locals()` returns a fresh dict of the current frame's bindings — function locals when called inside a function, the same set as `globals()` when called at module level (with builtins filtered).

```python
x = 100
y = 200

def add(a, b):
    return a + b

g = globals()
print(g['x'] + g['y'])

# Dynamic dispatch by name
fn = globals()['add']
print(fn(3, 4))

def f():
    a = 1
    b = 2
    return locals()
print(f())
```

```text Output
300
7
{'a': 1, 'b': 2}
```

The dicts are copies — mutating them does not change the VM's bindings.

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

`vars(instance)` returns a snapshot of the instance's attribute dict. `vars(module)` returns a dict of the module's exported names. **Only instances and modules are accepted** — there is no no-arg form returning the local frame; use `locals()` instead.

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

## Classes

### super

`super()` — zero-arg form only. Returns a proxy that resolves attribute access against the bases of the currently-running method's class, starting one step up the MRO. Calling it outside a method raises `TypeError`.

```python
class A:
    def m(self):
        return "a"

class B(A):
    def m(self):
        return super().m() + "b"

print(B().m())
```

```text Output
ab
```

### property

`property(fget, fset=None)` — builds a descriptor for use as a class member. Usually applied via the `@property` decorator, with an optional `@<name>.setter` decorator to attach the setter.

```python
class C:
    def __init__(self, x):
        self._x = x
    @property
    def x(self):
        return self._x
    @x.setter
    def x(self, v):
        self._x = v

c = C(1)
c.x = 9
print(c.x)
```

```text Output
9
```

## Async

These primitives are top-level builtins, not under `asyncio` — there is no `asyncio` module to import.

### run

`run(*coros)` — drive the cooperative scheduler until the **first** argument coroutine is done; additional coroutines are added to the scheduler and run concurrently. Returns the first coroutine's result.

### sleep

`sleep(seconds)` — yield and resume after the given duration. Negative values clamp to zero. With no host time hook the virtual clock advances; with one wired, the scheduler signals `PendingTimer(deadline_ns)` and the embedder resumes via `run_resume`.

### frame

`frame()` — yield until the host's next render frame. The coro transitions to `WaitingFrame` and the scheduler signals `PendingFrame`; browser embedders hook `requestAnimationFrame`. Use for animation loops driven at the display refresh rate.

```python
async def animate(node):
    for i in range(60):
        set_attribute(node, "style", f"transform: translateX({i}px)")
        frame()
```

### receive

`receive()` — pop the oldest message from the scheduler's event queue. Parks the coro in `WaitingEvent` when empty; the scheduler signals `PendingEvent` and the embedder resumes once `run_push_event(bytes)` injects a message. Messages are arbitrary strings (e.g. DOM event names from `bind_event`).

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

| Function          | Arity      | Notes                                      |
|-------------------|------------|--------------------------------------------|
| `print`           | variadic   | space-separated, newline; no kwargs        |
| `input`           | 0          | reads from host-provided buffer            |
| `abs`             | 1          | int / float                                |
| `round`           | 1 or 2     | banker's rounding                          |
| `min`             | variadic   | or single iterable; empty raises           |
| `max`             | variadic   | or single iterable; empty raises           |
| `sum`             | 1 or 2     | optional start (defaults to `0`)           |
| `pow`             | 2 or 3     | 3-arg = modular (int, non-negative exp)    |
| `divmod`          | 2          | returns `(q, r)`                           |
| `bin`             | 1          | `0b...` prefix                             |
| `oct`             | 1          | `0o...` prefix                             |
| `hex`             | 1          | `0x...` prefix                             |
| `int`             | 1          | 1-arg only; no base form; 128-bit cap      |
| `float`           | 1          | parse / cast; recognises `inf`/`nan`       |
| `str`             | 0 or 1     | display form                               |
| `bool`            | 0 or 1     | truthiness                                 |
| `list`            | 0 or 1     | from any iterable                          |
| `tuple`           | 0 or 1     | from any iterable                          |
| `set`             | 0 or 1     | from any iterable                          |
| `frozenset`       | 0 or 1     | immutable set                              |
| `dict`            | variadic   | kwargs and/or single mapping               |
| `chr`             | 1          | int -> 1-char string (full Unicode)         |
| `ord`             | 1          | length-1 string -> int                      |
| `len`             | 1          | element count (str = code points)          |
| `range`           | 1, 2, or 3 | lazy integer sequence                      |
| `sorted`          | 1          | new sorted list; no `key=` / `reverse=`    |
| `reversed`        | 1          | reversed as list (eager)                   |
| `enumerate`       | 1          | `(index, value)` pairs                     |
| `zip`             | variadic   | parallel iteration; truncates to shortest  |
| `iter`            | 1          | fresh iterator over any iterable           |
| `next`            | 1          | next item; no default form                 |
| `map`             | 2          | returns list (eager)                       |
| `filter`          | 2          | returns list; `None` filters by truthiness |
| `all`             | 1          | logical AND; `all([])` is `True`           |
| `any`             | 1          | logical OR; `any([])` is `False`           |
| `type`            | 1          | display string `<class 'name'>`            |
| `isinstance`      | 2          | type, user class, exception, or tuple      |
| `super`           | 0          | proxy to current method's class bases      |
| `property`        | 1 or 2     | descriptor; usually via `@property`        |
| `callable`        | 1          | True for fn / lambda / type / built-in     |
| `id`              | 1          | stable identifier                          |
| `hash`            | 1          | hash for hashable values                   |
| `repr`            | 1          | developer-readable form                    |
| `format`          | 1 or 2     | f-string format-spec mini-language         |
| `getattr`         | 2 or 3     | bound method, instance attr, or default    |
| `hasattr`         | 2          | True for built-in method or instance attr  |
| `setattr`         | 3          | write attr on user instance                |
| `delattr`         | 2          | remove attr from user instance             |
| `vars`            | 1          | snapshot dict of instance / module         |
| `globals`         | 0          | snapshot dict of module-level bindings     |
| `locals`          | 0          | snapshot dict of current frame             |
| `slice`           | 1, 2, or 3 | reusable slice object                      |
| `bytes`           | 0, 1, or 2 | empty / size / iterable / `(s, encoding)`  |
| `bytes_fromhex`   | 1          | parse hex string -> bytes                   |
| `int_from_bytes`  | 2          | bytes + `"big"`/`"little"` -> int           |
| `int_to_bytes`    | 3          | int + length (≤8) + order -> bytes          |
| `import_module`   | 1          | runtime lookup of statically-imported module |
| `run`             | variadic   | drive scheduler until first arg done       |
| `sleep`           | 1          | yield then resume after seconds            |
| `frame`           | 0          | yield until host renders next frame        |
| `gather`          | variadic   | concurrent fan-out; first error cancels peers |
| `with_timeout`    | 2          | `seconds, coro` -> result or `TimeoutError` |
| `cancel`          | 1          | mark coroutine cancel-pending              |
| `receive`         | 0          | pop oldest queued host message             |
