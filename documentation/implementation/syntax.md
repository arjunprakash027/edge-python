---
title: "Syntax"
description: "Single-pass parser, SSA emission, and bytecode shape."
---

## Overview

The parser is single-pass. It consumes the lexer token stream and emits bytecode directly into an `SSAChunk`, with no intermediate AST. Each grammatical construct is parsed and lowered in one traversal. The parser is also responsible for SSA versioning, phi-node insertion at control-flow joins, and structural diagnostics. Lex-time errors gathered by the scanner are merged into the parser's diagnostic stream so the user sees a single ordered report.

The Pratt scheme governs expression parsing: each operator has a left and right binding power, and `expr_bp(min_bp)` recursively pulls in everything bound at least as tightly as `min_bp`.

## Bytecode model

Each instruction is a tagged 4-byte record:

```rust
pub struct Instruction {
    pub opcode: OpCode, // 1 byte (with #[repr(u8)] planned)
    pub operand: u16, // 2 bytes
}
```

The operand is a 16-bit slot — its meaning depends on the opcode. Common shapes:

| OpCode               | Operand interpretation                              |
|----------------------|------------------------------------------------------|
| `LoadConst`          | constant pool index                                  |
| `LoadName` / `StoreName` | name slot index                                  |
| `Add`, `Sub`, ...    | unused (IC keyed by ip)                              |
| `Call`               | `(num_kw << 8) \| num_pos`                           |
| `BuildList` / `BuildTuple` / `BuildSet` | element count                     |
| `BuildDict`          | key-value pair count                                 |
| `BuildSlice`         | parts count (2 or 3)                                 |
| `Jump` / `JumpIfFalse` | target instruction index                           |
| `ForIter`            | jump target on iterator exhaustion                   |
| `Phi`                | target slot; sources stored in `chunk.phi_sources`   |
| `UnpackEx`           | `(before << 8) \| after`                             |
| `MakeFunction`       | function index in `chunk.functions`                  |

Operands are bounded to `u16::MAX` (65,535). The same cap applies to the size of the constant pool, name table, and instruction stream per chunk.

## Expression parsing

`expr_bp(min_bp)` runs the Pratt loop. The atom dispatcher in `parse_atom` advances one token and routes by kind:

```text
Name        -> name() (handles assignment, walrus, calls)
String      -> emit Str constant; concatenate adjacent String tokens
Int / Float -> emit numeric constant; literal beyond 2⁴⁷ is a parse error
True/False/None/Ellipsis -> emit dedicated load opcode
FstringStart -> fstring()
Lbrace      -> brace_literal()  (dict, set, comprehension)
Lsqb        -> list_literal()   (list, comprehension)
Lpar        -> grouped expr, tuple, generator, or empty tuple
Lambda      -> parse_lambda()
```

After an atom, `postfix_tail()` handles trailers — subscript, attribute access, and call — which iterate until none apply. This is what lets expressions like `fns[0](-3)`, `obj.method()`, `(lambda x: x)(3)`, and `compose(f, g)(x)` parse uniformly.

`*args` / `**kwargs` are accepted in **call** position only; the parser does **not** support starred unpacking inside literals (`[*a, *b]`, `{**d1, **d2}`, `(1, *xs, 2)`).

## Operator precedence

Every binary operator declares a `(l_bp, r_bp, OpCode)` triple in `binding_power`. Higher binding pulls more tightly. Right-associative operators (only `**` in Edge Python) have `r_bp < l_bp`; everything else is left-associative.

| Level | Operators                                | Notes                |
|-------|------------------------------------------|----------------------|
| 1/2   | `or`                                     | short-circuit        |
| 3/4   | `and`                                    | short-circuit        |
| 5     | unary `not`                              | prefix only          |
| 7/8   | `==` `!=` `<` `>` `<=` `>=` `in` `not in` `is` `is not` | chainable |
| 9/10  | `\|`                                     | bitwise              |
| 11/12 | `^`                                      | bitwise              |
| 13/14 | `&`                                      | bitwise              |
| 15/16 | `<<` `>>`                                | shifts               |
| 17/18 | `+` `-`                                  | additive             |
| 19/20 | `*` `/` `%` `//`                         | multiplicative       |
| 21    | unary `-` `~` `await`                    | prefix               |
| 22/21 | `**`                                     | right-associative    |

Comparison chaining (`a < b < c`) is handled inline by `infix_bp`: when a comparison opcode is followed by another comparison token, the parser stores the middle value in a synthetic `__cmp__N` slot, emits the first comparison, short-circuits on false, and reuses the stored value for the next comparison.

## Short-circuit lowering

