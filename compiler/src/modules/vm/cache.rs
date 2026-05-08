use super::types::{Val, HeapObj, HeapPool, VmErr, eq_vals_with_heap, cold_overflow};
use crate::modules::parser::{OpCode, SSAChunk, Instruction, Value};

use alloc::{vec, vec::Vec, string::ToString};

/* Type-specialised binop variants reachable from the inline cache. */
#[derive(Debug, Clone, Copy)]
pub enum FastOp {
    AddInt, AddFloat, AddStr,
    SubInt, SubFloat,
    MulInt, MulFloat,
    LtInt, LtFloat,
    GtInt, LtEqInt, GtEqInt,
    EqInt, EqStr,
    NotEqInt,
    ModInt, FloorDivInt
}

/* Promote to `fast` after this many hits with a stable type key. */
const QUICK_THRESH: u8 = 4;

#[derive(Clone, Default)]
struct CacheSlot {
    type_key: u8,
    hits: u8,
    fast: Option<FastOp>,
}

pub struct OpcodeCache {
    slots: Vec<CacheSlot>,
    fused: Option<Vec<Instruction>>,
    /* Pre-materialised constant pool. Built once per chunk on first exec
       so LoadConst is a single indexed load instead of a per-iteration
       Value→Val conversion (strings/bigints would heap-alloc). */
    const_vals: Option<Vec<Val>>,
}

impl OpcodeCache {
    pub fn new(chunk: &SSAChunk) -> Self {
        Self {
            slots: vec![CacheSlot::default(); chunk.instructions.len()],
            fused: None,
            const_vals: None,
        }
    }

    /* Compile the fused instruction stream on first access; reuse afterwards. */
    pub fn ensure_fused(&mut self, chunk: &SSAChunk) -> &[Instruction] {
        if self.fused.is_none() {
            self.fused = Some(fuse_method_calls(chunk));
        }
        self.fused.as_ref().unwrap()
    }

    /* Direct access (caller must have called ensure_fused). */
    pub fn fused_ref(&self) -> &[Instruction] {
        self.fused.as_ref().expect("fused code not compiled")
    }

    /* Materialise the constant pool. Int/Float/Bool/None become inline Vals
       (no heap touch); Str allocates once and is then shared. Ints outside
       the 47-bit Val range trap as OverflowError at materialisation. */
    pub fn ensure_const_vals(&mut self, chunk: &SSAChunk, heap: &mut HeapPool)
        -> Result<&[Val], VmErr>
    {
        if self.const_vals.is_none() {
            let mut out = Vec::with_capacity(chunk.constants.len());
            for c in &chunk.constants {
                let v = match c {
                    Value::Int(i) => {
                        if *i >= Val::INT_MIN && *i <= Val::INT_MAX { Val::int(*i) }
                        else { return Err(cold_overflow()); }
                    }
                    Value::Float(f) => Val::float(*f),
                    Value::Bool(b) => Val::bool(*b),
                    Value::None => Val::none(),
                    Value::Str(s) => heap.alloc(HeapObj::Str(s.to_string()))?,
                    Value::Bytes(b) => heap.alloc(HeapObj::Bytes(b.clone()))?,
                };
                out.push(v);
            }
            self.const_vals = Some(out);
        }
        Ok(self.const_vals.as_ref().unwrap())
    }

    /* Direct access (caller must have called ensure_const_vals). */
    pub fn const_vals_ref(&self) -> &[Val] {
        self.const_vals.as_ref().expect("const pool not materialized")
    }

    pub fn const_vals_opt(&self) -> Option<&[Val]> {
        self.const_vals.as_deref()
    }

    pub fn record(&mut self, ip: usize, opcode: &OpCode, ta: u8, tb: u8) {
        let Some(s) = self.slots.get_mut(ip) else { return };
        let key = (ta << 4) | (tb & 0xF);
        if s.type_key == key {
            s.hits = s.hits.saturating_add(1);
            if s.hits >= QUICK_THRESH && s.fast.is_none() {
                s.fast = Self::specialize(opcode, ta, tb);
            }
        } else {
            *s = CacheSlot { type_key: key, hits: 1, fast: None };
        }
    }

    #[inline]
    pub fn get_fast(&self, ip: usize) -> Option<FastOp> {
        self.slots.get(ip).and_then(|s| s.fast)
    }

    pub fn invalidate(&mut self, ip: usize) {
        if let Some(s) = self.slots.get_mut(ip) { *s = CacheSlot::default(); }
    }

