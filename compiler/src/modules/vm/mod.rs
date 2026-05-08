pub mod types;
mod cache;
mod ops;
mod builtins;
pub(crate) mod handlers;
pub mod optimizer;

use crate::s;
use crate::modules::parser::{OpCode, SSAChunk, Instruction, BUILTIN_TYPES, ssa_strip};
use crate::modules::fx::FxHashMap as HashMap;

pub use types::{Val, HeapObj, HeapPool, VmErr, Limits};

use types::*;
use cache::{OpcodeCache, FastOp, Templates};
use alloc::{string::{String, ToString}, vec::Vec, vec};

/* Saved stack/iter/with depths for unwinding to a try arm's handler. */
pub(crate) struct ExceptionFrame {
    pub handler_ip: usize,
    pub stack_depth: usize,
    pub iter_depth: usize,
    pub with_depth: usize,
}

#[derive(Clone, Copy)]
pub(crate) enum ParamKind { Normal, Star, DoubleStar }

pub struct VM<'a> {
    pub(crate) stack: Vec<Val>,
    pub(crate) heap: HeapPool,
    pub(crate) iter_stack: Vec<IterFrame>,
    pub(crate) yields: Vec<Val>,
    pub(crate) chunk: &'a SSAChunk,
    pub(crate) globals: HashMap<String, Val>,
    pub(crate) live_slots: Vec<Val>,
    pub(crate) templates: Templates,
    pub(crate) budget: usize,
    pub(crate) depth: usize,
    pub(crate) max_calls: usize,
    pub(crate) observed_impure: Vec<bool>,
    pub(crate) exception_stack: Vec<ExceptionFrame>,
    pub(crate) functions: Vec<&'a (Vec<String>, SSAChunk, u16, u16)>,
    // (chunk_ptr, [global_fn_id; chunk.functions.len()]). Linear scan; one
    // entry per chunk (typically <20). Avoids HashMap monomorphization for
    // a tiny pointer-keyed map.
    pub(crate) fn_index: Vec<(*const SSAChunk, Vec<u32>)>,
    // function_parents[fi] = the fi of the def that lexically encloses `fi`,
    // or None for module-level. body_to_fi resolves a body chunk pointer to
    // its owning fi (for caller identification). See build_function_table.
    pub(crate) function_parents: Vec<Option<usize>>,
    pub(crate) body_to_fi: HashMap<*const SSAChunk, usize>,
    pub(crate) body_maps: Vec<HashMap<alloc::string::String, usize>>,
    pub(crate) param_slots: Vec<Vec<(ParamKind, usize)>>,
    pub(crate) slot_templates: Vec<Vec<Val>>,
    pub(crate) nonlocal_tables: Vec<Vec<(usize, usize)>>,
    pub(crate) needs_caller_slots: Vec<bool>,
    /* `is_param_slot[fi][slot]` — true when slot is bound to a formal
       parameter and must NOT be overwritten by caller-slot propagation.
       Replaces a per-call BTreeSet<usize> allocation in exec_call. */
    pub(crate) is_param_slot: Vec<Vec<bool>>,
    /* Body slots holding free-variable references (canonical, version-0,
       not a parameter). Each entry is `(bare_name, body_slot)`. exec_call
       falls back to base-name lookup against the caller's chunk for these
       slots so that names whose SSA version differs between body and caller
       still late-bind correctly — e.g. a code-module splice where each body
       records `is_odd_0` but the parent stores `is_odd_1+`, so exact-name
       propagation can't match. */
    pub(crate) body_free_loads: Vec<Vec<(String, usize)>>,
    pub(crate) is_async: Vec<bool>,
    pub(crate) default_slots: Vec<Vec<(usize, Val)>>,
    pub(crate) opcode_caches: HashMap<*const SSAChunk, OpcodeCache>,
    /* Const pool slice ptrs for caches currently owned by a live exec()
       frame (removed from `opcode_caches` for the duration of the call). */
    pub(crate) active_const_pools: Vec<*const [Val]>,
    /* Cached `Limits::ops == usize::MAX` so the hot dispatch path skips
       the budget decrement on every backward jump. */
    pub(crate) sandbox_off: bool,
    pub(crate) with_stack: Vec<Val>,
    pub(crate) pending_pos_delta: i32,
    pub(crate) pending_kw_delta: i32,
    pub(crate) yielded: bool,
    pub(crate) resume_ip: usize,
    pub output: Vec<String>,
    pub print_hook: Option<fn(&str)>,
    pub input_buffer: Vec<String>,
    pub event_queue: Vec<Val>,
    pub strict_input: bool,
    /* Source byte offset of the deepest frame that raised a propagating
       error in the most recent run(). Set by the dispatch error catch and
       cleared on swallow / at run() entry; readable via error_pos() so the
       outer renderer can attach a Diagnostic-style caret. */
    pub(crate) error_byte_pos: Option<u32>,
}

