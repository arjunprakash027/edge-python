---
title: "Classes"
description: "User-defined classes as state machines and library namespaces."
---

Edge Python is **functional-first**: classes are state containers and library namespaces, not the primary abstraction. They cover two patterns: **state machines** (a small number of methods that mutate the receiver) and **namespaces** (a bundle of related functions and constants).

Classes support single-level inheritance with `super()`, `@property` / `@x.setter` for managed attributes, and the full dunder protocol for operators, indexing, iteration, context managers, and the rest (see [Dunder methods](/language/dunders) for the matrix). Multi-base inheritance with C3 MRO, descriptors, metaclasses, and `__slots__` remain out of scope.

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

## Inheritance and super()

A class can declare a single base with `class Sub(Base):`. Methods not defined on the subclass are looked up on the base, and the lookup walks the chain linearly — there is no C3 MRO. `isinstance(x, Base)` walks the same chain, so an instance of `Sub` is also an instance of every ancestor.

`super()` in zero-argument form delegates to the next class up the chain, bound to the current `self`. It is most commonly used in `__init__` to extend a base constructor.

```python
class Animal:
    def __init__(self, name):
        self.name = name
    def describe(self):
        return self.name

class Dog(Animal):
    def __init__(self, name, breed):
        super().__init__(name)
        self.breed = breed
    def describe(self):
        return super().describe() + " (" + self.breed + ")"

d = Dog("Rex", "lab")
print(d.describe())
print(isinstance(d, Animal))
```

```text Output
Rex (lab)
True
```

A multi-base declaration `class C(A, B):` is parsed and both bases are stored, but resolution is a simple left-to-right depth-first walk, not Python's C3 MRO. Prefer single inheritance.

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

## Properties

`@property` turns a method into a read-only attribute. `@x.setter` (built via the `property.setter` factory on the existing property) makes the same attribute writable. Properties are looked up on the class, so subclasses inherit them and may override either side.

```python
class Temp:
    def __init__(self, c):
        self._c = c
    @property
    def celsius(self):
        return self._c
    @celsius.setter
    def celsius(self, value):
        self._c = value
    @property
    def fahrenheit(self):
        return self._c * 9 / 5 + 32

t = Temp(20)
print(t.celsius)
print(t.fahrenheit)
t.celsius = 100
print(t.fahrenheit)
```

```text Output
20
68.0
212.0
```

The two-argument form `property(fget, fset)` is also available for building properties without decorator syntax.

## Operator overloading and protocols

Operators, indexing, iteration, context managers, hashing, and `repr` / `str` / `format` all dispatch through dunder methods defined on the class. Define `__add__` for `+`, `__eq__` for `==`, `__getitem__` for `x[i]`, `__iter__` / `__next__` for `for`, `__enter__` / `__exit__` for `with`, and so on.

```python
class Vec:
    def __init__(self, x, y):
        self.x, self.y = x, y
    def __add__(self, other):
        return Vec(self.x + other.x, self.y + other.y)
```

See [Dunder methods](/language/dunders) for the full matrix.

## What is *not* supported

- Multi-base inheritance with proper C3 MRO. `class C(A, B):` is parsed and both bases are stored, but resolution is a linear depth-first walk, not Python's C3 algorithm. Prefer single inheritance.
- Metaclasses, descriptors (`__get__` / `__set__`), `__slots__`, abstract base classes, `__init_subclass__`.
- `@staticmethod` and `@classmethod`. Use the namespace pattern above or free functions instead.
- Async dunders: `__aenter__` / `__aexit__` / `__aiter__` / `__anext__`. `async with` and `async for` do not dispatch these hooks.

Behaviour reuse via free functions and composition is still the preferred default — it keeps dispatch fast and aligns with the functional-first identity. Reach for operator overloading and inheritance when the abstraction genuinely calls for them.
