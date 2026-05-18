---
title: "Async"
description: "Cooperative coroutines: run, sleep, frame, gather, with_timeout, cancel, receive."
---

Edge Python supports cooperative concurrency via `async def` coroutines and the `await` / `yield` keywords. There is **no preemption**: a coroutine runs until it explicitly yields, sleeps, awaits, or returns. The scheduler runs on a single OS thread; concurrency is by interleaving, not parallelism.

There is **no `asyncio` module**. The async primitives; `run`, `sleep`, `frame`, `gather`, `with_timeout`, `cancel`, `receive`, are top-level builtins. The scheduler is direct enough that wrapping it in a module-shaped namespace would add no semantic value.

```python
import asyncio   # ModuleNotFoundError — there is no asyncio
```

```python
# Idiomatic edge-python: call the primitives directly.
async def main():
    sleep(0.01)
    return "ok"

print(run(main()))
```

```text Output
ok
```

## Two kinds of callables

| Construct | Runs as | Cancellable | Real time |
|---|---|---|---|
| `def` (a routine) | synchronous, in-place | no | no |
| `async def` (a coroutine) | suspended object until passed to `run`/`gather` | yes (via `cancel`) | yes (via `sleep`) |

A `def` body executes immediately when called. An `async def` body returns a coroutine value that does nothing until you drive it.

```python
def routine():
    return 1

async def coro():
    return 1

print(routine())   # 1
print(coro())   # <coroutine>  (does not run yet)
print(run(coro()))   # 1  (run drives it to completion)
```

## Driving coroutines

`run(coro)` executes a single coroutine to completion and returns its value.

```python
async def square(n):
    return n * n

print(run(square(5)))
```

```text Output
25
```

`run(c1, c2, ...)` accepts multiple coroutines. They run concurrently; the call returns the first argument's result.

## Sleeping

`sleep(seconds)` suspends the current coroutine until `seconds` of wall-clock time pass. With no host time hook installed, a virtual clock advances logically — coroutines still interleave deterministically, but no real wait happens (useful for tests).

```python
async def task(name):
    print(f"{name} step 1")
    sleep(0) # yield to the scheduler
    print(f"{name} step 2")

run(task("a"), task("b"))
```

```text Output
a step 1
b step 1
a step 2
b step 2
```

## gather

`gather(*coros)` runs every argument concurrently and returns a list of their results in argument order. If any coroutine raises, the others are cancelled and the first error propagates.

```python
async def fetch(name, delay):
    sleep(delay)
    return name + "!"

print(gather(fetch("a", 0.05), fetch("b", 0.02), fetch("c", 0.03)))
```

```text Output
['a!', 'b!', 'c!']
```

The total wall time is `max(delays)`, not the sum — `b` and `c` overlap with `a`'s sleep.

### Errors

```python
async def good(): return 1
async def bad():  raise ValueError

try:
    gather(good(), bad())
except ValueError:
    print("caught")
```

```text Output
caught
```

## with_timeout

`with_timeout(seconds, coro)` runs `coro` to completion or raises `TimeoutError` if it doesn't finish in time. The coroutine is cancelled on timeout.

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

`with_timeout` evaluates the coroutine eagerly — it's a function call, not an awaitable.

## cancel

`cancel(coro)` flags a coroutine registered with the scheduler for cancellation. On its next scheduler tick the coroutine transitions to `Cancelled` and stops running. The coroutine's body does **not** observe a `CancelledError` raised inside it — cancellation is cooperative and silent.

A coroutine in a tight synchronous loop without any `await`/`sleep` cannot be cancelled until it yields:

```python
async def loop_forever():
    for i in range(1_000_000):
        pass # no yield — not cancellable here
    sleep(0) # cancellable from this point on
```

For deadline-driven cancellation use `with_timeout`. For peer-cancellation on error use `gather` (it cancels surviving peers automatically).

## Exception types

The async primitives can raise:

| Exception | When |
|---|---|
| `TimeoutError` | `with_timeout` deadline expired |
| `CancelledError` | reserved for user-thrown cancellation; not auto-raised by `cancel()` |

Both are in the built-in exception namespace and match `except` clauses normally.

## Limitations

* **No preemption.** A `while True: pass` inside a coroutine blocks the scheduler.
* **Cancellation is silent.** `cancel(coro)` stops the coroutine; the body does not see `CancelledError`. Use `with_timeout` if you need deadline semantics that propagate as exceptions.
* **Host event loop is cooperative.** When the scheduler cannot make synchronous progress it yields a `SchedulerStatus` through `VmErr::HostYield` — `PendingTimer(deadline_ns)`, `PendingFrame`, or `PendingEvent`. Embedders drive the cycle via the `run_start` / `run_resume` / `run_push_event` exports; the bundled JS runtime (`runtime/src/engine.js`) handles it automatically with `setTimeout`, `requestAnimationFrame`, and a Promise resolved by `pushEvent`, and also auto-invokes a `main` global if the script defines one — scripts never call `run(main())` directly. The legacy `run` entry cannot resume — code that needs `sleep(n>0)`, `frame()`, or an empty `receive()` must use the driver loop. Resume re-enters the scheduler, not the dispatch loop, so statements after a top-level `run()` do not execute after a yield.
* **`async for`** works against any iterable accepted by `for`, *plus* coroutines and async generators (functions defined with both `async def` and `yield`). Each iteration resumes the coroutine to its next yield. There is no `__aiter__` / `__anext__` dispatch on user-defined classes — define an `async def` generator instead. Behavior over plain lists/tuples/dicts is identical to a regular `for`.
* **`async with`** reuses the sync `with` dispatch path, invoking `__enter__` / `__exit__` on the context manager. `__aenter__` / `__aexit__` are not consulted (the async dunder forms are not dispatched). For async setup/teardown that needs `await`, use `try` / `finally` with explicit `await` calls.
* **No async comprehensions.** `[x async for x in it]` is not supported.
* **No `gen.send` / `.throw` / `.close`.** Generators and coroutines are one-way producers. For bidirectional flow, structure the work with `run` / `gather` and pass messages through call arguments.
* **`receive()` blocks indefinitely.** With an empty queue and no host `run_push_event` arriving, the coroutine stays parked in `WaitingEvent`. Pair with `with_timeout` if you need a deadline.

## Time capability

The scheduler reads time from `vm.time_hook`. WASM hosts wire it to `Date.now() * 1e6` via the `host_now_ns` import; native hosts can use `std::time::Instant`. Without a hook installed, `sleep` advances a virtual clock so deterministic tests still interleave coroutines correctly.
