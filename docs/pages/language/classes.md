---
title: "Classes"
description: "User-defined classes as state machines and library namespaces."
---

Where classes are state containers and namespaces, not the primary abstraction (decided by design for the compiler purpose). Two patterns: state machines (a few methods that mutate the receiver) and namespaces (a bundle of related functions and constants).

Single-level inheritance with `super()`, `@property` / `@x.setter`, and a curated dunder protocol covering operators, indexing, iteration, hashing, context managers, and attribute fallback (see [Dunder methods](/language/dunders)). Multi-base C3 MRO, descriptors, metaclasses, `__slots__` are out of scope.

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

A class with no `__init__` and no per-instance state is a namespace. Methods called on the class are unbound — no `self` prepended.

```python
class Status:
  IDLE = 0
  RUNNING = 1
  DONE = 2

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

Single base via `class Sub(Base):`. Methods not on the subclass are looked up linearly on the base — no C3 MRO. `isinstance(x, Base)` walks the same chain, so `Sub` instances are also instances of every ancestor.

`super()` (zero-arg) delegates to the next class up the chain, bound to current `self`. Most common in `__init__` to extend a base constructor.

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

## Attribute access on classes vs instances

| Access form | Resolves to |
|---------------------|-------------------------------------------|
| `MyClass.attr` | class member, returned as-is (no binding) |
| `MyClass.method()` | method called directly, no `self` |
| `instance.attr` | instance `__dict__` first, then class |
| `instance.method()` | bound method, `self` prepended |

`setattr` / `delattr` work on instances. They do not modify the class object.

## Class decorators

A class decorator wraps the class object the same way it wraps a function — the decorator is called with the class; its return binds to the name.

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

`@property` turns a method into a read-only attribute. `@x.setter` (via `property.setter`) makes it writable. Properties live on the class; subclasses inherit and can override either side.

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

Two-arg form `property(fget, fset)` also works without decorator syntax.

## Operator overloading and protocols

Operators, indexing, iteration, context managers, hashing, `repr` / `str` / `format` all dispatch through dunders: `__add__` for `+`, `__eq__` for `==`, `__getitem__` for `x[i]`, `__iter__` / `__next__` for `for`, `__enter__` / `__exit__` for `with`, etc.

```python
class Vec:
  def __init__(self, x, y):
    self.x, self.y = x, y
  def __add__(self, other):
    return Vec(self.x + other.x, self.y + other.y)
```

See [Dunder methods](/language/dunders) for the full matrix.

## What is not supported

* Multi-base inheritance with proper C3 MRO. `class C(A, B):` parses and both bases are stored, but resolution is a linear depth-first walk, not C3. Prefer single inheritance.
* Metaclasses, descriptors (`__get__` / `__set__`), `__slots__`, ABCs, `__init_subclass__`.
* `@staticmethod` / `@classmethod` — use the namespace pattern or free functions.
* Async dunders; see [Dunders, What's not dispatched](/language/dunders#whats-not-dispatched).

Behaviour reuse via free functions and composition remains the default — fast dispatch, aligned with the functional-first identity. Reach for inheritance and operator overloading when the abstraction genuinely calls for them.
