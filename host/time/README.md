# Edge Python time

Wall and monotonic clocks, sleep, and calendar formatting shipped as a plain ESM module. Scripts see `time` as ordinary, a subset of `time`.

```python
from time import time, sleep, strftime, gmtime

print(time()) # float seconds since the Unix epoch
sleep(0.1) # yields, resumes after 100ms
print(strftime("%Y-%m-%d %H:%M:%S", gmtime(0))) # 1970-01-01 00:00:00
```

## Setup

```html
<script type="module">
    import { createWorker } from "https://runtime.edgepython.com/js/src/index.js";
    import { time } from "./src/index.js";

    const worker = await createWorker({
        wasmUrl: "https://cdn.edgepython.com/compiler.wasm",
        mainThreadModules: { time },
    });
    await worker.run(await (await fetch("./script.py")).text());
</script>
```

Clock reads return immediately. `sleep` returns a Promise on the JS side and the runtime parks the coroutine until it resolves, the same deferred host-call path that `fetch` uses.

## Testing

Cases live in [`time.json`](time.json) and run through the shared runner at the repo root:

```bash
# One-time setup
deno run -A npm:playwright install chromium

# Run (from repo root)
HOSTCAP=time deno test --allow-all tests/
```

See [`tests/README.md`](../tests/README.md) for the corpus shape.

## API

**Conventions:**

- Clock reads are sync. They return numbers, no `await` and no `receive()`.
- `sleep(secs)` yields, so it composes with the concurrency builtins (`gather`, `with_timeout`, `run`) exactly like `fetch`.
- Nanosecond counts that exceed JS's safe-integer range cross as strings. `time_ns()` returns a numeric string (epoch nanoseconds overflow), parse it with `int()`. `monotonic_ns()` and `perf_counter_ns()` fit and return integers.
- A `struct_time` is a JSON nine-tuple string in CPython order: `[tm_year, tm_mon, tm_mday, tm_hour, tm_min, tm_sec, tm_wday, tm_yday, tm_isdst]`. `gmtime`, `localtime`, and `strptime` produce it. `strftime`, `asctime`, and `mktime` consume it directly. To read individual fields, decode it with the [`json`](https://github.com/dylan-sutton-chavez/edge-python/tree/main/std/json) package.
- `tm_wday` is Monday=0 through Sunday=6, `tm_yday` is 1 based, `tm_isdst` is always -1 (unknown).

### Clocks

```python
from time import time, time_ns, monotonic, monotonic_ns, perf_counter, perf_counter_ns

print(time()) # float seconds since the epoch, UTC wall clock
print(int(time_ns())) # nanoseconds since the epoch (crosses as a string)

start = perf_counter() # highest-resolution timer, for benchmarking a block
# ... work to measure ...
print(perf_counter() - start >= 0) # elapsed seconds
print(perf_counter_ns() >= 0) # same, as integer nanoseconds

print(monotonic() >= 0) # seconds, never steps backward
print(monotonic_ns() >= 0) # same, as integer nanoseconds
```

`time`, `time_ns`, `monotonic`, `monotonic_ns`, `perf_counter`, `perf_counter_ns`. In a browser `monotonic` and `perf_counter` share one source (`performance.now()`); reach for `perf_counter` when timing a block, `monotonic` for general elapsed-time checks.

### Sleep

```python
from time import sleep, monotonic

a = monotonic()
sleep(0.05)
print(monotonic() - a >= 0.05) # True
```

Because `sleep` yields, it composes with the scheduler:

```python
async def slow():
    sleep(10)
    return "never"

try:
    with_timeout(0.1, slow()) # deadline enforced by the scheduler
except TimeoutError:
    print("timed out")
```

### Calendar

```python
from time import gmtime, localtime, mktime
from "https://std.edgepython.com/json.wasm" import loads

t = loads(gmtime(0)) # decode the struct_time tuple
print(t[0], t[1], t[2]) # 1970 1 1
print(mktime(localtime(1000000)) == 1000000) # round-trips
```

`gmtime([secs])`, `localtime([secs])`, `mktime(t)`. Omitting `secs` uses the current time. `gmtime` is UTC, `localtime` is the host's local zone, `mktime` is the inverse of `localtime`.

### Formatting

```python
from time import strftime, strptime, asctime, ctime

print(strftime("%Y-%m-%d %H:%M:%S", gmtime(0))) # 1970-01-01 00:00:00
print(strftime("%a %b %d", gmtime(0))) # Thu Jan 01
print(asctime(gmtime(0))) # Thu Jan  1 00:00:00 1970
print(ctime(0)) # readable local time, e.g. Thu Jan  1 00:00:00 1970
print(strptime("2021-06-15", "%Y-%m-%d")) # struct_time tuple for that date
```

`strftime(fmt[, t])`, `strptime(s, fmt)`, `asctime([t])`, `ctime([secs])`. `strftime` and `asctime` default `t` to the current local time, `ctime` defaults `secs` to now.

Supported directives: `%Y %y %m %d %H %M %S %I %p %j %w %a %A %b %B %%`. Any literal that is not a directive passes through unchanged.

### Timezone

```python
from time import timezone, altzone, daylight, tzname

print(timezone()) # seconds west of UTC, standard (non-DST) offset
print(altzone()) # seconds west of UTC during DST
print(daylight()) # 1 if the zone observes DST, else 0
print(tzname()) # IANA zone name, e.g. "America/Bogota"
```

`timezone`, `altzone`, `daylight`, `tzname`. CPython exposes these as module constants, here they are zero-argument calls because host modules export callables, not values. `tzname` returns the IANA zone name rather than CPython's `(std, dst)` tuple.

## Not supported

By design, a sandboxed WASM runtime has no process, thread, or POSIX clock to expose:

- CPU and thread clocks: `process_time`, `process_time_ns`, `thread_time`, `thread_time_ns`.
- POSIX clock syscalls and ids: `clock_gettime`, `clock_settime`, `clock_getres`, their `_ns` variants, `pthread_getcpuclockid`, and the `CLOCK_*` constants.
- `tzset`, which re-reads the `TZ` environment variable. There is no environment, the zone comes from `Intl`.
- `struct_time` as a named tuple. Tuples cross as JSON strings, so `t[0]` after `json.loads` works but `t.tm_year` does not. A thin Python wrapper can add attribute access if you want it.

## How it works

`src/index.js` is a factory `() => handlers` (same shape as `dom`, `storage`). Two slices merge with `Object.assign`. `clock.js` returns the sync reads (`Date.now`, `performance.now`, `Intl`) plus the yielding `sleep`. `fmt.js` returns the pure calendar helpers, building struct_time tuples and formatting through `Date` without shipping any locale tables, on CPython's C locale names.

## License

MIT OR Apache-2.0
