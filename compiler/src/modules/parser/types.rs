use crate::s;
use crate::modules::fx::FxHashMap as HashMap;

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
    BuildSlice, MakeClass, SetupExcept, PopExcept, Raise, Import, ImportFrom, BitAnd, BitOr, BitXor, 
    BitNot, Shl, Shr, In, NotIn, Is, IsNot, UnpackSequence, BuildTuple, SetupWith, ExitWith, Yield, 
    Del, Assert, Global, Nonlocal, UnpackArgs, ListAppend, SetAdd, MapAdd, BuildSet, RaiseFrom, 
    UnpackEx, LoadEllipsis, Await, MakeCoroutine, YieldFrom, TypeAlias, StoreItem, Dup2, 
    JumpIfFalseOrPop, JumpIfTrueOrPop, Dup, CallMethod, CallMethodArgs, CallAll, CallAny, CallBin,
    CallOct, CallHex, CallDivmod, CallPow, CallRepr, CallReversed, CallCallable, CallId, CallHash,
    PopIter, DelItem,
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
}

impl SSAChunk {
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

/* Production-style diagnostic. `start`/`end` are byte offsets into the
   original source. Line/column are computed at render time so the parser
   never has to track them, and they're always char-accurate (UTF-8 safe). */
pub struct Diagnostic {
    pub start: usize,
    pub end: usize,
    pub msg: String,
}

impl Diagnostic {
    /* Convert a byte offset into (line, column), both 1-indexed.
       Counts CHARS (not bytes) within the line for UTF-8 correctness. */
    fn line_col(src: &str, byte: usize) -> (usize, usize) {
        let byte = byte.min(src.len());
        let line = src[..byte].matches('\n').count() + 1;
        let line_start = src[..byte].rfind('\n').map_or(0, |p| p + 1);
        let col = src[line_start..byte].chars().count() + 1;
        (line, col)
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
        let mark = src[s_off..e_off].chars().count().max(1);
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
    /* Compact one-line render: `path:line:col: msg`. No source preview.
       Used by WASM (host renders preview) and by tests. */
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

