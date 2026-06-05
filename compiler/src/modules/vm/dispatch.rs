use crate::s;
use alloc::{string::{String, ToString}, vec::Vec};

use crate::modules::parser::{OpCode, SSAChunk, Instruction, ssa_strip};

use super::{ExceptionFrame, VM, handlers};
use super::types::*;
use super::cache::{OpcodeCache, FastOp, InstanceCache};

/* Three-way result of a fast-path attempt; see exec_fast for semantics. */
enum FastOutcome { Done, TypeMiss, Overflow }

impl<'a> VM<'a> {

    /* IC fast path: Done (consumed+pushed) / TypeMiss (deopt) / Overflow (keep IC, slow handler raises). */
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

            FastOp::EqStr if a.is_heap() && b.is_heap() => {
                match (self.heap.get(a), self.heap.get(b)) {
                    (HeapObj::Str(x), HeapObj::Str(y)) => Val::bool(x == y),
                    _ => return Ok(FastOutcome::TypeMiss),
                }
            }

            FastOp::AddStr if a.is_heap() && b.is_heap() => {
                let (sa, sb) = match (self.heap.get(a), self.heap.get(b)) {
                    (HeapObj::Str(x), HeapObj::Str(y)) => (x.clone(), y.clone()),
                    _ => return Ok(FastOutcome::TypeMiss),
                };
                let mut r = String::with_capacity(sa.len() + sb.len());
                r.push_str(&sa); r.push_str(&sb);
                self.heap.alloc(HeapObj::Str(r))?
            }