impl<'a> VM<'a> {
    pub fn new(chunk: &'a SSAChunk) -> Self { Self::with_limits(chunk, Limits::none()) }

    /* Recursively flatten nested `def`s into a single global function table,
       depth-first so closures defined inside nested functions still resolve.
       Class bodies are walked too, since they may host method `def`s.

       Also populates two reverse maps used by the call-site propagation to
       distinguish "calling our own lexical-parent's def" (late-binding —
       captures may be overwritten) from "calling a closure created elsewhere"
       (closure semantics — captures must stick):

         function_parents[fi]      → fi of the def that emitted MakeFunction
                                     for `fi`, None for module-level defs
         body_to_fi[body_chunk_ptr]→ fi whose body that chunk is, used to
                                     resolve the caller's own fi at call time

       Together they let `exec_call` answer: "is the caller the lexical
       parent of the callee?". When yes, propagation overwrites freely
       (Python late-binding); when no, captured slots are protected
       (fixes stacked decorators where each `w` captures its own `f`). */
    fn build_function_table(&mut self, chunk: &'a SSAChunk, parent_fi: Option<usize>) {
        let mut indices = Vec::with_capacity(chunk.functions.len());
        for desc in chunk.functions.iter() {
            let global = self.functions.len() as u32;
            self.functions.push(desc);
            self.function_parents.push(parent_fi);
            self.body_to_fi.insert(&desc.1 as *const _, global as usize);
            indices.push(global);
            self.build_function_table(&desc.1, Some(global as usize));
        }
        self.fn_index.push((chunk as *const _, indices));
        for class_body in chunk.classes.iter() {
            self.build_function_table(class_body, parent_fi);
        }
    }

    /* Materialise an iterable into Vec<Val> for `*args` positional spread. */
    fn iter_to_vec_for_spread(&self, v: Val) -> Result<Vec<Val>, VmErr> {
        if !v.is_heap() {
            return Err(VmErr::Type("argument after * must be an iterable"));
        }
        Ok(match self.heap.get(v) {
            HeapObj::List(rc)  => rc.borrow().clone(),
            HeapObj::Tuple(t)  => t.clone(),
            HeapObj::Set(rc)   => rc.borrow().iter().cloned().collect(),
            HeapObj::Range(s, e, st) => {
                let (s, e, st) = (*s, *e, *st);
                if st == 0 { return Err(VmErr::Value("range() arg 3 must not be zero")); }
                let mut out = Vec::new();
                let mut i = s;
                if st > 0 { while i < e { out.push(Val::int(i)); i += st; } }
                else      { while i > e { out.push(Val::int(i)); i += st; } }
                out
            }
            _ => return Err(VmErr::Type("argument after * must be an iterable")),
        })
    }

    /* Materialise a mapping into (key_str, value) pairs for `**kwargs` spread. */
    fn mapping_to_kw_pairs(&self, v: Val) -> Result<Vec<(Val, Val)>, VmErr> {
        if !v.is_heap() {
            return Err(VmErr::Type("argument after ** must be a mapping"));
        }
        match self.heap.get(v) {
            HeapObj::Dict(rc) => {
                let entries: Vec<(Val, Val)> = rc.borrow().iter().collect();
                for (k, _) in &entries {
                    if !k.is_heap() || !matches!(self.heap.get(*k), HeapObj::Str(_)) {
                        return Err(VmErr::Type("keywords must be strings"));
                    }
                }
                Ok(entries)
            }
            _ => Err(VmErr::Type("argument after ** must be a mapping")),
        }
    }

    /* `Val::undef()` distinguishes unbound slots from None. LoadName
       checks `is_undef()` to raise NameError, avoiding Option<Val> reads. */
    fn fill_builtins(&self, names: &[String]) -> Vec<Val> {
        let mut slots = vec![Val::undef(); names.len()];
        for (i, name) in names.iter().enumerate() {
            if let Some(v) = self.globals.get(name) {
                slots[i] = *v;
            }
        }
        slots
    }

    #[inline]
    fn checked_jump(&mut self, target: usize, limit: usize) -> Result<usize, VmErr> {
        // Non-sandboxed mode has unlimited budget; skip the decrement entirely.
        // Bounds check stays — malformed bytecode could still produce bad targets.
        if !self.sandbox_off {
            if self.budget == 0 { return Err(cold_budget()); }
            self.budget -= 1;
        }
        if target > limit { return Err(cold_runtime("jump target out of bounds")); }
        Ok(target)
    }

    pub(crate) fn str_to_char_vals(&mut self, s: &str) -> Result<Vec<Val>, VmErr> {
        s.chars().map(|c| self.heap.alloc(HeapObj::Str(c.to_string()))).collect()
    }

    fn make_iter_frame(&mut self, obj: Val) -> Result<IterFrame, VmErr> {
        if !obj.is_heap() {
            return Err(VmErr::TypeMsg(s!("'", str self.type_name(obj), "' object is not iterable")));
        }
        Ok(match self.heap.get(obj) {
            HeapObj::Range(s, e, st) => IterFrame::Range { cur: *s, end: *e, step: *st },
            HeapObj::List(v) => IterFrame::Seq { items: v.borrow().clone(), idx: 0 },
            HeapObj::Tuple(v) => IterFrame::Seq { items: v.clone(), idx: 0 },
            HeapObj::Dict(p) => IterFrame::Seq { items: p.borrow().keys().collect(), idx: 0 },
            HeapObj::Set(s) => {
                let mut items: Vec<Val> = s.borrow().iter().cloned().collect();
                self.sort_set_items(&mut items);
                IterFrame::Seq { items, idx: 0 }
            },
            HeapObj::Str(s) => {
                let s = s.clone();
                let items = self.str_to_char_vals(&s)?;
                IterFrame::Seq { items, idx: 0 }
            },
            HeapObj::Coroutine(..) => return Ok(IterFrame::Coroutine(obj)),
            _ => return Err(VmErr::TypeMsg(s!("'", str self.type_name(obj), "' object is not iterable"))),
        })
    }

    fn exec_unpack_seq(&mut self, expected: usize) -> Result<(), VmErr> {
        let obj = self.pop()?;
        if !obj.is_heap() { return Err(cold_type("cannot unpack non-sequence")); }
        let items: Vec<Val> = match self.heap.get(obj) {
            HeapObj::List(v) => v.borrow().clone(),
            HeapObj::Tuple(v) => v.clone(),
            HeapObj::Str(s) => {
                let s = s.clone();
                let out = self.str_to_char_vals(&s)?;
                if out.len() > expected {
                    return Err(cold_value("too many values to unpack"));
                } else if out.len() < expected {
                    return Err(cold_value("not enough values to unpack"));
                }
                out
            },
            _ => return Err(cold_type("cannot unpack non-sequence")),
        };
        if items.len() > expected {
            return Err(cold_value("too many values to unpack"));
        } else if items.len() < expected {
            return Err(cold_value("not enough values to unpack"));
        }
        for item in items.into_iter().rev() { self.push(item); }
        Ok(())
    }

    /* Pick the first defined Phi source; if both are undef fall back to None. */
    fn exec_phi(op: u16, rip: usize, phi_map: &[usize], slots: &mut [Val], phi_sources: &[(u16, u16)]) {
        let (ia, ib) = phi_sources[phi_map[rip]];
        let a = slots[ia as usize];
        let val = if !a.is_undef() { a }
                  else { let b = slots[ib as usize]; if !b.is_undef() { b } else { Val::none() } };
        slots[op as usize] = val;
    }

    pub fn with_limits(chunk: &'a SSAChunk, limits: Limits) -> Self {
        let sandbox_off = limits.ops == usize::MAX;
        let mut vm = Self {
            stack: Vec::with_capacity(256),
            iter_stack: Vec::with_capacity(16),
            yields: Vec::new(),
            chunk,
            heap: HeapPool::new(limits.heap),
            globals: HashMap::default(),
            live_slots: Vec::new(),
            templates: Templates::new(),
            budget: limits.ops,
            depth: 0,
            max_calls: limits.calls,
            with_stack: Vec::new(),
            pending_pos_delta: 0,
            pending_kw_delta: 0,
            yielded: false,
            resume_ip: 0,
            strict_input: false,
            output: Vec::new(),
            print_hook: None,
            input_buffer: Vec::new(),
            event_queue: Vec::new(),
            observed_impure: Vec::new(),
            exception_stack: Vec::new(),
            error_byte_pos: None,
            functions: Vec::new(),
            fn_index: Vec::new(),
            function_parents: Vec::new(),
            body_to_fi: HashMap::default(),
            body_maps: Vec::new(),
            param_slots: Vec::new(),
            slot_templates: Vec::new(),
            nonlocal_tables: Vec::new(),
            needs_caller_slots: Vec::new(),
            is_param_slot: Vec::new(),
            body_free_loads: Vec::new(),
            is_async: Vec::new(),
            default_slots: Vec::new(),
            opcode_caches: HashMap::default(),
            active_const_pools: Vec::new(),
            sandbox_off,
        };
        vm.build_function_table(chunk, None);
        vm.body_maps = vm.functions.iter().map(|(_, body, _, _)| {
            body.names.iter().enumerate().map(|(i, n)| (n.clone(), i)).collect()
        }).collect();
        vm.param_slots = (0..vm.functions.len()).map(|fi| {
            let (params, _, _, _) = vm.functions[fi];
            let bm = &vm.body_maps[fi];
            params.iter().map(|p| {
                let (kind, bare) = if let Some(stripped) = p.strip_prefix("**") {
                    (ParamKind::DoubleStar, stripped)
                } else if let Some(stripped) = p.strip_prefix('*') {
                    (ParamKind::Star, stripped)
                } else {
                    (ParamKind::Normal, p.as_str())
                };
                let slot = bm.get(&s!(str bare, "_0")).copied().unwrap_or(usize::MAX);
                (kind, slot)
            }).collect()
        }).collect();

        // Pre-compute nonlocal resolution: (canonical_body_slot, canonical_body_slot).
        vm.nonlocal_tables = vm.functions.iter().map(|(_, body, _, _)| {
            body.nonlocals.iter().filter_map(|base| {
                // Must skip names that lack a `_<digits>` SSA suffix entirely:
                // body.names also holds the bare `Nonlocal` opcode operand,
                // and that slot isn't the variable's canonical SSA root. So
                // we explicitly require the suffix-bearing form here, not
                // ssa_strip's "fall through to bare on missing suffix" shape.
                let canon = body.names.iter().enumerate()
                    .find(|(_, n)| n.rfind('_').map(|p| &n[..p]) == Some(base.as_str()))
                    .map(|(i, _)| body.alias_groups.get(i).and_then(|g| g.first().copied()).unwrap_or(i as u16) as usize)?;
                Some((canon, canon))
            }).collect()
        }).collect();

        // True iff the body references names not in params/builtins/captures.
        vm.needs_caller_slots = (0..vm.functions.len()).map(|fi| {
            let (params, body, _, _) = vm.functions[fi];
            let param_names: crate::modules::fx::FxHashSet<&str> = params.iter()
                .map(|p| p.trim_start_matches('*')).collect();
            body.names.iter().any(|n| {
                let base = ssa_strip(n);
                !param_names.contains(base) && !vm.globals.contains_key(n)
            })
        }).collect();

        // Bitmap of slots bound to formal parameters — used to skip caller-slot
        // propagation without allocating a BTreeSet per call.
        vm.is_param_slot = (0..vm.functions.len()).map(|fi| {
            let (_, body, _, _) = vm.functions[fi];
            let n_slots = body.names.len();
            let mut bm = alloc::vec![false; n_slots];
            for &(_, slot) in &vm.param_slots[fi] {
                if slot < n_slots { bm[slot] = true; }
            }
            bm
        }).collect();

        // Body free-load slots: canonical, non-parameter names that the body
        // never writes to. exec_call resolves these at call time by base-name
        // fallback against the caller's chunk so a body reference whose SSA
        // version differs from the caller's still binds — e.g. mutual
        // recursion across spliced top-level defs, where each body records
        // the version current at body-compile time but the splicer ends up
        // storing under a higher version. Built once at VM init.
        vm.body_free_loads = (0..vm.functions.len()).map(|fi| {
            let (_, body, _, _) = vm.functions[fi];
            let param_bm = &vm.is_param_slot[fi];
            let mut written: crate::modules::fx::FxHashSet<usize> = crate::modules::fx::FxHashSet::default();
            for ins in &body.instructions {
                if matches!(ins.opcode, OpCode::StoreName | OpCode::Phi) {
                    written.insert(ins.operand as usize);
                }
            }
            body.names.iter().enumerate().filter_map(|(slot, name)| {
                let canon = body.alias_groups.get(slot)
                    .and_then(|g| g.first().copied())
                    .unwrap_or(slot as u16) as usize;
                if canon != slot { return None; }
                if param_bm.get(slot).copied().unwrap_or(false) { return None; }
                if written.contains(&slot) { return None; }
                let p = name.rfind('_')?;
                name[p+1..].parse::<u32>().ok()?;
                Some((name[..p].to_string(), slot))
            }).collect()
        }).collect();

        // Default-slot table: (slot, placeholder) entries the call path overwrites.
        vm.default_slots = (0..vm.functions.len()).map(|fi| {
            let (params, _, n_defaults, _) = vm.functions[fi];
            let n_defaults = *n_defaults as usize;
            if n_defaults == 0 { return Vec::new(); }
            let pslots = &vm.param_slots[fi];
            let n_params = params.len();
            let offset = n_params.saturating_sub(n_defaults);
            (0..n_defaults).filter_map(|di| {
                pslots.get(offset + di).map(|&(_, slot)| (slot, Val::none()))
            }).collect()
        }).collect();
        for &name in BUILTIN_TYPES {
            if let Ok(type_obj) = vm.heap.alloc(HeapObj::Type(name.to_string())) {
                vm.globals.insert(name.to_string(), type_obj);
                vm.globals.insert(s!(str name, "_0"), type_obj);
            }
        }
        // Module identity. The entry chunk always runs as "__main__", matching
        // CPython's convention so the `if __name__ == "__main__":` guard works
        // without special-casing in the parser. Inserted before slot_templates
        // is built below so name references get pre-resolved into slots.
        if let Ok(main_name) = vm.heap.alloc(HeapObj::Str("__main__".to_string())) {
            vm.globals.insert("__name__".to_string(), main_name);
            vm.globals.insert("__name___0".to_string(), main_name);
        }
        // Register builtins as first-class NativeFn values so `print = print`,
        // `f = len; f([1,2])`, etc. work without a separate dispatch path.
        let builtin_fns: &[NativeFnId] = &[
            NativeFnId::Print, NativeFnId::Len, NativeFnId::Abs, NativeFnId::Str,
            NativeFnId::Int, NativeFnId::Float, NativeFnId::Bool, NativeFnId::Type,
            NativeFnId::Chr, NativeFnId::Ord, NativeFnId::Range, NativeFnId::Round,
            NativeFnId::Min, NativeFnId::Max, NativeFnId::Sum, NativeFnId::Sorted,
            NativeFnId::Enumerate, NativeFnId::Zip, NativeFnId::List, NativeFnId::Tuple,
            NativeFnId::Dict, NativeFnId::Set, NativeFnId::IsInstance, NativeFnId::Input,
            NativeFnId::All, NativeFnId::Any, NativeFnId::Bin, NativeFnId::Oct,
            NativeFnId::Hex, NativeFnId::Divmod, NativeFnId::Pow, NativeFnId::Repr,
            NativeFnId::Reversed, NativeFnId::Callable, NativeFnId::Id, NativeFnId::Hash,
            NativeFnId::Format, NativeFnId::Ascii, NativeFnId::GetAttr, NativeFnId::HasAttr, NativeFnId::Next,
            NativeFnId::Run, NativeFnId::Sleep, NativeFnId::Receive,
            NativeFnId::Map, NativeFnId::Filter, NativeFnId::Iter,
        ];
        for &id in builtin_fns {
            if let Ok(v) = vm.heap.alloc(HeapObj::NativeFn(id)) {
                let name = id.name();
                vm.globals.insert(name.to_string(), v);
                vm.globals.insert(s!(str name, "_0"), v);
            }
        }
        // Slot templates need every global already populated — built once
        // after the loop, not per builtin (the previous per-iteration build
        // was 44× wasted work in cold init).
        vm.slot_templates = vm.functions.iter().map(|(_, body, _, _)| {
            vm.fill_builtins(&body.names)
        }).collect();
        vm
    }

    pub fn run(&mut self) -> Result<Val, VmErr> {
        self.error_byte_pos = None;
        let mut slots = self.fill_builtins(&self.chunk.names);
        self.exec(self.chunk, &mut slots)
    }

    /* Source byte offset of the last propagating runtime error, or None if
       run() succeeded / hasn't been called. Renderers turn this into the
       fancy `--> path:line:col` form via parser::Diagnostic. */
    pub fn error_pos(&self) -> Option<usize> { self.error_byte_pos.map(|p| p as usize) }

    /* Mark all reachable roots and sweep. mark() is a no-op on non-heap
       values, so undef/None/int/float/bool slots are free to scan. */
    fn collect(&mut self, current_slots: &[Val]) {
        for &v in &self.stack { self.heap.mark(v); }
        for &v in &self.with_stack { self.heap.mark(v); }
        for &v in &self.yields { self.heap.mark(v); }
        for &v in &self.event_queue { self.heap.mark(v); }
        for &v in current_slots { self.heap.mark(v); }
        for &v in &self.live_slots { self.heap.mark(v); }
        for tpl in &self.slot_templates {
            for &v in tpl { self.heap.mark(v); }
        }
        for &v in self.globals.values() { self.heap.mark(v); }
        for frame in &self.iter_stack {
            match frame {
                IterFrame::Seq { items, .. } => {
                    for &v in items { self.heap.mark(v); }
                }
                IterFrame::Coroutine(v) => self.heap.mark(*v),
                IterFrame::Range { .. } => {}
            }
        }
        for cache in self.opcode_caches.values() {
            if let Some(consts) = cache.const_vals_opt() {
                for &v in consts { self.heap.mark(v); }
            }
        }
        // SAFETY: each ptr is pushed at exec() entry and popped before the
        // owning OpcodeCache is moved back into `opcode_caches`. The Vec's
        // heap allocation is stable across that move.
        for i in 0..self.active_const_pools.len() {
            let consts: &[Val] = unsafe { &*self.active_const_pools[i] };
            for &v in consts { self.heap.mark(v); }
        }
        self.templates.mark_all(&mut self.heap);
        self.heap.sweep();
    }

    pub fn heap_usage(&self) -> usize { self.heap.usage() }
    pub fn cache_stats(&self) -> (usize, usize) {
        (self.templates.count(), self.chunk.instructions.len())
    }

    // Stack helpers.

    #[inline] pub(crate) fn push(&mut self, v: Val) { self.stack.push(v); }

    #[inline] pub(crate) fn pop(&mut self) -> Result<Val, VmErr> {
        self.stack.pop().ok_or(cold_runtime("stack underflow"))
    }
    #[inline] pub(crate) fn pop2(&mut self) -> Result<(Val, Val), VmErr> {
        let b = self.pop()?; let a = self.pop()?; Ok((a, b))
    }
    #[inline] pub(crate) fn pop_n(&mut self, n: usize) -> Result<Vec<Val>, VmErr> {
        let at = self.stack.len().checked_sub(n)
            .ok_or(cold_runtime("stack underflow"))?;
        Ok(self.stack.split_off(at))
    }

    /* Inline-cache fast path. Peeks the stack and only pops on success;
       returns Ok(false) with the stack untouched on a type-guard miss
       so the caller can fall back to the generic handler and deopt the IC. */
    #[inline]
    fn exec_fast(&mut self, fast: FastOp) -> Result<bool, VmErr> {
        let len = self.stack.len();
        if len < 2 { return Ok(false); }

        let a = self.stack[len - 2];
        let b = self.stack[len - 1];

        let result = match fast {
            FastOp::AddFloat if a.is_float() && b.is_float() => Val::float(a.as_float() + b.as_float()),
            FastOp::AddInt if a.is_int() && b.is_int() => {
                match a.as_int().checked_add(b.as_int()).and_then(Val::int_checked) {
                    Some(v) => v,
                    None => return Ok(false),
                }
            }
            FastOp::SubInt if a.is_int() && b.is_int() => {
                match a.as_int().checked_sub(b.as_int()).and_then(Val::int_checked) {
                    Some(v) => v,
                    None => return Ok(false),
                }
            }
            FastOp::MulInt if a.is_int() && b.is_int() => {
                let r = a.as_int() as i128 * b.as_int() as i128;
                if r >= Val::INT_MIN as i128 && r <= Val::INT_MAX as i128 { Val::int(r as i64) } else { return Ok(false); }
            }
            FastOp::MulFloat if a.is_float() && b.is_float() => Val::float(a.as_float() * b.as_float()),
            FastOp::ModInt if a.is_int() && b.is_int() => {
                let bv = b.as_int();
                if bv == 0 { return Ok(false); }
                Val::int(((a.as_int() % bv) + bv) % bv)
            }
            FastOp::FloorDivInt if a.is_int() && b.is_int() => {
                let bv = b.as_int();
                if bv == 0 { return Ok(false); }
                Val::int(a.as_int().div_euclid(bv))
            }

            FastOp::LtInt if a.is_int() && b.is_int() => Val::bool(a.as_int() < b.as_int()),
            FastOp::LtFloat if a.is_float() && b.is_float() => Val::bool(a.as_float() < b.as_float()),
            FastOp::EqInt if a.is_int() && b.is_int() => Val::bool(a.as_int() == b.as_int()),
            FastOp::GtInt if a.is_int() && b.is_int() => Val::bool(a.as_int() > b.as_int()),
            FastOp::LtEqInt if a.is_int() && b.is_int() => Val::bool(a.as_int() <= b.as_int()),
            FastOp::GtEqInt if a.is_int() && b.is_int() => Val::bool(a.as_int() >= b.as_int()),
            FastOp::NotEqInt if a.is_int() && b.is_int() => Val::bool(a.as_int() != b.as_int()),

            FastOp::AddStr | FastOp::EqStr if a.is_heap() && b.is_heap() => {
                let (sa, sb) = match (self.heap.get(a), self.heap.get(b)) {
                    (HeapObj::Str(x), HeapObj::Str(y)) => (x.clone(), y.clone()),
                    _ => return Ok(false),
                };
                match fast {
                    FastOp::AddStr => {
                        let mut r = String::with_capacity(sa.len() + sb.len());
                        r.push_str(&sa); r.push_str(&sb);
                        self.heap.alloc(HeapObj::Str(r))?
                    }
                    _ => Val::bool(sa == sb),
                }
            }

            _ => return Ok(false),
        };

        self.stack.truncate(len - 2);
        self.push(result);
        Ok(true)
    }

    /* Main dispatch loop. Walks the fused instruction stream (LoadAttr+Call
       already collapsed to CallMethod+CallMethodArgs); checks the IC inline
       for hot arith/compare opcodes. */
    pub(crate) fn exec(&mut self, chunk: &SSAChunk, slots: &mut [Val]) -> Result<Val, VmErr> {

        let slots_base = self.live_slots.len();
        let exc_base   = self.exception_stack.len();
        let key        = chunk as *const _;

        let mut cache = self.opcode_caches.remove(&key)
            .unwrap_or_else(|| OpcodeCache::new(chunk));
        cache.ensure_fused(chunk);
        // Pre-materialise the constant pool here (not in OpcodeCache::new)
        // because Str/BigInt allocate into the live HeapPool.
        if let Err(e) = cache.ensure_const_vals(chunk, &mut self.heap) {
            self.opcode_caches.insert(key, cache);
            return Err(e);
        }

        // Hoist immutable views out of the loop so the inner dispatch doesn't
        // re-unwrap `cache.fused_ref()` / `const_vals_ref()` per instruction.
        // SAFETY: the slices borrow from `cache`, which is a stack local that
        // lives for the entire exec() call; no other path mutates the cache.
        let insns_ptr: *const [Instruction] = cache.fused_ref();
        let consts_ptr: *const [Val] = cache.const_vals_ref();
        self.active_const_pools.push(consts_ptr);
        let result: Result<Val, VmErr> = (|| {
            // SAFETY: see comment above.
            let insns: &[Instruction] = unsafe { &*insns_ptr };
            let consts: &[Val] = unsafe { &*consts_ptr };
            let n          = insns.len();
            let mut ip     = self.resume_ip;
            self.resume_ip = 0;

            loop {
                if ip >= n {
                    self.exception_stack.truncate(exc_base);
                    return Ok(Val::none());
                }

                let rip = ip;
                match self.dispatch(chunk, slots, &mut cache, insns, consts, &mut ip) {
                    Ok(None) => {
                        if self.yielded {
                            let val = self.pop().unwrap_or(Val::none());
                            // Skip the PopTop following Yield on resume so the
                            // yielded value isn't discarded twice.
                            self.resume_ip = if ip < n && matches!(insns.get(ip), Some(ins) if ins.opcode == OpCode::PopTop) { ip + 1 } else { ip };
                            self.live_slots.truncate(slots_base);
                            self.exception_stack.truncate(exc_base);
                            return Ok(val);
                        }
                    }
                    Ok(Some(v)) => {
                        self.live_slots.truncate(slots_base);
                        self.exception_stack.truncate(exc_base);
                        return Ok(v);
                    }
                    Err(e) => {
                        // Record the deepest frame's source position. The first
                        // dispatch loop to catch an error (the innermost) wins;
                        // outer dispatches that re-catch the propagating Err see
                        // Some(_) and skip. Reset on swallow below so a later
                        // unhandled error in the same run anchors correctly.
                        if self.error_byte_pos.is_none() {
                            self.error_byte_pos = chunk.resolve(rip as u32);
                        }
                        if self.exception_stack.len() > exc_base {
                            let frame = self.exception_stack.pop().unwrap();
                            self.stack.truncate(frame.stack_depth);
                            self.iter_stack.truncate(frame.iter_depth);
                            self.with_stack.truncate(frame.with_depth);
                            self.pending_pos_delta = 0;
                            self.pending_kw_delta  = 0;
                            self.error_byte_pos    = None;
                            // Cold path: allocate-once String for the lookup
                            // key. `Raised` carries the user-supplied class
                            // name so `except <Type>` can match it.
                            let msg: alloc::string::String = match &e {
                                VmErr::ZeroDiv     => "ZeroDivisionError".into(),
                                VmErr::Type(_)     => "TypeError".into(),
                                VmErr::TypeMsg(_)  => "TypeError".into(),
                                VmErr::Value(_)    => "ValueError".into(),
                                VmErr::Attribute(_)=> "AttributeError".into(),
                                VmErr::Name(_)     => "NameError".into(),
                                VmErr::CallDepth   => "RecursionError".into(),
                                VmErr::Heap        => "MemoryError".into(),
                                VmErr::Budget      => "RuntimeError".into(),
                                VmErr::Runtime(_)  => "RuntimeError".into(),
                                VmErr::Raised(s)   => s.clone(),
                            };
                            let exc = if let Some(&type_val) = self.globals.get(&msg) {
                                type_val
                            } else {
                                self.heap.alloc(HeapObj::Str(msg))?
                            };
                            self.push(exc);
                            ip = frame.handler_ip;
                        } else {
                            return Err(e);
                        }
                    }
                }
            }
        })();

        self.active_const_pools.pop();
        self.opcode_caches.insert(key, cache);
        result
    }

    pub(crate) fn exec_from(&mut self, chunk: &SSAChunk, slots: &mut [Val], start_ip: usize) -> Result<Val, VmErr> {
        self.resume_ip = start_ip;
        self.exec(chunk, slots)
    }

    /* Resolve the bound method on the receiver and call it directly,
       avoiding a BoundMethod heap allocation. Args come from the paired
       CallMethodArgs instruction. */
    fn exec_call_method(&mut self, attr_idx: u16, call_op: u16, chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        let raw = call_op as usize;
        let num_kw  = (raw >> 8) & 0xFF;
        let num_pos = raw & 0xFF;
        let total = num_pos + 2 * num_kw;

        let mut stack_items: Vec<Val> = (0..total)
            .map(|_| self.pop())
            .collect::<Result<_, _>>()?;
        stack_items.reverse();

        let kw_flat: Vec<Val> = stack_items.split_off(num_pos);
        let positional = stack_items;

        let obj = self.pop()?;
        let ty = self.type_name(obj);
        let name = chunk.names.get(attr_idx as usize)
            .ok_or(VmErr::Runtime("CallMethod: bad name index"))?;

        // Module attribute call: look up the attr on the module and call it
        // directly. No `self` is prepended (modules aren't classes).
        if obj.is_heap()
            && let HeapObj::Module(mod_name, attrs) = self.heap.get(obj) {
                let bare = ssa_strip(name);
                if let Some((_, attr)) = attrs.iter().find(|(n, _)| n == bare) {
                    let callee = *attr;
                    self.push(callee);
                    for a in &positional { self.push(*a); }
                    for a in &kw_flat   { self.push(*a); }
                    let argc = positional.len() as u16;
                    let encoded = ((kw_flat.len() as u16 / 2) << 8) | argc;
                    return self.exec_call(encoded, chunk, slots);
                }
                let mod_name = mod_name.clone();
                return Err(VmErr::Attribute(s!(
                    "module '", str &mod_name, "' has no attribute '", str bare, "'")));
            }

        // User-defined method on Instance: call with self prepended.
        if obj.is_heap()
            && let HeapObj::Instance(cls_val, _) = self.heap.get(obj) {
                let cls_val = *cls_val;
                if cls_val.is_heap()
                    && let HeapObj::Class(_, methods) = self.heap.get(cls_val)
                    && let Some((_, mv)) = methods.iter().find(|(n, _)| n == name.as_str()) {
                        let mv = *mv;
                        self.push(mv);
                        self.push(obj);
                        for a in &positional { self.push(*a); }
                        let argc = (positional.len() + 1) as u16;
                        let encoded = ((kw_flat.len() as u16 / 2) << 8) | argc;
                        return self.exec_call(encoded, chunk, slots);
                    }
            }
        let method_id = handlers::methods::lookup_method(ty, name.as_str())
            .ok_or_else(|| VmErr::Attribute(s!("'", str ty, "' object has no attribute '", str &name, "'")))?;

        self.exec_bound_method(obj, method_id, positional, kw_flat)
    }

    /* Hot dispatch. Takes the fused instruction slice and constants slice as
       borrowed parameters so the inner loop never re-unwraps cache.fused_ref()
       or cache.const_vals_ref(). */
    #[inline]
    fn dispatch(
        &mut self, chunk: &SSAChunk, slots: &mut [Val],
        cache: &mut OpcodeCache,
        insns: &[Instruction], consts: &[Val],
        ip: &mut usize,
    ) -> Result<Option<Val>, VmErr> {
        let n = insns.len();
        let ins = insns[*ip];
        let rip = *ip;
        let op = ins.operand;
        *ip += 1;

        match ins.opcode {
            // Short-circuit jumps.
            OpCode::JumpIfFalseOrPop => {
                let v = *self.stack.last().ok_or(cold_runtime("stack underflow"))?;
                if !self.truthy(v) { *ip = op as usize; }
                else { self.pop()?; }
            }
            OpCode::JumpIfTrueOrPop => {
                let v = *self.stack.last().ok_or(cold_runtime("stack underflow"))?;
                if self.truthy(v) { *ip = op as usize; }
                else { self.pop()?; }
            }

            // Hot opcodes.
            OpCode::LoadName => {
                // Single u64 compare for unbound-slot detection — no Option.
                let v = slots[op as usize];
                if v.is_undef() {
                    return Err(VmErr::Name(ssa_strip(&chunk.names[op as usize]).into()));
                }
                self.push(v);
            }
            OpCode::StoreName => {
                self.handle_store(op, slots)?;
            }
            OpCode::LoadConst => {
                // Constants are pre-materialised at exec entry, so this is a
                // single bounds-checked index instead of a Value→Val conversion.
                let v = *consts.get(op as usize)
                    .ok_or(cold_runtime("constant index out of bounds"))?;
                self.push(v);
            }

            // Arith / compare with inline cache. Add/Sub/Mul/Mod/FloorDiv
            // and every comparison op share the same fast-path / record /
            // deopt cycle, so they collapse into one branch with handler
            // selection at the bottom.
            OpCode::Add | OpCode::Sub | OpCode::Mul
            | OpCode::Mod | OpCode::FloorDiv
            | OpCode::Eq | OpCode::Lt | OpCode::NotEq
            | OpCode::Gt | OpCode::LtEq | OpCode::GtEq => {
                if let Some(fast) = cache.get_fast(rip) {
                    if self.exec_fast(fast)? { return Ok(None); }
                    cache.invalidate(rip);
                }
                if matches!(ins.opcode, OpCode::Eq | OpCode::Lt | OpCode::NotEq
                    | OpCode::Gt | OpCode::LtEq | OpCode::GtEq)
                {
                    self.handle_compare(ins.opcode, rip, cache)?;
                } else {
                    self.handle_arith(ins.opcode, rip, cache)?;
                }
            }
            OpCode::Div | OpCode::Pow | OpCode::Minus => {
                self.handle_arith(ins.opcode, rip, cache)?;
            }

            OpCode::Jump => { *ip = self.checked_jump(op as usize, n)?; }
            OpCode::JumpIfFalse => {
                let v = self.pop()?;
                if !self.truthy(v) { *ip = self.checked_jump(op as usize, n)?; }
            }
            OpCode::ForIter => {
                if !self.sandbox_off {
                    if self.budget == 0 { return Err(cold_budget()); }
                    self.budget -= 1;
                }
                if self.heap.needs_gc() { self.collect(slots); }
                // Coroutine iteration: resume via call instead of next_item().
                if let Some(IterFrame::Coroutine(coro_val)) = self.iter_stack.last() {
                    let cv = *coro_val;
                    self.push(cv);
                    self.exec_call(0, chunk, slots)?;
                    let result = self.pop().unwrap_or(Val::none());
                    if result.is_none() {
                        self.iter_stack.pop();
                        *ip = op as usize;
                    } else {
                        self.push(result);
                    }
                    return Ok(None);
                }
                match self.iter_stack.last_mut().and_then(|f| f.next_item()) {
                    Some(item) => self.push(item),
                    None => {
                        self.iter_stack.pop();
                        if op as usize > n { return Err(cold_runtime("jump target out of bounds")); }
                        *ip = op as usize;
                    }
                }
            }
            OpCode::PopTop => { self.pop()?; }
            OpCode::ReturnValue => {
                let result = if self.stack.is_empty() { Val::none() } else { self.pop()? };
                return Ok(Some(result));
            }

            // Warm opcodes.
            OpCode::GetItem => { self.get_item()?; }

            OpCode::Call | OpCode::CallPrint | OpCode::CallLen | OpCode::CallAbs
            | OpCode::CallStr | OpCode::CallInt | OpCode::CallFloat | OpCode::CallBool
            | OpCode::CallType | OpCode::CallChr | OpCode::CallOrd | OpCode::CallSorted
            | OpCode::CallList | OpCode::CallTuple | OpCode::CallEnumerate | OpCode::CallIsInstance
            | OpCode::CallRange | OpCode::CallRound | OpCode::CallMin | OpCode::CallMax
            | OpCode::CallSum | OpCode::CallZip | OpCode::CallDict | OpCode::CallSet
            | OpCode::CallInput | OpCode::MakeFunction | OpCode::MakeCoroutine
            | OpCode::CallAll | OpCode::CallAny | OpCode::CallBin | OpCode::CallOct
            | OpCode::CallHex | OpCode::CallDivmod | OpCode::CallPow | OpCode::CallRepr
            | OpCode::CallReversed | OpCode::CallCallable | OpCode::CallId | OpCode::CallHash
            | OpCode::CallExtern => {
                self.handle_function(ins.opcode, op, chunk, slots)?;
            }

            OpCode::GetIter => {
                let obj = self.pop()?;
                let frame = self.make_iter_frame(obj)?;
                self.iter_stack.push(frame);
            }
            OpCode::LoadTrue  => self.push(Val::bool(true)),
            OpCode::LoadFalse => self.push(Val::bool(false)),
            OpCode::LoadNone  => self.push(Val::none()),
            OpCode::Not => self.handle_logic(OpCode::Not)?,

            OpCode::Phi => {
                Self::exec_phi(op, rip, &chunk.phi_map, slots, &chunk.phi_sources);
            }

            OpCode::LoadAttr => { self.handle_load_attr(op, chunk)?; }

            // Fused method call.
            OpCode::CallMethod => {
                // Next instruction is the paired CallMethodArgs (consumed here).
                let call_op = insns[*ip].operand;
                *ip += 1;
                self.exec_call_method(op, call_op, chunk, slots)?;
            }
            OpCode::CallMethodArgs => {
                // Always consumed by CallMethod; reaching here is a bytecode bug.
                return Err(cold_runtime("CallMethodArgs reached dispatch unpaired"));
            }

            // Cold opcodes.
            OpCode::And | OpCode::Or => {
                // Both should be short-circuited via JumpIfFalseOrPop / JumpIfTrueOrPop
                // by the parser; reaching here is a codegen bug.
                return Err(cold_runtime("And/Or reached VM dispatch (should be short-circuited)"));
            }

            OpCode::MakeClass => {
                let ci = op as usize;
                let body = &chunk.classes[ci];
                let mut class_slots = self.fill_builtins(&body.names);
                self.exec(body, &mut class_slots)?;
                let mut methods: Vec<(alloc::string::String, Val)> = Vec::new();
                for (i, name) in body.names.iter().enumerate() {
                    if let Some(&v) = class_slots.get(i)
                        && !v.is_undef() && v.is_heap()
                        && matches!(self.heap.get(v), HeapObj::Func(..)) {
                            let base = ssa_strip(name);
                            if !methods.iter().any(|(n, _)| n == base) {
                                methods.push((base.to_string(), v));
                            }
                        }
                }
                let next_op = cache.fused_ref().get(*ip).map(|i| i.operand).unwrap_or(0);
                let name_str = chunk.names.get(next_op as usize)
                    .map(|n| ssa_strip(n))
                    .unwrap_or("?").to_string();
                let cls = self.heap.alloc(HeapObj::Class(name_str, methods))?;
                self.push(cls);
            }
            OpCode::StoreAttr => {
                let value = self.pop()?;
                let obj = self.pop()?;
                if !obj.is_heap() { return Err(cold_type("cannot set attribute")); }
                let name = chunk.names.get(op as usize)
                    .ok_or(cold_runtime("StoreAttr: bad name index"))?.clone();
                let key = self.heap.alloc(HeapObj::Str(name))?;
                match self.heap.get_mut(obj) {
                    HeapObj::Instance(_, attrs) => {
                        attrs.borrow_mut().insert(key, value);
                    }
                    _ => return Err(cold_type("cannot set attribute on this type")),
                }
            }

            OpCode::LoadExtern => {
                let f = chunk.extern_table.get(op as usize)
                    .ok_or(cold_runtime("LoadExtern: extern index out of bounds"))?
                    .clone();
                let v = self.heap.alloc(HeapObj::Extern(f))?;
                self.push(v);
            }

            OpCode::BuildModule => {
                /* Stack on entry, top→bottom: module-name, then `op` pairs of
                   (attr_name_str, attr_value). Build the attr vec preserving
                   declaration order (innermost-first when popped). */
                let total = (op as usize) * 2 + 1;
                let mut frame = self.pop_n(total)?;
                let module_name_val = frame.pop().ok_or(cold_runtime("BuildModule: empty stack"))?;
                let module_name = match self.heap.get(module_name_val) {
                    HeapObj::Str(s) => s.clone(),
                    _ => return Err(cold_runtime("BuildModule: module name not a string")),
                };
                let mut attrs: Vec<(alloc::string::String, Val)> = Vec::with_capacity(op as usize);
                let mut it = frame.into_iter();
                while let Some(name_v) = it.next() {
                    let val = it.next().ok_or(cold_runtime("BuildModule: malformed attr stack"))?;
                    let n = match self.heap.get(name_v) {
                        HeapObj::Str(s) => s.clone(),
                        _ => return Err(cold_runtime("BuildModule: attr name not a string")),
                    };
                    attrs.push((n, val));
                }
                let m = self.heap.alloc(HeapObj::Module(module_name, attrs))?;
                self.push(m);
            }

            other => self.dispatch_generic(other, op, slots)?,
        }
        Ok(None)
    }

    fn dispatch_generic(
        &mut self, opcode: OpCode, operand: u16,
        slots: &mut [Val],
    ) -> Result<(), VmErr> {
        match opcode {
            OpCode::BitAnd | OpCode::BitOr | OpCode::BitXor
            | OpCode::BitNot | OpCode::Shl | OpCode::Shr => self.handle_bitwise(opcode)?,
            OpCode::In | OpCode::NotIn | OpCode::Is | OpCode::IsNot => self.handle_identity(opcode)?,

            OpCode::BuildList | OpCode::BuildTuple | OpCode::BuildDict
            | OpCode::BuildString | OpCode::BuildSet | OpCode::BuildSlice => self.handle_build(opcode, operand)?,

            OpCode::StoreItem => { self.mark_impure(); self.store_item()?; }
            OpCode::DelItem => { self.mark_impure(); self.del_item()?; }
            OpCode::UnpackSequence | OpCode::UnpackEx | OpCode::FormatValue => self.handle_container(opcode, operand)?,

            OpCode::ListAppend | OpCode::SetAdd | OpCode::MapAdd => self.handle_comprehension(opcode)?,

            OpCode::Yield => self.handle_yield()?,
            OpCode::LoadEllipsis => {
                let v = self.heap.alloc(HeapObj::Str("...".to_string()))?;
                self.push(v);
            }
            OpCode::Dup => {
                let v = *self.stack.last().ok_or(cold_runtime("stack underflow"))?;
                self.push(v);
            }
            OpCode::Dup2 => {
                let b = self.pop()?; let a = self.pop()?;
                self.push(a); self.push(b); self.push(a); self.push(b);
            }
            OpCode::Assert | OpCode::Del | OpCode::Global | OpCode::Nonlocal
            | OpCode::TypeAlias
            | OpCode::Raise | OpCode::RaiseFrom | OpCode::Await | OpCode::YieldFrom => {
                self.handle_side(opcode, operand, slots)?;
            }
            OpCode::SetupExcept => {
                self.exception_stack.push(ExceptionFrame {
                    handler_ip:  operand as usize,
                    stack_depth: self.stack.len(),
                    iter_depth:  self.iter_stack.len(),
                    with_depth:  self.with_stack.len(),
                });
            }
            OpCode::SetupWith => {
                let _ = operand;
                let cm = self.pop()?;
                self.with_stack.push(cm);
                self.push(cm);
            }
            OpCode::ExitWith => {
                let _ = operand;
                let cm = self.with_stack.pop()
                    .ok_or(cold_runtime("ExitWith without matching SetupWith"))?;
                if let Some(&top) = self.stack.last()
                    && top.0 == cm.0 { self.pop()?; }
            }
            OpCode::UnpackArgs => {
                let val = self.pop()?;
                match operand {
                    1 => {
                        let items = self.iter_to_vec_for_spread(val)?;
                        let n = items.len() as i32;
                        for v in items { self.push(v); }
                        self.pending_pos_delta += n - 1;
                    }
                    2 => {
                        let pairs = self.mapping_to_kw_pairs(val)?;
                        let n = pairs.len() as i32;
                        for (k, v) in pairs { self.push(k); self.push(v); }
                        self.pending_pos_delta -= 1;
                        self.pending_kw_delta  += n;
                    }
                    _ => return Err(cold_runtime("UnpackArgs: bad operand")),
                }
            }
            OpCode::PopExcept => { self.exception_stack.pop(); }
            // Emitted by `break` inside a for-loop to drop the abandoned
            // iterator so the surrounding for-iter reads from its own iter.
            OpCode::PopIter => { self.iter_stack.pop(); }
            OpCode::MakeClass | OpCode::StoreAttr => {
                return Err(cold_runtime("MakeClass/StoreAttr must be in main dispatch"));
            }
            _ => return Err(cold_runtime("unexpected opcode in generic dispatch")),
        }
        Ok(())
    }
}