`and` and `or` lower to `JumpIfFalseOrPop` / `JumpIfTrueOrPop` — superinstructions that peek the stack top, pop only if execution continues, and otherwise jump while leaving the value on the stack:

```text
a and b

LoadName a
JumpIfFalseOrPop  ->  end
LoadName b
end:
```

This means `and` / `or` correctly preserve operand identity (returning the actual value, not a coerced bool) without an extra opcode.

## SSA versioning

Every binding emits a fresh slot with an incremented version counter. The parser maintains a `HashMap<String, u32>` mapping each base name to its current version. Names in the chunk's `names` table are stored as `name_version`:

```python
x = 1 # x_1
x = 2 # x_2
y = x # y_1, references x_2
```

```text
chunk.names = ["x_1", "x_2", "y_1"]
chunk.instructions:
   LoadConst 0    (1)
   StoreName 0    (x_1)
   LoadConst 1    (2)
   StoreName 1    (x_2)
   LoadName  1    (x_2)
   StoreName 2    (y_1)
```

Lookups on undefined names target version 0 (`x_0`), which is filled either by the host before execution (the VM seeds globals like `print_0`) or — if still unbound at load time — raises `NameError`.

## Phi nodes at joins

At each control-flow boundary the parser pushes a `JoinNode { backup, then }` onto a stack:

```text
enter_block() -> snapshot current versions into JoinNode.backup (and reset to the same baseline for the if branch).
mid_block() -> snapshot post-then versions into JoinNode.then; restore baseline (max of backup, then-state) for else.
commit_block() -> diff (then ∪ post) against (backup), emit Phi for each name that diverged.
```

Each emitted `Phi` carries the *target* slot (the new version after the join) in its operand. The two source slots are stored separately in `chunk.phi_sources` and indexed by `chunk.phi_map[ip]` at runtime. This keeps `Instruction` at 4 bytes while supporting binary phis.

```python
if cond:
    x = 1
else:
    x = 2
print(x)
```

```text
LoadName cond_0
JumpIfFalse else_label
LoadConst 0 *(1)
StoreName x_1
Jump end_label
else_label:
LoadConst 1 *(2)
StoreName x_2
end_label:
Phi x_3 *(sources: x_1, x_2)
LoadName x_3
CallPrint 1
```

The runtime resolves `Phi` by reading whichever of the two source slots is `Some` — at the join, exactly one branch executed.

## Statement dispatch

`stmt()` peeks the leading token and routes:

```text
if          -> if_stmt          (with elif chain, optional else)
for         -> for_stmt_inner   (sync iter, optional else)
while       -> while_stmt       (with break/continue patches)
match       -> match_stmt
def         -> func_def_inner
class       -> class_def        (__init__, attributes, methods)
with        -> with_stmt_inner  (multi-target, async variant)
try         -> try_stmt         (except, else, finally, raise)
import      -> import_stmt      (compile-time resolver lookup)
from        -> parse_from_stmt  (named / star imports, same path)
type        -> type-alias declaration
yield       -> yield expr / yield from
async       -> async def / for / with
@           -> decorator stack + def or class (peeks `class` after the @-list)
return      -> expr + ReturnValue
raise       -> expr + Raise / RaiseFrom
break       -> patched at loop end
continue    -> jump to current loop_start
del / global / nonlocal / assert / pass -> direct emit
Name        -> name_stmt (assignment, augmented, indexed, attribute, call)
```

Each statement returns a bool indicating whether it left a value on the stack. The driver loop emits `PopTop` after expression-shaped statements (`x.method()`, `1 + 2` at module level) but not after statement-shaped ones (assignment, control flow).

Decorators apply to both `def` and `class` — the `@` arm peeks for `class` after collecting the decorator stack and routes accordingly. Each decorator wraps the produced value via a `Call,1` instruction emitted between `MakeFunction`/`MakeClass` and the final `StoreName`.

`raise X from Y` lowers to `RaiseFrom`, which pops the cause first and then the exception so `X` is the value that surfaces. Bare `raise` re-raises the current exception. `__cause__` / `__context__` chaining is not exposed — only the final raised type and args are visible. `except` matching walks an `EXC_PARENTS` table so `except Exception` catches subclasses (`RuntimeError`, `ValueError`, ...) the same way `isinstance(e, Exception)` does.

## Lambda and function bodies

Lambdas and `def` both compile their body into a *fresh* SSAChunk:

```rust
self.with_fresh_chunk(|s| {
    s.ssa_versions = outer_versions.clone();
    for p in &params { s.ssa_versions.insert(p.clone(), 0); }
    s.expr();                        // or compile_block_body for def
    s.chunk.emit(OpCode::ReturnValue, 0);
});
```