    fn specialize(opcode: &OpCode, ta: u8, tb: u8) -> Option<FastOp> {
        match (opcode, ta, tb) {
            (OpCode::Add, 1, 1) => Some(FastOp::AddInt),    (OpCode::Add, 2, 2) => Some(FastOp::AddFloat),
            (OpCode::Add, 5, 5) => Some(FastOp::AddStr),    (OpCode::Sub, 1, 1) => Some(FastOp::SubInt),
            (OpCode::Sub, 2, 2) => Some(FastOp::SubFloat),  (OpCode::Mul, 1, 1) => Some(FastOp::MulInt),
            (OpCode::Mul, 2, 2) => Some(FastOp::MulFloat),  (OpCode::Lt, 1, 1) => Some(FastOp::LtInt),
            (OpCode::Lt, 2, 2) => Some(FastOp::LtFloat),    (OpCode::Eq, 1, 1) => Some(FastOp::EqInt),
            (OpCode::Eq, 5, 5) => Some(FastOp::EqStr),      (OpCode::Gt, 1, 1) => Some(FastOp::GtInt),
            (OpCode::LtEq, 1, 1) => Some(FastOp::LtEqInt),  (OpCode::GtEq, 1, 1) => Some(FastOp::GtEqInt),
            (OpCode::NotEq, 1, 1) => Some(FastOp::NotEqInt),
            (OpCode::Mod, 1, 1) => Some(FastOp::ModInt),
            (OpCode::FloorDiv, 1, 1) => Some(FastOp::FloorDivInt),
            _ => None,
        }
    }
}

// Template memoization for pure functions.

fn args_match(e: &TplEntry, args: &[Val], h: u64, heap: &super::types::HeapPool) -> bool {
    e.hash == h
    && e.args.len() == args.len()
    && e.args.iter().zip(args).all(|(a, b)| eq_vals_with_heap(*a, *b, heap))
}

const TPL_THRESH: u32 = 2;

struct TplEntry { args: Vec<Val>, result: Val, hits: u32, hash: u64 }

fn hash_args(args: &[Val]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for v in args {
        h ^= v.0;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// Indexed by `fi` (function id, dense from 0..N). Vec gives O(1) lookup
// without a HashMap monomorphization.
pub struct Templates { slots: Vec<Vec<TplEntry>> }

impl Templates {
    pub fn new() -> Self { Self { slots: Vec::new() } }

    pub fn lookup(&self, fi: usize, args: &[Val], heap: &super::types::HeapPool) -> Option<Val> {
        let h = hash_args(args);
        self.slots.get(fi)?.iter()
            .find(|e| e.hits >= TPL_THRESH && args_match(e, args, h, heap))
            .map(|e| e.result)
    }

    pub fn record(&mut self, fi: usize, args: &[Val], result: Val, heap: &super::types::HeapPool) {
        if self.slots.len() <= fi { self.slots.resize_with(fi + 1, Vec::new); }
        let h = hash_args(args);
        let v = &mut self.slots[fi];
        if let Some(e) = v.iter_mut().find(|e| args_match(e, args, h, heap)) {
            e.hits += 1; e.result = result;
        } else if v.len() < 256 {
            v.push(TplEntry { args: args.to_vec(), result, hits: 1, hash: h });
        }
    }

    pub fn count(&self) -> usize {
        self.slots.iter().flat_map(|v| v.iter()).filter(|e| e.hits >= TPL_THRESH).count()
    }

    pub fn mark_all(&self, heap: &mut super::types::HeapPool) {
        for slot in &self.slots {
            for e in slot {
                for &v in &e.args { heap.mark(v); }
                heap.mark(e.result);
            }
        }
    }
}

/* Fuse adjacent LoadAttr+Call into CallMethod+CallMethodArgs in-place.
   The two opcodes change but operands and instruction count stay, so
   jump targets remain valid. Compiled once per chunk and then cached.

   Only fires when Call's operand is zero (no args, no kwargs). When the
   call has args, the parser interleaves them between LoadAttr and Call
   (e.g. `x.foo(a)` → LoadAttr foo, LoadName a, Call(1)) and adjacent
   LoadAttr+Call signals an attribute access in the LAST argument
   position (e.g. `f(self.n)` → LoadName self, LoadAttr n, Call(1)) —
   fusing that mis-treats the arg expression as the call target. */
fn fuse_method_calls(chunk: &SSAChunk) -> Vec<Instruction> {
    let src = &chunk.instructions;
    let n = src.len();
    let mut out = src.clone();

    let mut i = 0;
    while i + 1 < n {
        if src[i].opcode == OpCode::LoadAttr
            && src[i + 1].opcode == OpCode::Call
            && src[i + 1].operand == 0
        {
            out[i].opcode = OpCode::CallMethod;
            out[i + 1].opcode = OpCode::CallMethodArgs;
            i += 2;
            continue;
        }
        i += 1;
    }
    out
}