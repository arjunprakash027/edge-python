---
title: "Built-in functions"
description: "Every built-in function in Edge Python with examples and outputs."
---

60 built-in functions, all first-class values — pass as arguments, store in containers, alias.

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

Edge Python is functional-first. Introspection helpers (`eval`, `exec`, `compile`, `dir`, `ascii`, `help`, `__import__`, `breakpoint`, `open`, `issubclass`) are intentionally absent — the static-import contract and lack of a writable global module table make them either impossible or inconsistent with the paradigm. `staticmethod` / `classmethod` are omitted (use the namespace pattern or free functions); `super` and `property` are supported. See [`/language/classes`](/language/classes), [`/language/dunders`](/language/dunders).

## Output

### print

`print(*args)` — space-separated values to stdout, trailing newline. No `sep` / `end` / `file` / `flush` kwargs — pre-join for custom separators.

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

`input()` — one line from the host buffer. Native: stdin. WASM: drains the buffer the host wrote via `set_input`. Empty buffer → empty string. No prompt argument.

## Numeric

### abs

`abs(x)` — absolute value of int or float. Non-numeric → `TypeError`. Works on inline and `LongInt` i128; literals beyond ±2¹²⁷ are rejected at parse time.

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

Variadic or single iterable. Empty → `ValueError`. No `key=` / `default=`.

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

`pow(base, exp)` or `pow(base, exp, mod)` for modular exp. 3-arg requires int operands and non-negative exp (`pow(a, b, 0)` → `ZeroDivisionError`; `pow(a, -1, m)` → `ValueError`). Modulus must be `< 2^63` (larger overflows i128 in `(result * base) % m`) — raises `ValueError("pow() modulus too large; must be < 2^63")`.

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

`int(x)` — single-arg. Accepts `int`, `bool`, `float` (truncates toward zero), numeric string. Bad strings → `ValueError`. Supports ±2¹²⁷ (inline 47-bit + `LongInt` i128); wider → `OverflowError`. No `int(x, base)` form — parse hex/oct/bin yourself or use `0x` / `0o` / `0b` literals.

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

`float(x)` — `int`, `bool`, `float`, or string. Strings recognise `inf`, `-inf`, `nan` (case-insensitive).

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

`str(x)` — display form. No arg → empty string.

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

`list`, `tuple`, `set`, `frozenset` accept any iterable — list, tuple, set, frozenset, dict (keys), range, bytes, str, generator, coroutine. Share an `extract_iter` helper, so all constructors are interchangeable.

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

`dict` also accepts a mapping or kwargs; iterable-of-pairs (`dict([('a', 1)])`) is not supported — use a literal or `dict.update`.

### chr, ord

Convert between code points and single-char strings. `chr` accepts full Unicode (`chr(0x1F600)` → `"😀"`); negative → `ValueError`. `ord` requires a length-1 string; `ord(b'A')` not accepted.

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

Element count for `str` (code points), `bytes`, `list`, `tuple`, `dict`, `set`, `frozenset`, `range`. Else `TypeError`.

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

`range(stop)`, `range(start, stop)`, `range(start, stop, step)`. Lazy. `step=0` → `ValueError`; non-int args → `TypeError`.

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

New sorted list. No `key=` / `reverse=` — sort by derived value via a precomputed list of `(key, value)` tuples.

```python
print(sorted([3, 1, 4, 1, 5]))
print(sorted("hello"))
```

```text Output
[1, 1, 3, 4, 5]
['e', 'h', 'l', 'l', 'o']
```

### reversed

Returns a list (eager, not a lazy iterator). For strings: list of length-1 strings. Operationally equivalent to a lazy iterator for finite inputs.

```python
print(reversed([1, 2, 3]))
print(reversed("abc"))
```

```text Output
[3, 2, 1]
['c', 'b', 'a']
```

### enumerate

Pairs each element with its index → list of `(i, value)` tuples. No `start=` — add the offset yourself.

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

Pairs N iterables, truncating to shortest. No `strict=` — pre-validate lengths if needed.

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

`next(iterator)` → next item. Exhausted → `StopIteration`. Two-arg `next(it, default)` not supported.

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

`iter(x)` returns a fresh iterator over any iterable (list, tuple, set, dict, range, str, bytes, frozenset). Materialises a snapshot — original never mutated. Two-arg `iter(callable, sentinel)` not supported.

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

`map(fn, iterable)` → list of `fn(item)`. Eager — full list materialises immediately; pipelines into `sum`, `list`, `max`.

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

`filter(pred, iterable)` → list of items where `pred(item)` is truthy. `None` predicate filters by truthiness (equivalent to `lambda x: x`).

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

`import_module(name)` returns a module previously imported statically. Runtime dispatch among pre-imported modules without a manual dict.

```python
import prod_handler
import dev_handler

def handle(env, request):
    return import_module(env + "_handler").handle(request)

handle("prod", req)
handle("dev",  req)
```

Candidates must be imported statically somewhere — `import_module` is a runtime *lookup*, not a *fetch*. Preserves lockfile and integrity: every reachable module is verified at compile time. Unknown name → `NameError`; non-module global → `TypeError`.

Dynamic loading (`importlib.import_module`, `__import__`) doesn't exist by design — static-import + runtime-dispatch replaces it.

### bytes

Four forms:

