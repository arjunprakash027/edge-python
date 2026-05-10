use crate::s;
use alloc::{string::{String, ToString}, vec::Vec};

use crate::modules::parser::{OpCode, SSAChunk, Instruction, ssa_strip};

use super::{ExceptionFrame, VM, handlers};
use super::types::*;
use super::cache::{OpcodeCache, FastOp};

/* Three-way result of a fast-path attempt; see exec_fast for semantics. */
enum FastOutcome { Done, TypeMiss, Overflow }

impl<'a> VM<'a> {

    /* Inline-cache fast path. Peeks the stack and only pops on success.
       Three outcomes:
         Done     — the op ran inline; stack consumed and result pushed.
         TypeMiss — operands didn't match the speculation; deopt the IC.
         Overflow — types matched but the result can't be represented (int
                    overflow, division by zero); the slow handler will
                    raise the proper Python exception. The IC stays warm
                    because the speculation was correct: the op IS hot
                    on these types, only this *one* input pair fell out. */
    #[inline]
    fn exec_fast(&mut self, fast: FastOp) -> Result<FastOutcome, VmErr> {
        let len = self.stack.len();
        if len < 2 { return Ok(FastOutcome::TypeMiss); }

        let a = self.stack[len - 2];
        let b = self.stack[len - 1];

        let result = match fast {
            FastOp::AddFloat if a.is_float() && b.is_float() => Val::float(a.as_float() + b.as_float()),
            FastOp::AddInt if a.is_int() && b.is_int() => {
                match a.as_int().checked_add(b.as_int()).and_then(Val::int_checked) {
                    Some(v) => v,
                    None => return Ok(FastOutcome::Overflow),
                }
            }
            FastOp::SubInt if a.is_int() && b.is_int() => {
                match a.as_int().checked_sub(b.as_int()).and_then(Val::int_checked) {
                    Some(v) => v,
                    None => return Ok(FastOutcome::Overflow),
                }
            }
            FastOp::MulInt if a.is_int() && b.is_int() => {
                let r = a.as_int() as i128 * b.as_int() as i128;
                if r >= Val::INT_MIN as i128 && r <= Val::INT_MAX as i128 { Val::int(r as i64) } else { return Ok(FastOutcome::Overflow); }
            }
            FastOp::MulFloat if a.is_float() && b.is_float() => Val::float(a.as_float() * b.as_float()),
            FastOp::ModInt if a.is_int() && b.is_int() => {
                let bv = b.as_int();
                if bv == 0 { return Ok(FastOutcome::Overflow); }
                Val::int(((a.as_int() % bv) + bv) % bv)
            }
            FastOp::FloorDivInt if a.is_int() && b.is_int() => {
                let bv = b.as_int();
                if bv == 0 { return Ok(FastOutcome::Overflow); }
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
                    _ => return Ok(FastOutcome::TypeMiss),
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

            _ => return Ok(FastOutcome::TypeMiss),
        };

        self.stack.truncate(len - 2);
        self.push(result);
        Ok(FastOutcome::Done)
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
        // because Str allocates into the live HeapPool.
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
                            self.pending.pos_delta = 0;
                            self.pending.kw_delta  = 0;
                            self.error_byte_pos    = None;
                            // Caught exception: discard the partial traceback
                            // so a later unhandled error doesn't carry stale
                            // frames from the swallowed one.
                            self.call_stack.clear();
                            // Cold path: allocate-once String for the lookup
                            // key. `Raised` carries the user-supplied class
                            // name so `except <Type>` can match it.
                            let msg: String = match &e {
                                VmErr::ZeroDiv     => "ZeroDivisionError".into(),
                                VmErr::Overflow    => "OverflowError".into(),
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
                            // Prefer the pending ExcInstance Val (built by
                            // `raise X("msg")`) so `except X as e` binds the
                            // actual instance — `e.args` then works. Fall
                            // back to the Type from globals for bare-name
                            // raises and to a fresh Str for ad-hoc messages.
                            let exc = if let Some(v) = self.pending.exc_val.take() {
                                v
                            } else if let Some(&type_val) = self.globals.get(&msg) {
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
        let name = chunk.names.get(attr_idx as usize)
            .ok_or(VmErr::Runtime("CallMethod: bad name index"))?
            .clone();

        match self.resolve_attr(obj, &name)? {
            handlers::methods::AttrLookup::ModuleAttr(callee)
            | handlers::methods::AttrLookup::ClassMember(callee) => {
                // Direct call on the resolved value, no `self` prepended.
                self.push(callee);
                for a in &positional { self.push(*a); }
                for a in &kw_flat   { self.push(*a); }
                let argc = positional.len() as u16;
                let encoded = ((kw_flat.len() as u16 / 2) << 8) | argc;
                self.exec_call(encoded, chunk, slots)
            }
            handlers::methods::AttrLookup::InstanceMethod { recv, func } => {
                // Prepend `self`. Pre-existing behaviour: kwargs aren't
                // forwarded — see the encoded num_kw below mirrors the
                // original code path so any call site that ever passed
                // kwargs through this branch keeps the same observable
                // shape.
                self.push(func);
                self.push(recv);
                for a in &positional { self.push(*a); }
                let argc = (positional.len() + 1) as u16;
                let encoded = ((kw_flat.len() as u16 / 2) << 8) | argc;
                self.exec_call(encoded, chunk, slots)
            }
            handlers::methods::AttrLookup::BuiltinMethod(id) => {
                self.exec_bound_method(obj, id, &positional, &kw_flat)
            }
            handlers::methods::AttrLookup::InstanceField(_) => {
                // `inst.field()` where `field` isn't a class method falls
                // through to "no such method" — pre-existing semantics
                // had no Instance-field-as-callable path here.
                let ty = self.type_name(obj);
                Err(VmErr::Attribute(s!(
                    "'", str ty, "' object has no attribute '", str &name, "'")))
            }
            handlers::methods::AttrLookup::ExcArgs(_) => {
                // `e.args()` was an AttributeError under the previous
                // implementation; preserve that.
                let ty = self.type_name(obj);
                Err(VmErr::Attribute(s!(
                    "'", str ty, "' object has no attribute '", str &name, "'")))
            }
        }
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
                // Mirror entry-chunk Module stores to `globals` so
                // `import_module(name)` (and any cross-frame accessor)
                // finds the alias the user wrote in their `import`
                // statement. Restricted to entry chunk + Module Vals
                // so user-level assignments to plain values don't
                // pollute globals.
                if core::ptr::eq(chunk, self.chunk) {
                    let v = slots[op as usize];
                    if v.is_heap()
                        && matches!(self.heap.get(v), HeapObj::Module(..))
                        && let Some(name) = chunk.names.get(op as usize)
                    {
                        let bare = ssa_strip(name).to_string();
                        self.globals.insert(bare, v);
                    }
                }
            }
            OpCode::LoadConst => {
                // Constants are pre-materialised at exec entry, so this is a
                // single bounds-checked index instead of a Value->Val conversion.
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
                    match self.exec_fast(fast)? {
                        FastOutcome::Done => return Ok(None),
                        /* Speculation was right (matching types); the slow handler
                           will raise the proper Python exception. Keep the IC. */
                        FastOutcome::Overflow => {}
                        FastOutcome::TypeMiss => cache.invalidate(rip),
                    }
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
                // Snapshot the byte_pos of this call site so exec_call can
                // record it on the new CallFrame. Prefer call_byte_pos
                // (instr-level) and fall back to the enclosing statement.
                self.pending.call_byte_pos = chunk.resolve_call(rip as u32)
                    .or_else(|| chunk.resolve(rip as u32));
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
                // Collect every defined slot as a class member: methods (Func),
                // class-level constants (`Status.IDLE = 0`), and any other Val
                // produced by class-body execution.
                let mut methods: Vec<(String, Val)> = Vec::new();
                for (i, name) in body.names.iter().enumerate() {
                    if let Some(&v) = class_slots.get(i)
                        && !v.is_undef() {
                            let base = ssa_strip(name);
                            // Builtin globals also live in class_slots (filled
                            // by fill_builtins). Skip them so `MyClass.print`
                            // doesn't shadow the global builtin.
                            let is_builtin_shadow = v.is_heap()
                                && matches!(self.heap.get(v), HeapObj::NativeFn(_))
                                && self.globals.get(base).copied() == Some(v);
                            if is_builtin_shadow { continue; }
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

            OpCode::LoadModule => {
                let entry = chunk.imports.get(op as usize)
                    .ok_or(cold_runtime("LoadModule: import index out of range"))?;
                let v = *self.module_table.get(&entry.spec)
                    .ok_or(cold_runtime("LoadModule: module not initialised"))?;
                self.push(v);
            }

            OpCode::BuildModule => {
                /* Stack on entry, top->bottom: module-name, then `op` pairs of
                   (attr_name_str, attr_value). Build the attr vec preserving
                   declaration order (innermost-first when popped). */
                let total = (op as usize) * 2 + 1;
                let mut frame = self.pop_n(total)?;
                let module_name_val = frame.pop().ok_or(cold_runtime("BuildModule: empty stack"))?;
                let module_name = match self.heap.get(module_name_val) {
                    HeapObj::Str(s) => s.clone(),
                    _ => return Err(cold_runtime("BuildModule: module name not a string")),
                };
                let mut attrs: Vec<(String, Val)> = Vec::with_capacity(op as usize);
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
                let v = self.heap.alloc(HeapObj::Ellipsis)?;
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
            | OpCode::Raise | OpCode::RaiseFrom | OpCode::Await => {
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
                        self.pending.pos_delta += n - 1;
                    }
                    2 => {
                        let pairs = self.mapping_to_kw_pairs(val)?;
                        let n = pairs.len() as i32;
                        for (k, v) in pairs { self.push(k); self.push(v); }
                        self.pending.pos_delta -= 1;
                        self.pending.kw_delta  += n;
                    }
                    _ => return Err(cold_runtime("UnpackArgs: bad operand")),
                }
            }
            OpCode::PopExcept => { self.exception_stack.pop(); }
            // Emitted by `break` inside a for-loop to drop the abandoned
            // iterator so the surrounding for-iter reads from its own iter.
            OpCode::PopIter => { self.iter_stack.pop(); }
            _ => return Err(cold_runtime("unexpected opcode in generic dispatch")),
        }
        Ok(())
    }
}
