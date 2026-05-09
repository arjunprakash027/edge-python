---
title: "Classes"
description: "User-defined classes as state machines and library namespaces."
---

Edge Python is **functional-first**: classes are state containers and library namespaces, not the primary abstraction. They cover two patterns: **state machines** (a small number of methods that mutate the receiver) and **namespaces** (a bundle of related functions and constants).

By design, the class system omits inheritance chains, `super()`, MRO walking, descriptor protocols, properties, metaclasses, slots, and dunder dispatch. The only dunder the VM consults on user instances is `__init__`. This keeps the object model in ~300 LOC of VM code; programs that need a richer object system are a poor fit for the target.

## State-machine pattern

```python
class Counter:
    def __init__(self, n=0):
        self.n = n
    def tick(self):
        self.n += 1
    def value(self):
        return self.n

c = Counter()
c.tick()
c.tick()
c.tick()
print(c.value())
```

```text Output
3
```

## Namespace pattern

A class with no `__init__` and no per-instance state is a namespace of functions and constants. Methods called on the class object are unbound — no `self` is prepended.

```python
class Status:
    IDLE    = 0
    RUNNING = 1
    DONE    = 2

class Math:
    PI = 3.14159
    def square(x):
        return x * x
    def cube(x):
        return x * x * x

print(Status.IDLE)
print(Math.PI)
print(Math.square(5))
print(Math.cube(3))
```

```text Output
0
3.14159
25
27
```

## Attribute access on classes vs instances

| Access form         | Resolves to                              |
|---------------------|------------------------------------------|
| `MyClass.attr`      | class member, returned as-is (no binding)|
| `MyClass.method()`  | method called directly, no `self`        |
| `instance.attr`     | instance `__dict__` first, then class    |
| `instance.method()` | bound method, `self` prepended           |

`setattr` / `delattr` work on instances. They do not modify the class object.

## Class decorators

A decorator on a class wraps the class object the same way it wraps a function: the decorator is called with the class and its return value is bound to the name.

```python
def tag(cls):
    cls.kind = "tagged"
    return cls

@tag
class Box:
    def __init__(self, v):
        self.v = v

print(Box.kind)
print(Box(7).v)
```

```text Output
tagged
7
```

## What is *not* supported

- `class Sub(Super):` — parsed but the base list has no MRO; methods are not inherited from a base class. There is no `super()`, no method resolution order, and no inheritance chain. Reuse comes from composition (hold a field of another class) or free functions.
- `__eq__`, `__hash__`, `__repr__`, `__add__`, `__getitem__`, `__iter__`, `__len__`, `__call__`, `__bool__`, ... — none of these dunders are dispatched. Operators and built-ins resolve on the type tag, not on user-class methods. `==` on instances compares by identity.
- `__enter__` / `__exit__` and `__aenter__` / `__aexit__` — `with` and `async with` are stack-save scopes; the runtime does **not** invoke entry or exit hooks. Use `try` / `finally` for resource cleanup.
- `@property`, `@staticmethod`, `@classmethod` — the namespace pattern above replaces `@staticmethod`. The other two have no equivalent.
- Slots, descriptors, metaclasses, abstract base classes, `__slots__`, `__init_subclass__`.

When you need behaviour reuse, write a free function that takes the value rather than a method on the class. That keeps dispatch fast (one ALU instruction per op rather than a dunder lookup) and aligns with the functional-first identity.
