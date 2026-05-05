use crate::s;
use crate::modules::fx::FxHashMap as HashMap;
use crate::modules::vm::types::ExternFn;

use alloc::{string::{String, ToString}, vec, vec::Vec};

pub(crate) const MAX_EXPR_DEPTH: usize = 200;
pub(crate) const MAX_INSTRUCTIONS: usize = 65_535;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OpCode {
    LoadConst, LoadName, StoreName, Call, PopTop, ReturnValue, BuildString, CallPrint, CallLen, 
    FormatValue, CallAbs, Minus, CallStr, CallInt, CallRange, Phi, CallChr, CallType, MakeFunction, 
    Add, Sub, Mul, Div, Eq, CallFloat, CallBool, CallRound, CallMin, CallMax, CallSum, CallSorted, 
    CallEnumerate, CallZip, CallList, CallTuple, CallDict, CallIsInstance, CallSet, CallInput, 
    CallOrd, BuildDict, BuildList, NotEq, Lt, Gt, LtEq, GtEq, And, Or, Not, JumpIfFalse, Jump, 
    GetIter, ForIter, GetItem, Mod, Pow, FloorDiv, LoadTrue, LoadFalse, LoadNone, LoadAttr, StoreAttr, 
    BuildSlice, MakeClass, SetupExcept, PopExcept, Raise, BitAnd, BitOr, BitXor,
    BitNot, Shl, Shr, In, NotIn, Is, IsNot, UnpackSequence, BuildTuple, SetupWith, ExitWith, Yield,
    Del, Assert, Global, Nonlocal, UnpackArgs, ListAppend, SetAdd, MapAdd, BuildSet, RaiseFrom,
    UnpackEx, LoadEllipsis, Await, MakeCoroutine, YieldFrom, TypeAlias, StoreItem, Dup2,
    JumpIfFalseOrPop, JumpIfTrueOrPop, Dup, CallMethod, CallMethodArgs, CallAll, CallAny, CallBin,
    CallOct, CallHex, CallDivmod, CallPow, CallRepr, CallReversed, CallCallable, CallId, CallHash,
    PopIter, DelItem, CallExtern,
    /* Push a heap-wrapped extern callable (`HeapObj::Extern`) onto the stack.
       Operand is the index into the chunk's extern_table. Used by `import X`
       (native) when building the module's attr table. */
    LoadExtern,
    /* Build a `HeapObj::Module` from the top of the stack and push it. The
       stack on entry has, top-down: module-name string, then `operand`
       (attr_name_str, attr_value) pairs. */
    BuildModule,
}

// Python builtin name → (specialised OpCode, leaves_value_on_stack).
pub(super) fn builtin(name: &str) -> Option<(OpCode, bool)> {
    match name {
        "len" => Some((OpCode::CallLen, true)),
        "abs" => Some((OpCode::CallAbs, true)),
        "str" => Some((OpCode::CallStr, true)),
        "int" => Some((OpCode::CallInt, true)),
        "type" => Some((OpCode::CallType, true)),
        "float" => Some((OpCode::CallFloat, true)),
        "bool" => Some((OpCode::CallBool, true)),
        "round" => Some((OpCode::CallRound, true)),
        "min" => Some((OpCode::CallMin, true)),
        "max" => Some((OpCode::CallMax, true)),
        "sum" => Some((OpCode::CallSum, true)),
        "sorted" => Some((OpCode::CallSorted, true)),
        "enumerate" => Some((OpCode::CallEnumerate, true)),
        "zip" => Some((OpCode::CallZip, true)),
        "list" => Some((OpCode::CallList, true)),
        "tuple" => Some((OpCode::CallTuple, true)),
        "dict" => Some((OpCode::CallDict, true)),
        "set" => Some((OpCode::CallSet, true)),
        "input" => Some((OpCode::CallInput, true)),
        "isinstance" => Some((OpCode::CallIsInstance, true)),
        "chr" => Some((OpCode::CallChr, true)),
        "ord" => Some((OpCode::CallOrd, true)),
        "all"      => Some((OpCode::CallAll, true)),
        "any"      => Some((OpCode::CallAny, true)),
        "bin"      => Some((OpCode::CallBin, true)),
        "oct"      => Some((OpCode::CallOct, true)),
        "hex"      => Some((OpCode::CallHex, true)),
        "divmod"   => Some((OpCode::CallDivmod, true)),
        "pow"      => Some((OpCode::CallPow, true)),
        "repr"     => Some((OpCode::CallRepr, true)),
        "reversed" => Some((OpCode::CallReversed, true)),
        "callable" => Some((OpCode::CallCallable, true)),
        "id"       => Some((OpCode::CallId, true)),
        "hash"     => Some((OpCode::CallHash, true)),
        _ => None,
    }
}