Free variables in the body — names that aren't parameters and don't have a local binding — are looked up in the outer chunk's name table. The `MakeFunction` opcode at runtime captures matching slots from the enclosing scope into the function's `captures` list (snapshotted at `MakeFunction` time — there are no cell objects). Nested `def` and `lambda` push their own free names back into their parent's name table, so each enclosing function captures whatever its descendants need; the chain propagates through any depth (e.g. `def A → def B → def C` where `C` references a var in `A`).

Parameters classify into three slot kinds: `Normal`, `Star` (`*args`), `DoubleStar` (`**kwargs`). The keyword-only marker `*` (lone `*` separator) prefixes following parameter names so they are matched by keyword only and never receive positional arguments. Defaults are stored in `HeapObj::Func.defaults` and applied to the last-N positional slots. Parameter annotations (`x: T`) and return annotations (`-> T`) are parsed and drained without affecting runtime; they are recorded in `chunk.annotations` for tooling use only.

After body compilation, `compile_body` inspects the body's instruction stream for opcodes that imply impurity (`StoreItem`, `StoreAttr`, `CallPrint`, `CallInput`, `Global`, `Nonlocal`, `Import`, `Raise`, `Yield`, `LoadAttr`) and sets `body.is_pure` accordingly. The runtime template-memoisation layer uses this flag — pure functions get their `(args) -> result` mapping cached after `TPL_THRESH = 2` hits, capped at 256 entries.

## Type annotations

Annotations are parsed for compatibility with CPython source but discarded for execution:

```python
counter: int = 0       # annotation 'int' parsed and stored, slot still gets 0
def f(x: int) -> int:  # annotations on params and return parsed and skipped
    return x
```

Annotations are recorded in `chunk.annotations: HashMap<String, String>` for diagnostic and tooling use, but no code is emitted for them; `f.__annotations__` is **not** exposed at runtime.

## Comprehensions and generators

List, set, and dict comprehensions are supported with multi-`for` and multi-`if` filters. They lower to `BuildList` / `BuildSet` / `BuildDict` plus an explicit loop scaffold that calls `ListAppend` / `SetAdd` / `MapAdd`.

Generator expressions `(i*2 for i in xs)` are **eagerly lowered to `BuildList`** — the parenthesised form is operationally equivalent to `[i*2 for i in xs]`. This is a deliberate trade-off: the template-memoisation layer requires hashable, finite arguments, and lazy generators wouldn't memoise. For unbounded streams, write a `def` with `yield` (which produces a real `HeapObj::Coroutine`).

Async comprehensions (`[x async for x in y]`) and starred-unpack patterns inside comprehensions are not supported.

## F-string lowering

```python
f"hello {name}, age {age}"
```

Lowers to:

```text
LoadConst "hello "
LoadName  name_v
FormatValue 0
LoadConst ", age "
LoadName  age_v
FormatValue 0
BuildString 5
```

`FormatValue`'s 16-bit operand is a small flags field:
- bit 0 — set when a format spec string is on the stack just below the value (collected as the raw text between `:` and `}` and emitted as a constant).
- bits 1–2 — conversion: `0` none, `1` `!r`, `2` `!s`, `3` `!a`.

The VM applies the conversion first (if any), then runs the format-spec mini-language `[[fill]align][sign][#][0][width][,][.precision][type]` with type chars `s d b o x X f F e E g G n % c`. `n` is aliased to `d` (no locale support). The `=` self-documenting form (`{expr=}`) is supported and emits a literal `expr=` prefix. Adjacent string literals (including bytes) concatenate at parse time. Spec parsing failures surface as a `ValueError` at runtime.

## Limits

| Constant             | Value     | Purpose                                |
|----------------------|-----------|----------------------------------------|
| `MAX_EXPR_DEPTH`     | 200       | Cap on recursive expression parsing    |
| `MAX_INSTRUCTIONS`   | 65,535    | Cap on instructions per chunk          |

Hitting `MAX_EXPR_DEPTH` raises a parser diagnostic ("expression too deeply nested"). Hitting `MAX_INSTRUCTIONS` sets `chunk.overflow = true`, which is reported as a diagnostic at the end of parsing — the chunk's instruction stream is cleared rather than dispatched.

## References

- Pratt. *Top Down Operator Precedence* (POPL 1973). Precedence climbing.
- Cytron et al. *Efficiently Computing Static Single Assignment Form* (TOPLAS 1991). SSA, phi-nodes.
- Crafting Interpreters, by Robert Nystrom: craftinginterpreters.com — single-pass codegen patterns.
- Casey et al. *Towards Superinstructions for Java Interpreters* (SCOPES 2003). LoadAttr+Call fusion.