            _ => return Ok(FastOutcome::TypeMiss),
        };

        self.stack.truncate(len - 2);
        self.push(result);
        Ok(FastOutcome::Done)
    }

    /* Instance-dunder fast path. Guards the receiver's class identity, invokes the pre-resolved method bypassing `resolve_attr_silent`, and treats `NotImplemented` as a deopt so reflected dispatch can take over via the slow path. Restores the stack on miss so the slow handler reads its operands unchanged. */
    #[inline]
    fn exec_inst(&mut self, inst: InstanceCache, chunk: &SSAChunk, slots: &mut [Val]) -> Result<FastOutcome, VmErr> {
        let arity = inst.arity as usize;
        let len = self.stack.len();
        if len < arity { return Ok(FastOutcome::TypeMiss); }

        let recv_idx = len - arity;
        let recv = self.stack[recv_idx];
        if !recv.is_heap() { return Ok(FastOutcome::TypeMiss); }
        let class_val = match self.heap.get(recv) {
            HeapObj::Instance(c, _) => *c,
            _ => return Ok(FastOutcome::TypeMiss),
        };
        if class_val.as_heap() != inst.class { return Ok(FastOutcome::TypeMiss); }

        if self.depth >= self.max_calls { return Err(cold_depth()); }

        // Snapshot the operand window before mutating; reused to roll back on deopt.
        let mut operands: Vec<Val> = Vec::with_capacity(arity);
        operands.extend_from_slice(&self.stack[recv_idx..len]);
        self.stack.truncate(recv_idx);

        // SAFETY: `method_bits` was recorded from a live `Val` and `Class` references are immutable, so the function still lives on the heap.
        let method = unsafe { Val::from_raw(inst.method_bits) };
        self.pending.method_binding = Some((class_val, recv));
        self.push(method);
        for &v in &operands { self.push(v); }
        self.exec_call(arity as u16, chunk, slots)?;

        let result = self.pop()?;
        if self.heap.is_not_implemented(result) {
            // Deopt: restore the original stack window so the slow handler sees its operands.
            for &v in &operands { self.push(v); }
            return Ok(FastOutcome::TypeMiss);
        }
        self.push(result);
        Ok(FastOutcome::Done)
    }

    /* Post-success recording for the instance-dunder IC; ignored when the receiver isn't an instance or the method isn't on its class. */
    #[inline]
    pub(crate) fn record_dunder_hit(&self, ip: usize, cache: &mut OpcodeCache, recv: Val, name: &str, arity: u8) {
        if !recv.is_heap() { return; }
        let HeapObj::Instance(cls, _) = self.heap.get(recv) else { return; };
        let cls = *cls;
        let Some((method, _)) = self.lookup_class_member(cls, name) else { return; };
        cache.record_inst(ip, cls.as_heap(), method, arity);
    }

    /* Main dispatch loop. Walks the fused instruction stream (LoadAttr+Call already collapsed to CallMethod+CallMethodArgs); checks the IC inline for hot arith/compare opcodes. */
    pub(crate) fn exec(&mut self, chunk: &SSAChunk, slots: &mut [Val]) -> Result<Val, VmErr> {

        let slots_base = self.live_slots.len();
        // `resume_coroutine` pre-pushes restored exception frames before calling us; honor its override so dispatch's handler search includes them.
        let exc_base = self.pending_exec_exc_base.take().unwrap_or(self.exception_stack.len());
        let key = chunk as *const _;

        let mut cache = self.opcode_caches.remove(&key).unwrap_or_else(|| OpcodeCache::new(chunk));
        cache.ensure_fused(chunk);
        // Pre-materialise the constant pool here (not in OpcodeCache::new) because Str allocates into the live HeapPool.
        if let Err(e) = cache.ensure_const_vals(chunk, &mut self.heap) {
            self.opcode_caches.insert(key, cache);
            return Err(e);
        }

        // Hoist slices out of the loop; cache outlives exec() and isn't mutated meanwhile.
        let insns_ptr: *const [Instruction] = cache.fused_ref();
        let consts_ptr: *const [Val] = cache.const_vals_ref();
        self.active_const_pools.push(consts_ptr);
        let result: Result<Val, VmErr> = (|| {
            // SAFETY: see comment above.
            let insns: &[Instruction] = unsafe { &*insns_ptr };
            let consts: &[Val] = unsafe { &*consts_ptr };
            let n = insns.len();
            let mut ip = self.resume_ip;
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
                            // Event yields keep the None placeholder (overwritten by `run_push_event` before resume). Sync sub-call yields pushed nothing, the helper's return lands on the stack when its frame completes, so don't pop and don't skip the next PopTop. Child-wait yields keep the placeholder (wake-loop overwrites it with the target's result). Host-call yields keep the placeholder (overwritten by `set_host_result`).
                            let event_yield = self.pending.event_wait_request;
                            let sub_call_yield = !self.pending_sync_frames.is_empty();
                            let child_yield = self.pending.waiting_for_children.is_some();
                            let host_yield = self.pending.host_call_request;
                            let val = if event_yield || sub_call_yield || child_yield || host_yield { Val::none() } else { self.pop().unwrap_or(Val::none()) };
                            self.resume_ip = if !event_yield && !sub_call_yield && !child_yield && !host_yield && ip < n && matches!(insns.get(ip), Some(ins) if ins.opcode == OpCode::PopTop) { ip + 1 } else { ip };
                            self.live_slots.truncate(slots_base);
                            // DON'T truncate exception_stack here, frames pushed in this exec belong to active try/except blocks; the enclosing `resume_coroutine` drains them into the coroutine's saved state so `try` survives the yield.
                            return Ok(val);
                        }
                    }
                    Ok(Some(v)) => {
                        self.live_slots.truncate(slots_base);
                        self.exception_stack.truncate(exc_base);
                        return Ok(v);
                    }
                    Err(e) => {
                        // HostYield is a control-flow signal, not a Python exception, bypass try/except.
                        if matches!(e, VmErr::HostYield(_)) {
                            return Err(e);
                        }
                        // Innermost frame wins; cleared below on swallow so later errors re-anchor.
                        if self.error_byte_pos.is_none() {
                            self.error_byte_pos = chunk.resolve(rip as u32);
                        }
                        if self.exception_stack.len() > exc_base {
                            let frame = self.exception_stack.pop().unwrap();
                            self.stack.truncate(frame.stack_depth);
                            self.iter_stack.truncate(frame.iter_depth);
                            self.with_stack.truncate(frame.with_depth);
                            self.pending.pos_delta = 0;
                            self.pending.kw_delta = 0;
                            self.error_byte_pos = None;
                            // Drop partial traceback so a later error doesn't inherit stale frames.
                            self.call_stack.clear();
                            let msg = e.class_name();
                            // Prefer the user-raised instance; synthesize one for native errors.
                            let exc = if let Some(v) = self.pending.exc_val.take() {
                                v
                            } else {
                                let msg_val = self.heap.alloc(HeapObj::Str(e.message()))?;
                                self.heap.alloc(HeapObj::ExcInstance(msg, alloc::vec![msg_val]))?
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

    /* Resolve the receiver's method and call directly; args come from CallMethodArgs. */
    fn exec_call_method(&mut self, attr_idx: u16, call_op: u16, chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        let raw = call_op as usize;
        let num_kw = (raw >> 8) & 0xFF;
        let num_pos = raw & 0xFF;
        let total = num_pos + 2 * num_kw;

        let mut stack_items: Vec<Val> = (0..total).map(|_| self.pop()).collect::<Result<_, _>>()?;
        stack_items.reverse();

        let kw_flat: Vec<Val> = stack_items.split_off(num_pos);
        let positional = stack_items;

        let obj = self.pop()?;
        let name = chunk.names.get(attr_idx as usize).ok_or(VmErr::Runtime("CallMethod: bad name index"))?.clone();

        let lookup = match self.resolve_attr(obj, &name) {
            Ok(l) => l,
            Err(VmErr::Attribute(msg)) => {
                //  if `__getattr__` resolves the name to a callable, invoke it with the positional args.
                if let Some(v) = self.try_getattr_fallback(obj, &name, chunk, slots)? {
                    self.push(v);
                    for a in &positional { self.push(*a); }
                    for a in &kw_flat { self.push(*a); }
                    let argc = positional.len() as u16;
                    let encoded = ((kw_flat.len() as u16 / 2) << 8) | argc;
                    return self.exec_call(encoded, chunk, slots);
                }
                return Err(VmErr::Attribute(msg));
            }
            Err(other) => return Err(other),
        };
        match lookup {
            handlers::methods::AttrLookup::ModuleAttr(callee)
            | handlers::methods::AttrLookup::ClassMember(callee) => {
                // Direct call on the resolved value, no `self` prepended.
                self.push(callee);
                for a in &positional { self.push(*a); }
                for a in &kw_flat { self.push(*a); }
                let argc = positional.len() as u16;
                let encoded = ((kw_flat.len() as u16 / 2) << 8) | argc;
                self.exec_call(encoded, chunk, slots)
            }
            handlers::methods::AttrLookup::InstanceMethod { recv, func, class } => {
                // Prepend `self`; kwargs aren't forwarded (preserved behaviour). `super()` reads the binding off `pending`.
                self.pending.method_binding = Some((class, recv));
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
                // No Instance-field-as-callable path; reports as missing attribute.
                let ty = self.type_name(obj);
                Err(VmErr::Attribute(s!("'", str ty, "' object has no attribute '", str &name, "'")))
            }
            handlers::methods::AttrLookup::ExcArgs(_) | handlers::methods::AttrLookup::Name(_) => {
                // `e.args()` / `f.__name__()`: the value isn't callable, reports as missing attribute.
                let ty = self.type_name(obj);
                Err(VmErr::Attribute(s!("'", str ty, "' object has no attribute '", str &name, "'")))
            }
            handlers::methods::AttrLookup::PropertyGet { recv, getter } => {
                // Materialise the value first, then call it with the user's args, `foo.prop(arg)` where `prop` returns a callable.
                if self.depth >= self.max_calls { return Err(cold_depth()); }
                self.push(getter);
                self.push(recv);
                self.exec_call(1, chunk, slots)?;
                let result = self.pop()?;
                self.push(result);
                for a in &positional { self.push(*a); }
                for a in &kw_flat { self.push(*a); }
                let argc = positional.len() as u16;
                let encoded = ((kw_flat.len() as u16 / 2) << 8) | argc;
                self.exec_call(encoded, chunk, slots)
            }
            handlers::methods::AttrLookup::PropertySetterRef(prop) => {
                let v = self.heap.alloc(HeapObj::PropertySetter(prop))?;
                self.push(v);
                for a in &positional { self.push(*a); }
                for a in &kw_flat { self.push(*a); }
                let argc = positional.len() as u16;
                let encoded = ((kw_flat.len() as u16 / 2) << 8) | argc;
                self.exec_call(encoded, chunk, slots)
            }
        }
    }

    /* Hot dispatch; slices passed in so the loop never re-unwraps the cache views. */
    #[inline]
    fn dispatch(&mut self, chunk: &SSAChunk, slots: &mut [Val], cache: &mut OpcodeCache, insns: &[Instruction], consts: &[Val], ip: &mut usize) -> Result<Option<Val>, VmErr> {
        let n = insns.len();
        let ins = insns[*ip];
        let rip = *ip;
        let op = ins.operand;
        *ip += 1;

        match ins.opcode {
            // Short-circuit jumps; instance `__bool__` / `__len__` may run via `truthy_op`.
            OpCode::JumpIfFalseOrPop => {
                let v = *self.stack.last().ok_or(cold_runtime("stack underflow"))?;
                if !self.truthy_op(v, chunk, slots)? { *ip = op as usize; }
                else { self.pop()?; }
            }
            OpCode::JumpIfTrueOrPop => {
                let v = *self.stack.last().ok_or(cold_runtime("stack underflow"))?;
                if self.truthy_op(v, chunk, slots)? { *ip = op as usize; }
                else { self.pop()?; }
            }

            // Hot opcodes.
            OpCode::LoadName => {
                // Malformed bytecode can carry an out-of-range slot; treat it as unbound.
                let v = slots.get(op as usize).copied().unwrap_or(Val::undef());
                if v.is_undef() {
                    let name = chunk.names.get(op as usize).map(|n| ssa_strip(n)).unwrap_or_default();
                    return Err(VmErr::Name(name.into()));
                }
                self.push(v);
            }
            OpCode::StoreName => {
                self.handle_store(op, slots)?;
                // Mirror entry-chunk stores into `module_state` so functions with `global X` see updates, and mirror Module values into `globals` so `import_module()` finds module aliases.
                if core::ptr::eq(chunk, self.chunk)
                    && let Some(name) = chunk.names.get(op as usize)
                {
                    let v = slots[op as usize];
                    let bare = ssa_strip(name).to_string();
                    self.module_state.insert(bare.clone(), v);
                    if v.is_heap() && matches!(self.heap.get(v), HeapObj::Module(..)) {
                        self.globals.insert(bare, v);
                    }
                }
            }
            OpCode::LoadGlobal => {
                let name = chunk.names.get(op as usize).ok_or(cold_runtime("LoadGlobal: name index out of bounds"))?;
                let v = self.module_state.get(name.as_str()).copied()
                    .or_else(|| self.globals.get(name.as_str()).copied())
                    .unwrap_or(Val::undef());
                if v.is_undef() {
                    return Err(VmErr::Name(name.clone()));
                }
                self.push(v);
            }
            OpCode::StoreGlobal => {
                let v = self.pop()?;
                let name = chunk.names.get(op as usize).ok_or(cold_runtime("StoreGlobal: name index out of bounds"))?;
                self.module_state.insert(name.clone(), v);
            }
            OpCode::LoadConst => {
                // Constants are pre-materialised at exec entry, so this is a single bounds-checked index instead of a Value->Val conversion.
                let v = *consts.get(op as usize)
                    .ok_or(cold_runtime("constant index out of bounds"))?;
                self.push(v);
            }

            // Extracted to exec_arith_or_compare so VM::dispatch doesn't fuse the IC/deopt cycle into its own symbol.
            OpCode::Add | OpCode::Sub | OpCode::Mul
            | OpCode::Mod | OpCode::FloorDiv
            | OpCode::Eq | OpCode::Lt | OpCode::NotEq
            | OpCode::Gt | OpCode::LtEq | OpCode::GtEq
            | OpCode::Div | OpCode::Pow | OpCode::Minus => {
                self.exec_arith_or_compare(ins.opcode, rip, cache, chunk, slots)?;
            }

            OpCode::Jump => {
                let target = self.checked_jump(op as usize, n)?;
                // Backward jumps are loop back-edges; charge them so `while` is bounded like `for`.
                if target <= rip && !self.sandbox_off {
                    if self.budget == 0 { return Err(cold_budget()); }
                    self.budget -= 1;
                }
                *ip = target;
            }
            OpCode::JumpIfFalse => {
                let v = self.pop()?;
                if !self.truthy_op(v, chunk, slots)? { *ip = self.checked_jump(op as usize, n)?; }
            }
            OpCode::ForIter => self.exec_for_iter(op, ip, n, chunk, slots)?,
            OpCode::PopTop => { self.pop()?; }
            OpCode::ReturnValue => {
                let result = if self.stack.is_empty() { Val::none() } else { self.pop()? };
                return Ok(Some(result));
            }

            // Warm opcodes.
            OpCode::GetItem => {
                // `Series[i]`-style hot loop, bypass `resolve_attr_silent("__getitem__")` once the site is monomorphic.
                if let Some(inst) = cache.get_inst(rip) {
                    match self.exec_inst(inst, chunk, slots)? {
                        FastOutcome::Done => return Ok(None),
                        FastOutcome::Overflow => {}
                        FastOutcome::TypeMiss => cache.invalidate_inst(rip),
                    }
                }
                self.get_item(rip, chunk, slots, cache)?;
            }

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
                // Snapshot call-site byte_pos for the new CallFrame; falls back to enclosing stmt.
                self.pending.call_byte_pos = chunk.resolve_call(rip as u32).or_else(|| chunk.resolve(rip as u32));
                self.handle_function(ins.opcode, op, chunk, slots)?;
            }

            OpCode::GetIter => {
                let obj = self.pop()?;
                let frame = self.make_iter_frame(obj, chunk, slots)?;
                self.iter_stack.push(frame);
            }
            OpCode::LoadTrue => self.push(Val::bool(true)),
            OpCode::LoadFalse => self.push(Val::bool(false)),
            OpCode::LoadNone => self.push(Val::none()),
            OpCode::Not => self.handle_logic(OpCode::Not, chunk, slots)?,

            OpCode::Phi => {
                Self::exec_phi(op, rip, &chunk.phi_map, slots, &chunk.phi_sources);
            }

            OpCode::LoadAttr => { self.handle_load_attr(op, chunk, slots)?; }

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
                // Parser should short-circuit these via JumpIf*OrPop; reaching here is a codegen bug.
                return Err(cold_runtime("And/Or reached VM dispatch (should be short-circuited)"));
            }

            OpCode::MakeClass => self.exec_make_class(op, *ip, cache, chunk, slots)?,
            OpCode::StoreAttr => self.exec_store_attr(op, chunk, slots)?,

            OpCode::LoadExtern => {
                let f = chunk.extern_table.get(op as usize).ok_or(cold_runtime("LoadExtern: extern index out of bounds"))?.clone();
                let v = self.heap.alloc(HeapObj::Extern(f))?;
                self.push(v);
            }

            OpCode::LoadModule => {
                let entry = chunk.imports.get(op as usize).ok_or(cold_runtime("LoadModule: import index out of range"))?;
                let v = *self.module_table.get(&entry.spec).ok_or(cold_runtime("LoadModule: module not initialised"))?;
                self.push(v);
            }

            OpCode::BuildModule => self.exec_build_module(op)?,

            other => self.dispatch_generic(other, op, chunk, slots)?,
        }
        Ok(None)
    }

    fn dispatch_generic(&mut self, opcode: OpCode, operand: u16, chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        match opcode {
            OpCode::BitAnd | OpCode::BitOr | OpCode::BitXor
            | OpCode::BitNot | OpCode::Shl | OpCode::Shr => self.handle_bitwise(opcode)?,
            OpCode::In | OpCode::NotIn | OpCode::Is | OpCode::IsNot => self.handle_identity(opcode, chunk, slots)?,

            OpCode::BuildList | OpCode::BuildTuple | OpCode::BuildDict
            | OpCode::BuildString | OpCode::BuildSet | OpCode::BuildSlice => self.handle_build(opcode, operand)?,

            OpCode::StoreItem => { self.mark_impure(); self.store_item(chunk, slots)?; }
            OpCode::DelItem => { self.mark_impure(); self.del_item(chunk, slots)?; }
            OpCode::UnpackSequence | OpCode::UnpackEx | OpCode::FormatValue => self.handle_container(opcode, operand, chunk, slots)?,

            OpCode::ListAppend | OpCode::SetAdd | OpCode::MapAdd => self.handle_comprehension(opcode)?,
            OpCode::DictUpdate | OpCode::SetUpdate | OpCode::ListExtend => self.handle_spread_merge(opcode)?,

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
                self.handle_side(opcode, operand, chunk, slots)?;
            }
            OpCode::SetupExcept => {
                self.exception_stack.push(ExceptionFrame {
                    handler_ip: operand as usize,
                    stack_depth: self.stack.len(),
                    iter_depth: self.iter_stack.len(),
                    with_depth: self.with_stack.len(),
                });
            }
            OpCode::SetupWith => {
                let _ = operand;
                let cm = self.pop()?;
                // instance `__enter__` runs at setup; its return value feeds the `as` target.
                let bound = if let Some(r) = self.try_call_dunder(cm, "__enter__", &[], chunk, slots)? { r } else { cm };
                self.with_stack.push(cm);
                self.push(bound);
            }
            OpCode::ExitWith => {
                let _ = operand;
                let cm = self.with_stack.pop().ok_or(cold_runtime("ExitWith without matching SetupWith"))?;
                if let Some(&top) = self.stack.last() && top.0 == cm.0 { self.pop()?; }
                // normal-flow cleanup passes `(None, None, None)` to signal "no exception".
                if cm.is_heap() && matches!(self.heap.get(cm), HeapObj::Instance(..)) {
                    let n = Val::none();
                    let _ = self.try_call_dunder(cm, "__exit__", &[n, n, n], chunk, slots)?;
                }
            }
            OpCode::WithCleanup => {
                let _ = operand;
                // Reached when a `with` body raised: the SetupExcept unwind has pushed the synthesised exception. We consume it + the matching CM and dispatch `__exit__(type, exc, None)`; truthy return suppresses, falsy or absent re-raises with identity preserved via `pending.exc_val`.
                let exc = self.pop()?;
                let cm = self.with_stack.pop().ok_or(cold_runtime("WithCleanup without matching SetupWith"))?;
                let exc_name: String = if exc.is_heap() {
                    match self.heap.get(exc) {
                        HeapObj::ExcInstance(n, _) => n.clone(),
                        HeapObj::Instance(cls, _) => {
                            if cls.is_heap() && let HeapObj::Class(name, _, _) = self.heap.get(*cls) { name.clone() } else { "Exception".into() }
                        }
                        _ => "Exception".into(),
                    }
                } else { "Exception".into() };
                if cm.is_heap() && matches!(self.heap.get(cm), HeapObj::Instance(..)) {
                    let exc_type = self.heap.alloc(HeapObj::Type(exc_name.clone()))?;
                    let n = Val::none();
                    match self.try_call_dunder(cm, "__exit__", &[exc_type, exc, n], chunk, slots)? {
                        Some(r) if self.truthy(r) => {
                            // Suppressed: drop the pending exc identity so a later `raise` doesn't reuse it.
                            self.pending.exc_val = None;
                        }
                        _ => {
                            // Re-raise: preserve identity via `pending.exc_val` so an outer handler sees the same instance.
                            self.pending.exc_val = Some(exc);
                            return Err(VmErr::Raised(exc_name));
                        }
                    }
                } else {
                    // No `__exit__` (or non-instance CM): re-raise unconditionally.
                    self.pending.exc_val = Some(exc);
                    return Err(VmErr::Raised(exc_name));
                }
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
                        self.pending.kw_delta += n;
                    }
                    _ => return Err(cold_runtime("UnpackArgs: bad operand")),
                }
            }
            OpCode::PopExcept => { self.exception_stack.pop(); }
            // Emitted by `break` to drop the abandoned for-loop iterator.
            OpCode::PopIter => { self.iter_stack.pop(); }
            _ => return Err(cold_runtime("unexpected opcode in generic dispatch")),
        }
        Ok(())
    }

    /* Heavy arms extracted out of `dispatch` so wasm-opt can dedup prologues and the dispatcher itself stays compact. */

    #[inline(never)]
    fn exec_arith_or_compare(&mut self, opcode: OpCode, rip: usize, cache: &mut OpcodeCache, chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        // Scalar IC fast-path (Div/Pow/Minus skip, Float-only / overflow-prone).
        if !matches!(opcode, OpCode::Div | OpCode::Pow | OpCode::Minus)
            && let Some(fast) = cache.get_fast(rip)
        {
            match self.exec_fast(fast)? {
                FastOutcome::Done => return Ok(()),
                FastOutcome::Overflow => {}
                FastOutcome::TypeMiss => cache.invalidate(rip),
            }
        }
        // instance-dunder fast path.
        if let Some(inst) = cache.get_inst(rip) {
            match self.exec_inst(inst, chunk, slots)? {
                FastOutcome::Done => return Ok(()),
                FastOutcome::Overflow => {}
                FastOutcome::TypeMiss => cache.invalidate_inst(rip),
            }
        }
        if matches!(opcode, OpCode::Eq | OpCode::Lt | OpCode::NotEq | OpCode::Gt | OpCode::LtEq | OpCode::GtEq) {
            self.handle_compare(opcode, rip, cache, chunk, slots)
        } else {
            self.handle_arith(opcode, rip, cache, chunk, slots)
        }
    }

    /* Charge one unit against the op budget; for native loops (custom-iterator drain, generator collect) that bypass the dispatch back-edge counter. */
    #[inline]
    pub(crate) fn charge_step(&mut self) -> Result<(), VmErr> {
        if !self.sandbox_off {
            if self.budget == 0 { return Err(cold_budget()); }
            self.budget -= 1;
        }
        Ok(())
    }

    /* Charge `n` units at once; for native builtins (sort, materialise) whose cost scales with input size. */
    #[inline]
    pub(crate) fn charge_steps(&mut self, n: usize) -> Result<(), VmErr> {
        if !self.sandbox_off {
            if self.budget < n { self.budget = 0; return Err(cold_budget()); }
            self.budget -= n;
        }
        Ok(())
    }

    #[inline(never)]
    fn exec_for_iter(&mut self, op: u16, ip: &mut usize, n: usize, chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
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
            return Ok(());
        }
        // user-defined iterator calls `__next__`; `StopIteration` ends the loop without propagating, other exceptions surface.
        if let Some(IterFrame::UserDefined(iter_val)) = self.iter_stack.last() {
            let iter = *iter_val;
            match self.try_call_dunder(iter, "__next__", &[], chunk, slots) {
                Ok(Some(item)) => { self.push(item); }
                Ok(None) => {
                    self.iter_stack.pop();
                    if op as usize > n { return Err(cold_runtime("jump target out of bounds")); }
                    *ip = op as usize;
                }
                Err(VmErr::Raised(m)) if m == "StopIteration" || m.starts_with("StopIteration:") => {
                    self.iter_stack.pop();
                    if op as usize > n { return Err(cold_runtime("jump target out of bounds")); }
                    *ip = op as usize;
                }
                Err(e) => return Err(e),
            }
            return Ok(());
        }
        match self.iter_stack.last_mut().and_then(|f| f.next_item()) {
            Some(item) => self.push(item),
            None => {
                self.iter_stack.pop();
                if op as usize > n { return Err(cold_runtime("jump target out of bounds")); }
                *ip = op as usize;
            }
        }
        Ok(())
    }

    #[inline(never)]
    fn exec_make_class(&mut self, op: u16, ip: usize, cache: &OpcodeCache, chunk: &SSAChunk, caller_slots: &[Val]) -> Result<(), VmErr> {
        // Operand layout mirrors `class_def_with`: low byte = class chunk index, high byte = base count.
        let class_idx = (op & 0xFF) as usize;
        let num_bases = (op >> 8) as usize;
        // Pop bases first so a misencoded operand fails before we touch the body.
        let bases = self.pop_n(num_bases)?;
        for &b in &bases {
            if !b.is_heap() || !matches!(self.heap.get(b), HeapObj::Class(..)) {
                return Err(cold_type("base class must be a class object"));
            }
        }
        let Some(body) = chunk.classes.get(class_idx) else {
            return Err(cold_runtime("class index out of range"));
        };
        let mut class_slots = self.fill_builtins(&body.names);
        // Pin caller slots as GC roots so a nested class/function body can't sweep them.
        let snap = self.live_slots.len();
        self.live_slots.extend_from_slice(caller_slots);
        let exec_result = self.exec(body, &mut class_slots);
        self.live_slots.truncate(snap);
        exec_result?;
        let mut methods: Vec<(String, Val)> = Vec::new();
        for (i, name) in body.names.iter().enumerate() {
            if let Some(&v) = class_slots.get(i)
                && !v.is_undef() {
                    let base = ssa_strip(name);
                    let is_builtin_shadow = v.is_heap()
                        && matches!(self.heap.get(v), HeapObj::NativeFn(_))
                        && self.globals.get(base).copied() == Some(v);
                    if is_builtin_shadow { continue; }
                    if let Some(pos) = methods.iter().position(|(n, _)| n == base) {
                        methods[pos].1 = v;
                    } else {
                        methods.push((base.to_string(), v));
                    }
                }
        }
        let next_op = cache.fused_ref().get(ip).map(|i| i.operand).unwrap_or(0);
        let name_str = chunk.names.get(next_op as usize).map(|n| ssa_strip(n)).unwrap_or("?").to_string();
        let cls = self.heap.alloc(HeapObj::Class(name_str, bases, methods))?;
        self.push(cls);
        Ok(())
    }

    #[inline(never)]
    fn exec_store_attr(&mut self, op: u16, chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        let value = self.pop()?;
        let obj = self.pop()?;
        if !obj.is_heap() { return Err(cold_type("cannot set attribute")); }
        let name = chunk.names.get(op as usize).ok_or(cold_runtime("StoreAttr: bad name index"))?.clone();
        if let HeapObj::Instance(cls_val, _) = self.heap.get(obj) {
            let cls_val = *cls_val;
            if let Some((member, _)) = self.lookup_class_member(cls_val, ssa_strip(&name))
                && let HeapObj::Property(_, setter) = self.heap.get(member) {
                let setter = *setter;
                if setter.is_none() {
                    return Err(VmErr::Attribute(s!("can't set attribute '", str ssa_strip(&name), "'")));
                }
                if self.depth >= self.max_calls { return Err(cold_depth()); }
                self.push(setter);
                self.push(obj);
                self.push(value);
                self.exec_call(2, chunk, slots)?;
                self.pop()?;
                return Ok(());
            }
        }
        let key = self.heap.alloc(HeapObj::Str(name))?;
        match self.heap.get_mut(obj) {
            HeapObj::Instance(_, attrs) => {
                attrs.borrow_mut().insert(key, value);
            }
            _ => return Err(cold_type("cannot set attribute on this type")),
        }
        Ok(())
    }

    #[inline(never)]
    fn exec_build_module(&mut self, op: u16) -> Result<(), VmErr> {
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
        Ok(())
    }
}