- `bytes()` → empty
- `bytes(n)` → `n` zero bytes
- `bytes(iterable)` of ints in `0..=255` → those values
- `bytes(s, encoding)` → encoded (`"utf-8"`, `"utf8"`, `"ascii"` only; else `ValueError`)

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

See [Bytes](/language/data-types#bytes) for literal syntax (`b"..."`), indexing, slicing, methods.

### bytes_fromhex, int_from_bytes, int_to_bytes

Free functions, not int/bytes methods — primitives have no bound methods (`(5).bit_length()`, `(255).to_bytes(...)` don't exist).

- `bytes_fromhex(s)` — hex string → bytes. Inner whitespace ignored; non-hex → `ValueError`.
- `int_from_bytes(b, order)` — `order` is `"big"` or `"little"`. Unsigned (high bit never sign).
- `int_to_bytes(n, length, order)` — `n ≥ 0`, `length ≤ 8`. Accepts inline ints or `LongInt`; doesn't fit → `OverflowError`.

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

`type(x)` returns the class-name string `"<class 'name'>"` — not a class object. No `type(...)` constructor form, no metaclass, no introspection. For display and equality checks.

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

`isinstance(obj, X)` — `X` is a built-in type, exception class, user-defined `Class`, or tuple of any of those. String `X` (`isinstance(x, "str")`) → `TypeError`. `bool` is a subtype of `int`. Exception classes walk the standard hierarchy (`isinstance(e, Exception)` matches any built-in exception). User classes walk the inheritance chain.

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

No `issubclass` builtin — flat class layout has nothing to walk.

### callable

True for user functions, lambdas, bound methods, type objects, native builtins. False for everything else, including instances — no `__call__` dispatch.

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

`id(x)` — stable identifier (NaN-box bit pattern masked to int range). `hash(x)` — hash for hashable values; `hash(1) == hash(1.0)` so int/float keys collapse to one dict slot.

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

Mutable containers used as dict keys / set members → `TypeError("unhashable type")` at insertion (caught in `store_item`, `BuildDict`, `build_set`).

## Representation

### repr

Developer-readable form. Quotes strings; renders containers with element `repr`s.

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

`format(value)` → display form. `format(value, spec)` applies the f-string spec mini-language (`[[fill]align][sign][#][0][width][,][.precision][type]`).

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

`getattr` / `hasattr` consult the built-in method table for primitives (str/list/dict/set/bytes) and the instance `__dict__` for user-class instances. They don't walk user-class method definitions — `hasattr(MyClass(), 'my_method')` is `False`. Functional pattern: call functions with values, don't look up methods reflectively.

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

`globals()` — fresh dict snapshot of module-level bindings (builtins, types, top-level assignments). `locals()` — fresh dict of the current frame: function locals inside a function, same as `globals()` minus builtins at module level.

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

Dicts are copies — mutation doesn't change VM bindings.

### setattr, delattr

`setattr(obj, name, value)` / `delattr(obj, name)` store/remove on user instances. Builtin types have no writable attribute table.

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

`slice(stop)`, `slice(start, stop)`, `slice(start, stop, step)` — reusable slice value usable as a sequence index.

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

`vars(instance)` → attr-dict snapshot. `vars(module)` → exported-names dict. Only instances and modules — no no-arg form (use `locals()`).

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

`super()` — zero-arg only. Proxy resolving attribute access against the bases of the current method's class, starting one step up the MRO. Outside a method → `TypeError`.

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

`property(fget, fset=None)` — descriptor for class members. Usually applied via `@property` with optional `@<name>.setter`.

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

Top-level builtins, no `asyncio` module.

### run

`run(*coros)` — schedules every arg, drains until each reaches a terminal state, returns the first arg's result. Errors from peers other than the first are discarded — for fan-out collecting every result, use `gather`.

### sleep

`sleep(seconds)` — yield and resume after the duration. Negative clamps to zero. Without a host time hook, a virtual clock advances; with one, scheduler signals `PendingTimer(deadline_ns)` and the embedder resumes via `run_resume`.

### frame

`frame()` — yield until the host's next render frame. Coro → `WaitingFrame`, scheduler signals `PendingFrame`; browser embedders hook `requestAnimationFrame`. Use for animation loops at display refresh rate.

```python
async def animate(node):
    for i in range(60):
        set_attribute(node, "style", f"transform: translateX({i}px)")
        frame()
```

### receive

`receive()` — pop the oldest message from the scheduler queue. Empty → parks in `WaitingEvent`, scheduler signals `PendingEvent`; embedder resumes via `run_push_event(bytes)`. Messages are arbitrary strings (e.g. DOM event names from `bind_event`).

### gather

`gather(*coros)` — concurrent fan-out. Schedules every arg, drains until each terminal, returns a list of results in argument order. First-error propagates after all peers terminal.

```python
async def task(n):
    return n * 2

print(gather(task(1), task(2), task(3)))
```

```text Output
[2, 4, 6]
```

### with_timeout

`with_timeout(seconds, coro)` — runs `coro` to completion or raises `TimeoutError` on deadline. Coro cancelled on timeout.

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

`cancel(coro)` — flag a registered coroutine for cancellation; next tick stops it. Cooperative and silent — body doesn't observe `CancelledError`. For deadline-driven exception-style cancellation, use `with_timeout`.

