# Edge Python re

Regular expressions shipped as a `.wasm` plugin. Scripts see `re` as ordinary.

```python
from re import search, sub, findall

m = search(r'(\d+)-(\d+)', 'order 12-34 shipped')
print(m) # 12-34

print(sub(r'\s+', '_', 'a  b   c')) # a_b_c
print(findall(r'\w+', 'one two three')) # ['one', 'two', 'three']
```

## Design

A small backtracking engine, around 1k lines of Rust, no automaton or bytecode. It is Unicode aware without shipping Unicode tables: `\d`, `\w`, `\s`, and `(?i)` evaluate through the standard library char predicates at match time, the same data the runtime already links. The match runs over codepoints, so every offset is a codepoint index that lines up with how the runtime indexes strings.

The tradeoff is the backtracking tradeoff: no linear time guarantee. Instead of hanging, a step budget proportional to the input length watches for degradation. When a pattern blows the budget the call raises `ValueError` naming catastrophic backtracking, so the author can rewrite it rather than ship a pattern that runs in O(n^2) time or worse.

## API

All functions take the pattern first. Flags are written inline, `(?i)` ignorecase, `(?s)` dotall, `(?m)` multiline.

| Function | Returns |
| --- | --- |
| `match(pattern, string)` | matched text anchored at the start, or `None` |
| `search(pattern, string)` | first matched text anywhere, or `None` |
| `fullmatch(pattern, string)` | matched text if it spans the whole string, or `None` |
| `findall(pattern, string)` | list of matches, or list of groups when the pattern has groups |
| `groups(pattern, string)` | capture groups of the first match as a list, or `None` |
| `span(pattern, string)` | `[start, end]` codepoint offsets of the first match, or `None` |
| `sub(pattern, repl, string)` | string with every match replaced, `\1` and `\g<name>` expand groups |

```python
from re import match, fullmatch, groups, span

print(match(r'\d+', '123abc')) # 123
print(match(r'\d+', 'abc123')) # None
print(fullmatch(r'[a-z]+', 'hello')) # hello
print(groups(r'(\w+)@(\w+)', 'a@b')) # ['a', 'b']
print(span(r'\d+', 'áé123')) # [2, 5], codepoint offsets
```

## Pattern syntax

Patterns are ordinary strings. Prefer raw strings (`r'...'`) so backslashes reach the engine intact.

**Characters**

| Token | Meaning |
| --- | --- |
| `.` | Any character, also newline under `(?s)` |
| `\d` `\w` `\s` | Digit, word character, whitespace, Unicode aware |
| `\D` `\W` `\S` | Negations of the three above |
| `[abc]` `[a-z]` | A character set or range |
| `[^abc]` | Any character not in the set |
| `\n` `\t` `\xhh` `\uhhhh` | Escapes for control and codepoint literals |

**Anchors**

| Token | Meaning |
| --- | --- |
| `^` `$` | Start and end, per line under `(?m)` |
| `\b` `\B` | Word boundary, and non boundary |

**Quantifiers**

| Token | Meaning |
| --- | --- |
| `*` `+` `?` | Zero or more, one or more, optional |
| `{m}` `{m,n}` `{m,}` | Exactly m, between m and n, at least m |
| trailing `?` | Lazy form, as few as possible, as in `*?` and `+?` |

**Groups and references**

| Token | Meaning |
| --- | --- |
| `(...)` | Capturing group |
| `(?:...)` | Group without capture |
| `(?P<name>...)` | Named capturing group |
| `\1` `(?P=name)` | Backreference to an earlier group |
| <code>a&#124;b</code> | Alternation, either side |

**Lookaround and flags**

| Token | Meaning |
| --- | --- |
| `(?=...)` `(?!...)` | Lookahead, positive and negative |
| `(?<=...)` `(?<!...)` | Lookbehind, fixed width only |
| `(?i)` `(?s)` `(?m)` | Inline flags, ignorecase, dotall, multiline |

## Not supported

Unicode property classes `\p{...}`, named character escapes `\N{...}`, atomic groups, possessive quantifiers, conditional patterns, scoped inline flags `(?i:...)`, variable width lookbehind. `\d` follows the Unicode numeric property, slightly wider than the CPython decimal set. Invalid patterns raise `ValueError`.

## When a pattern degrades

Backtracking has no linear time guarantee. A step budget proportional to the input watches the work, and when a pattern explodes the budget the engine raises instead of hanging:

```text
error: catastrophic backtracking, pattern needs O(n^2) time or worse on this input
  --> <input>:1:8
  |
1 | search(r'(a+)+$', user_input)
  |        ^^^^^^^^^
```

The nested `(a+)+` lets the engine split a run of `a` countless ways before failing at `$`. The flat form `a+$` matches the same text in linear time. The same budget also trips on a plain pattern over a very large non matching input, since a naive backtracker is O(n^2) there too, so the signal means rewrite the pattern or shrink the input.

This raises `RuntimeError`, not `ValueError`. The distinction is deliberate: a malformed pattern is a value problem, but a valid pattern that exhausts its budget is a runtime one, the same split engines like .NET draw with `RegexMatchTimeoutException`.

## How it works

The crate compiles to `wasm32-unknown-unknown` (`cdylib`) against the [wasm-pdk](https://github.com/dylan-sutton-chavez/edge-python/tree/main/wasm-pdk) `v0.1.0` ABI. `match` is a hand written export since it is a Rust keyword; the rest go through `#[plugin_fn]`. Results cross the handle ABI as primitives and lists. Plugin memory recycles per call through a static pool, like the `json` package.

## License

MIT OR Apache-2.0
