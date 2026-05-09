---
title: "Classes"
description: "User-defined classes as state machines and library namespaces."
---

Edge Python's classes are deliberately small. They cover two patterns: **state machines** (a small number of methods that mutate the receiver) and **namespaces** (a bundle of related functions and constants). Inheritance, descriptors, properties, metaclasses, MRO, `super`, and dunder dispatch are not supported.

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

## What is *not* supported

- `class Sub(Super):` — no inheritance, no `super()`, no MRO. Reuse comes from composition (hold a field of another class) or free functions.
- `__eq__`, `__hash__`, `__repr__`, etc. — dunders are not dispatched. `==` on instances compares by identity.
- `@property`, `@staticmethod`, `@classmethod` — the namespace pattern above replaces `@staticmethod`. The other two have no equivalent.
- Slots, descriptors, metaclasses, abstract base classes.

The trade-off is intentional: the resulting object model fits in ~300 LOC of VM code and stays predictable when read top-to-bottom. Programs that need a richer object system are a poor fit for the target.