// Constant literals stored in the bytecode constants pool.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    BigInt(String),
    Float(f64),
    Bool(bool),
    None,
}

// One bytecode instruction: opcode + 16-bit operand.
#[derive(Debug, Clone, Copy)]
pub struct Instruction {
    pub opcode: OpCode,
    pub operand: u16,
}

// Compiled SSA chunk: instructions + pools + Phi metadata + nested
// functions/classes. One per module / function body / class body.
#[derive(Default, Clone)]
pub struct SSAChunk {
    pub instructions: Vec<Instruction>,
    pub constants: Vec<Value>,
    pub names: Vec<String>,
    pub functions: Vec<(Vec<String>, SSAChunk, u16, u16)>,
    pub annotations: HashMap<String, String>,
    pub phi_sources: Vec<(u16, u16)>,
    pub classes: Vec<SSAChunk>,
    pub is_pure: bool,
    pub is_generator: bool,
    pub overflow: bool,
    pub prev_slots: Vec<Option<u16>>,
    pub alias_groups: Vec<Vec<u16>>,
    pub phi_map: Vec<usize>,
    pub nonlocals: Vec<String>,
    pub(super) name_index: HashMap<String, u16>,
    /* Statement-level source map: (ip_at_stmt_entry, byte_offset) sorted by ip.
       Populated once per statement at parse time — granularity is enough for
       runtime diagnostics (the renderer needs only a byte offset; the source
       line and caret are derived from it). Lookup is binary search on the
       cold error path; hot dispatch never touches this. */
    pub stmt_pos: Vec<(u32, u32)>,
    /* External (native) functions resolved at parse time from `from <pkg> import <name>`.
       `extern_table[i]` is the function for `CallExtern` operand `i << 8`; the lower
       8 bits of the operand carry the argc. `extern_index` maps the local binding name
       to its slot so the parser's call site can dispatch to `CallExtern` instead of
       the generic `Call`. Per-chunk: each function body / class body has its own. */
    pub extern_table: Vec<ExternFn>,
    pub(super) extern_index: HashMap<String, u16>,
}

impl SSAChunk {
    /* Map a runtime ip back to a source byte offset via binary search on the
       per-statement table. Returns the offset of the enclosing statement —
       sub-statement precision isn't needed for runtime diagnostics. */
    pub fn resolve(&self, ip: u32) -> Option<u32> {
        let i = self.stmt_pos.partition_point(|&(s, _)| s <= ip).checked_sub(1)?;
        Some(self.stmt_pos[i].1)
    }

    pub(super) fn emit(&mut self, op: OpCode, operand: u16) {
        // Set overflow flag for post-parse diagnostic instead of panicking.
        if self.instructions.len() >= MAX_INSTRUCTIONS {
            self.overflow = true;
            return;
        }
        self.instructions.push(Instruction { opcode: op, operand });
    }

    pub(super) fn push_const(&mut self, v: Value) -> u16 {
        if self.constants.len() >= u16::MAX as usize {
            return 0;
        }
        self.constants.push(v);
        (self.constants.len() - 1) as u16
    }

    pub(super) fn push_name(&mut self, n: &str) -> u16 {
        if let Some(&i) = self.name_index.get(n) { return i; }
        if self.names.len() >= u16::MAX as usize {
            return 0;
        }
        let i = self.names.len() as u16;
        self.names.push(n.to_string());
        self.name_index.insert(n.to_string(), i);
        i
    }

