---
title: "Dunder methods"
description: "Protocol methods Edge Python invokes on user classes; operators, indexing, iteration, hashing, context managers, attribute fallback."
---

Dunders ("double-underscore" methods like `__add__`, `__eq__`, `__getitem__`) are how a class plugs into the language's protocols. Define them on the class body and the VM calls them when the corresponding operator, builtin, or syntax form runs.

```python
class V:
    def __init__(self, n):
        self.n = n
    def __add__(self, o):
        return V(self.n + o.n)
    def __eq__(self, o):
        return self.n == o.n

print((V(3) + V(4)).n)
print(V(3) == V(3))
```

```text Output
7
True
```

Dunders are looked up on the class chain (instance dict is skipped). A subclass inherits every dunder defined on its bases and can override any of them — operator overloading composes naturally with [single-level inheritance](/language/classes#inheritance-and-super). A monomorphic site — same class for both operands across iterations; promotes through the inline cache after four hits and bypasses the lookup entirely on subsequent calls.

## Arithmetic

| Operator | Forward         | Reflected        |
|----------|-----------------|------------------|
| `a + b`  | `__add__`       | `__radd__`       |
| `a - b`  | `__sub__`       | `__rsub__`       |
| `a * b`  | `__mul__`       | `__rmul__`       |
| `a / b`  | `__truediv__`   | `__rtruediv__`   |
| `a // b` | `__floordiv__`  | `__rfloordiv__`  |
| `a % b`  | `__mod__`       | `__rmod__`       |
| `a ** b` | `__pow__`       | `__rpow__`       |
| `-a`     | `__neg__`       | —                |

Returning `NotImplemented` from the forward op tells the VM to try the reflected op on the other operand. If both return `NotImplemented` (or neither is defined), the VM raises `TypeError`.

Subclass-first ordering: when `type(b)` is a strict subclass of `type(a)`, `b.__radd__` runs **before** `a.__add__`. This is the standard and lets a subclass override an inherited reflected op without touching the base.

```python
class Money:
    def __init__(self, n): self.n = n
    def __add__(self, o):
        return Money(self.n + (o.n if isinstance(o, Money) else o))
    def __radd__(self, o):
        return Money(o + self.n)

print((Money(10) + Money(5)).n)
print((3 + Money(7)).n)
```

```text Output
15
10
```

## Comparison

| Operator   | Forward     | Reflected  |
|------------|-------------|------------|
| `a == b`   | `__eq__`    | `__eq__`   |
| `a != b`   | `__eq__`    | `__eq__`   |
| `a < b`    | `__lt__`    | `__gt__`   |
| `a <= b`   | `__le__`    | `__ge__`   |
| `a > b`    | `__gt__`    | `__lt__`   |
| `a >= b`   | `__ge__`    | `__le__`   |

`!=` falls back to `not __eq__` when `__ne__` is absent. Comparison results are coerced to `bool`; returning `'A.lt'` from `__lt__` yields `True` in `a < b`, not the string.

## Truth and length

`bool(x)` (and any boolean context like `if x:`) consults:

1. `__bool__` if defined -> cast to bool.
2. `__len__` if defined -> `False` when length is 0, `True` otherwise.
3. Default `True`.

`len(x)` calls `__len__` directly; the return must be a non-negative integer.

```python
class Empty:
    def __bool__(self):
        return False

class Container:
    def __init__(self, n): self.n = n
    def __len__(self):
        return self.n

print(bool(Empty()))
print(bool(Container(0)), bool(Container(3)))
print(len(Container(5)))
```

```text Output
False
False True
5
```

## Indexing and containment

| Form                | Dunder           | Arguments              |
|---------------------|------------------|------------------------|
| `obj[i]`            | `__getitem__`    | `(self, i)`            |
| `obj[i] = v`        | `__setitem__`    | `(self, i, value)`     |
| `del obj[i]`        | `__delitem__`    | `(self, i)`            |
| `v in obj`          | `__contains__`   | `(self, value)`        |

Slices are passed as a single `slice` object: `obj[1:3]` calls `__getitem__(self, slice(1, 3, None))`.

When `__contains__` is absent, `v in obj` falls back to iterating `obj` and comparing each item with `__eq__`.

## Iteration

| Method        | Role                                                                 |
|---------------|----------------------------------------------------------------------|
| `__iter__`    | Returns an iterator (often `self`).                                  |
| `__next__`    | Returns the next item, or raises `StopIteration` to end the loop.    |

```python
class Up:
    def __init__(self, stop):
        self.i = 0
        self.stop = stop
    def __iter__(self):
        return self
    def __next__(self):
        if self.i >= self.stop:
            raise StopIteration
        self.i += 1
        return self.i

print(list(Up(3)))
```

```text Output
[1, 2, 3]
```

`for` loops, `list(x)`, and `tuple(x)` all honour the protocol.

## Callable

`__call__` makes instances invocable.

```python
class Double:
    def __call__(self, x):
        return x * 2

d = Double()
print(d(7))
print(callable(d))
```

```text Output
14
True
```

## Hashing

`hash(x)` calls `__hash__`. The result must be an `int`; the VM masks it to fit `INT_MAX`.

Eq/hash invariant: a class that defines `__eq__` **without** `__hash__` is unhashable; `hash(x)` and `{x: 1}` raise `TypeError`. This prevents inconsistent dict keys.

```python
class K:
    def __init__(self, n): self.n = n
    def __hash__(self):
        return self.n
    def __eq__(self, o):
        return self.n == o.n

print(hash(K(5)))
print({K(1): 'one'}[K(1)] if K(1).__hash__() == K(1).__hash__() else 'unhashable')
```

```text Output
5
one
```

Built-in dict and set still compare instance keys by identity (`Val` bit pattern); the user `__hash__` is returned by `hash()` but doesn't change containment semantics in built-in containers. Use the same instance reference as the key to look up reliably.

## Representation

| Function / form     | Dunder        | Fallback                    |
|---------------------|---------------|-----------------------------|
| `repr(x)`           | `__repr__`    | `<ClassName instance>`      |
| `str(x)`, `print(x)`| `__str__`     | `__repr__`, then default    |
| `f"{x}"` (no spec)  | `__str__`     | same as `str(x)`            |
| `f"{x:spec}"`       | `__format__`  | built-in format spec engine |
| `f"{x!r}"`          | `__repr__`    | —                           |

`__format__(spec)` receives the format spec string and must return a `str`.

## Attribute access fallback

`__getattr__(self, name)` runs only when the normal lookup (instance dict -> class chain) misses. It receives the attribute name as a string and returns the value to use, or raises `AttributeError` to surface a real miss.

```python
class Proxy:
    def __getattr__(self, name):
        return f"computed:{name}"

p = Proxy()
print(p.anything)
print(p.foo)
```

```text Output
computed:anything
computed:foo
```

Existing attributes bypass `__getattr__`; only misses trigger it.

## Context managers

`with cm() as x:` invokes `__enter__` on entry; its return value binds to the `as` target. On exit, `__exit__(exc_type, exc_value, traceback)` runs — receiving `(None, None, None)` for normal exit, or the live exception info if the body raised. Returning truthy from `__exit__` suppresses the exception; falsy (or no return) propagates it.

```python
class Suppress:
    def __enter__(self):
        return self
    def __exit__(self, t, v, tb):
        return True # swallow whatever raised

with Suppress():
    raise ValueError("boom")
print("after")
```

```text Output
after
```

Multiple managers in one `with` (`with a(), b() as x:`) nest LIFO: `b` enters last and exits first. Each manager has its own implicit exception handler, so an inner suppression still lets outer managers run their normal `__exit__(None, None, None)`.

If `__exit__` itself raises a new exception, the new exception replaces the original.

## What's not dispatched

These dunders are parsed for syntactic compatibility but the VM doesn't invoke them on user classes:

- `__init_subclass__`, `__set_name__`, descriptor protocol (`__get__` / `__set__` / `__delete__`)
- `__new__` (instances are constructed by the VM; `__init__` runs the user logic)
- Augmented assignment dunders (`__iadd__`, `__imul__`, ...) — `a += b` desugars to `a = a + b`, so `__add__` covers it
- Async dunders (`__aenter__` / `__aexit__` / `__aiter__` / `__anext__`) — `async with` and `async for` use the sync `__enter__` / `__iter__` paths

For class basics (constructors, inheritance, properties), see [Classes](/language/classes).