    /* Build SSA prev_slots chain, coalesce versions to a canonical root,
       rewrite LoadName/StoreName/Del/Phi operands and Phi sources, and
       build phi_map. Recurses into nested functions and classes. */
    pub fn finalize_prev_slots(&mut self) {
        let n = self.names.len();

        // prev_slots[i] = name `i` with version-1, if any.
        let mut ps: Vec<Option<u16>> = vec![None; n];
        for (i, name) in self.names.iter().enumerate() {
            if let Some(pos) = name.rfind('_')
                && let Ok(ver) = name[pos+1..].parse::<u32>()
                && ver > 0 {
                    let prev = s!(str &name[..pos], "_", int ver - 1);
                    if let Some(&j) = self.name_index.get(&prev) {
                        ps[i] = Some(j);
                    }
            }
        }

        // Register coalescing: walk each chain to its root.
        let mut canonical: Vec<u16> = (0..n as u16).collect();
        for (i, item) in canonical.iter_mut().enumerate().take(n) {
            let mut root = i;
            while let Some(Some(p)) = ps.get(root) {
                let p = *p as usize;
                if p == root { break; }
                root = p;
            }
            *item = root as u16;
        }

        for ins in &mut self.instructions {
            match ins.opcode {
                OpCode::LoadName | OpCode::StoreName | OpCode::Del | OpCode::Phi => {
                    ins.operand = canonical[ins.operand as usize];
                }
                _ => {}
            }
        }
        for (a, b) in &mut self.phi_sources {
            *a = canonical[*a as usize];
            *b = canonical[*b as usize];
        }

        self.prev_slots = ps;
        self.alias_groups = (0..n).map(|i| vec![canonical[i]]).collect();

        for (_, body, _, _) in &mut self.functions {
            body.finalize_prev_slots();
        }
        for body in &mut self.classes {
            body.finalize_prev_slots();
        }

        let phi_count = self.instructions.iter().filter(|i| i.opcode == OpCode::Phi).count();
        if phi_count > 0 {
            self.phi_map = vec![0; self.instructions.len()];
            let mut phi_idx = 0;
            for (i, ins) in self.instructions.iter().enumerate() {
                if ins.opcode == OpCode::Phi {
                    self.phi_map[i] = phi_idx;
                    phi_idx += 1;
                }
            }
        }
    }
}

// SSA version snapshots taken before/after branches to insert Phi nodes
// at the join. `then` is None until mid_block runs.
pub(crate) struct JoinNode {
    pub(super) backup: HashMap<String, u32>,
    pub(super) then: Option<HashMap<String, u32>>,
}

/* Strip a trailing `_<digits>` SSA version suffix from a parser-emitted
   name. Names like `model_0`, `x_3` come from Parser::ssa_name and are an
   SSA-internal artifact — user-facing diagnostics must show the original
   `model`/`x`. Returns the input unchanged if no version suffix is present. */
pub fn ssa_strip(name: &str) -> &str {
    if let Some(pos) = name.rfind('_')
        && pos + 1 < name.len()
        && name[pos + 1..].bytes().all(|b| b.is_ascii_digit())
    {
        &name[..pos]
    } else {
        name
    }
}

/* Production-style diagnostic. `start`/`end` are byte offsets into the
   original source. Line/column are computed at render time so the parser
   never has to track them, and they're always char-accurate (UTF-8 safe). */
pub struct Diagnostic {
    pub start: usize,
    pub end: usize,
    pub msg: String,
}

/* Display width for a Unicode codepoint — 0 for combining marks / ZWJ /
   variation selectors, 2 for CJK & common wide emoji blocks, 1 otherwise.
   Approximation of Unicode Standard Annex #11 (East Asian Width); pulls in
   only the ranges that overwhelmingly account for misalignment in normal
   source code. Used so the caret in rustc-style diagnostics stays under the
   span when the line contains CJK or emoji. */
const fn char_width(c: char) -> usize {
    let cp = c as u32;
    if matches!(cp,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x05BF
        | 0x05C1..=0x05C2 | 0x05C4..=0x05C5 | 0x05C7 | 0x0610..=0x061A
        | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4
        | 0x06E7..=0x06E8 | 0x06EA..=0x06ED | 0x0711 | 0x0730..=0x074A
        | 0x07A6..=0x07B0 | 0x07EB..=0x07F3 | 0x200B..=0x200F | 0x202A..=0x202E
        | 0x2060..=0x206F | 0xFE00..=0xFE0F | 0xFEFF | 0xE0100..=0xE01EF)
    {
        0
    } else if matches!(cp,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF | 0x20000..=0x3FFFD)
    {
        2
    } else {
        1
    }
}

#[inline]
fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

impl Diagnostic {
    /* Convert a byte offset into (line, column), both 1-indexed.
       Column counts terminal display cells (CJK = 2, combining = 0) so the
       caret line aligns with the source line for users with wide characters. */
    fn line_col(src: &str, byte: usize) -> (usize, usize) {
        let byte = byte.min(src.len());
        let line = src[..byte].matches('\n').count() + 1;
        let line_start = src[..byte].rfind('\n').map_or(0, |p| p + 1);
        let col = display_width(&src[line_start..byte]) + 1;
        (line, col)
    }
}

impl Diagnostic {
    /* rustc-style multi-line render with source preview and caret:

         error: <msg>
            --> path:line:col
             |
           N | <source line>
             |     ^^^

       `path` is shown as `<input>` if None. The caret spans `start..end` (char-counted),
       always at least one column. */
    pub fn render(&self, src: &str, path: Option<&str>) -> alloc::string::String {
        let path = path.unwrap_or("<input>");
        let s_off = self.start.min(src.len());
        let e_off = self.end.min(src.len()).max(s_off);
        let (line_no, col) = Self::line_col(src, s_off);
        let line_start = src[..s_off].rfind('\n').map_or(0, |p| p + 1);
        let line_end = src[s_off..].find('\n').map_or(src.len(), |p| s_off + p);
        let line_txt = &src[line_start..line_end];
        let mark = display_width(&src[s_off..e_off]).max(1);
        let mut buf = itoa::Buffer::new();
        let pad_len = buf.format(line_no).len();
        let pad: String = " ".repeat(pad_len);
        let mut o = alloc::string::String::with_capacity(line_txt.len() + 96);
        o.push_str("error: "); o.push_str(&self.msg); o.push('\n');
        o.push_str(&pad); o.push_str(" --> ");
        o.push_str(path); o.push(':');
        o.push_str(buf.format(line_no)); o.push(':'); o.push_str(buf.format(col)); o.push('\n');
        o.push_str(&pad); o.push_str(" |\n");
        o.push_str(buf.format(line_no)); o.push_str(" | "); o.push_str(line_txt); o.push('\n');
        o.push_str(&pad); o.push_str(" | ");
        for _ in 1..col { o.push(' '); }
        for _ in 0..mark { o.push('^'); }
        o.push('\n');
        o
    }
}

impl Diagnostic {
    /* Compact one-line render: `path:line:col: msg`. No source preview. Used by tests. */
    pub fn render_oneline(&self, src: &str, path: Option<&str>) -> alloc::string::String {
        use crate::s;
        let (line, col) = Self::line_col(src, self.start);
        match path {
            Some(p) => s!(str p, ":", int line, ":", int col, ": ", str &self.msg),
            None => s!("line ", int line, ":", int col, ": ", str &self.msg),
        }
    }
}

// Strip prefix + quotes and unescape (skipped for raw strings).
pub(super) fn parse_string(s: &str) -> String {
    let is_raw = s.contains('r') || s.contains('R');
    let s = s.trim_start_matches(|c: char| "bBrRuU".contains(c));
    let inner = if s.starts_with("\"\"\"") || s.starts_with("'''") {
        &s[3..s.len() - 3]
    } else {
        &s[1..s.len() - 1]
    };
    if is_raw { inner.to_string() } else { unescape(inner) }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    let take_hex = |chars: &mut core::iter::Peekable<core::str::Chars>, n: usize| -> char {
        let hex: String = chars.by_ref().take(n).collect();
        u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).unwrap_or('\u{FFFD}')
    };

    while let Some(c) = chars.next() {
        if c != '\\' { out.push(c); continue; }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some('\'') => out.push('\''),
            Some('"') => out.push('"'),
            Some('x') => out.push(take_hex(&mut chars, 2)),
            Some('u') => out.push(take_hex(&mut chars, 4)),
            Some('U') => out.push(take_hex(&mut chars, 8)),
            Some('0') => out.push('\0'),
            Some(c) => { out.push('\\'); out.push(c); }
            None => out.push('\\'),
        }
    }
    out
}

// Built-in types pre-registered as `Type` heap objects in the global
// scope at VM init.
pub const BUILTIN_TYPES: &[&str] = &[
    "int", "float", "str", "bool", "list",
    "tuple", "dict", "set", "range", "type", "NoneType",
    "Exception", "BaseException",
    "ValueError", "TypeError", "NameError", "KeyError",
    "IndexError", "AttributeError", "RuntimeError",
    "ZeroDivisionError", "OverflowError", "MemoryError",
    "RecursionError", "StopIteration", "NotImplementedError",
    "OSError", "IOError", "ImportError", "ModuleNotFoundError",
    "AssertionError", "ArithmeticError", "LookupError",
];

