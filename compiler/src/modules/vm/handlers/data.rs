// vm/handlers/data.rs

use super::*;

impl<'a> VM<'a> {

    /* StoreName con back-propagación SSA a versiones previas. */
    
    pub(crate) fn handle_store(&mut self, operand: u16, slots: &mut [Option<Val>]) -> Result<(), VmErr> {
        let v = self.pop()?;
        slots[operand as usize] = Some(v);
        Ok(())
    }

    /* Container constructors: list/tuple/dict/set/slice/string. */

    pub(crate) fn handle_build(&mut self, op: OpCode, operand: u16) -> Result<(), VmErr> {
        match op {
            OpCode::BuildList => {
                let v = self.pop_n(operand as usize)?;
                let val = self.heap.alloc(HeapObj::List(Rc::new(RefCell::new(v))))?;
                self.push(val);
            }
            OpCode::BuildTuple => {
                let v = self.pop_n(operand as usize)?;
                let val = self.heap.alloc(HeapObj::Tuple(v))?;
                self.push(val);
            }
            OpCode::BuildDict => {
                let flat = self.pop_n(operand as usize * 2)?;
                let dm = DictMap::from_pairs(flat.chunks(2).map(|c| (c[0], c[1])).collect());
                let val = self.heap.alloc(HeapObj::Dict(Rc::new(RefCell::new(dm))))?;
                self.push(val);
            }
            OpCode::BuildString => {
                let parts = self.pop_n(operand as usize)?;
                let s: String = parts.iter().map(|v| self.display(*v)).collect();
                let val = self.heap.alloc(HeapObj::Str(s))?;
                self.push(val);
            }
            OpCode::BuildSet   => self.build_set(operand)?,
            OpCode::BuildSlice => self.build_slice(operand)?,
            _ => unreachable!("non-build opcode in handle_build"),
        }
        Ok(())
    }

    /* Indexed access/allocation, unpacking, and value formatting. */

    pub(crate) fn handle_container(&mut self, op: OpCode, operand: u16) -> Result<(), VmErr> {
        match op {
            OpCode::GetItem => { self.get_item()?; }
            OpCode::StoreItem => {
                self.mark_impure();
                self.store_item()?;
            }
            OpCode::UnpackSequence => self.exec_unpack_seq(operand as usize)?,
            OpCode::UnpackEx => self.unpack_ex(operand)?,
            OpCode::FormatValue => {
                if operand == 1 { self.pop()?; }
                let v = self.pop()?;
                let s = self.display(v);
                let val = self.heap.alloc(HeapObj::Str(s))?;
                self.push(val);
            }
            _ => unreachable!("non-container opcode in handle_container"),
        }
        Ok(())
    }

    /* Append/add to accumulators at the top of the stack during comprehensions. */

    pub(crate) fn handle_comprehension(&mut self, op: OpCode) -> Result<(), VmErr> {
        match op {
            OpCode::ListAppend => {
                let v = self.pop()?;
                let acc = *self.stack.last().ok_or(VmErr::Runtime("stack underflow"))?;
                if !acc.is_heap() { return Err(VmErr::Runtime("list accumulator corrupted")); }
                match self.heap.get(acc) {
                    HeapObj::List(rc) => rc.borrow_mut().push(v),
                    _ => return Err(VmErr::Runtime("list accumulator corrupted")),
                }
            }
            OpCode::SetAdd => {
                let v = self.pop()?;
                let acc = *self.stack.last().ok_or(VmErr::Runtime("stack underflow"))?;
                if !acc.is_heap() { return Err(VmErr::Runtime("set accumulator corrupted")); }
                let already = match self.heap.get(acc) {
                    HeapObj::Set(rc) => rc.borrow().iter().any(|&x| eq_vals_with_heap(x, v, &self.heap)),
                    _ => return Err(VmErr::Runtime("set accumulator corrupted")),
                };
                if !already && let HeapObj::Set(rc) = self.heap.get(acc) {
                    rc.borrow_mut().insert(v);
                }
            }
            OpCode::MapAdd => {
                let value = self.pop()?;
                let key = self.pop()?;
                let acc = *self.stack.last().ok_or(VmErr::Runtime("stack underflow"))?;
                if !acc.is_heap() { return Err(VmErr::Runtime("dict accumulator corrupted")); }
                match self.heap.get(acc) {
                    HeapObj::Dict(rc) => { rc.borrow_mut().insert(key, value); }
                    _ => return Err(VmErr::Runtime("dict accumulator corrupted")),
                }
            }
            _ => unreachable!("non-comprehension opcode in handle_comprehension"),
        }
        Ok(())
    }

    /* Accumulates value in the generator buffer and pushes None as a placeholder. */

    pub(crate) fn handle_yield(&mut self) -> Result<(), VmErr> {
        let v = self.pop()?;
        self.push(v);
        self.yielded = true;
        Ok(())
    }

    /* Side-effects and impurities: assert, del, global/nonlocal, import, type aliases, exception handling stubs and await/yield-from. */
    
    pub(crate) fn handle_side(&mut self, op: OpCode, operand: u16, slots: &mut [Option<Val>]) -> Result<(), VmErr> {
        match op {
            OpCode::Assert => {
                let v = self.pop()?;
                if !self.truthy(v) { return Err(VmErr::Runtime("AssertionError")); }
            }
            OpCode::Del => {
                let slot = operand as usize;
                if slot < slots.len() { slots[slot] = None; }
            }
            OpCode::Global | OpCode::Nonlocal => self.mark_impure(),
            OpCode::TypeAlias => { self.pop()?; }
            OpCode::Import | OpCode::ImportFrom => {
                return Err(VmErr::Runtime("imports are not supported"));
            }
            OpCode::Raise | OpCode::RaiseFrom => {
                self.mark_impure();
                let exc = self.pop()?;
                let msg = self.display(exc);
                return Err(VmErr::Raised(msg));
            }
            OpCode::Await => {
                // If awaiting a coroutine, run it to completion or yield
                let val = self.pop()?;
                if val.is_heap() && matches!(self.heap.get(val), HeapObj::Coroutine(..)) {
                    // Resume the inner coroutine
                    self.push(val);
                    // Use Call dispatch with 0 args - callee is on stack
                    let callee = val;
                    if let HeapObj::Coroutine(ip, saved_slots, saved_stack, fi, saved_iters) = self.heap.get(callee) {
                        let (ip, fi) = (*ip, *fi);
                        let mut fn_slots = saved_slots.clone();
                        let saved_stack_len = self.stack.len();
                        let saved_iter_len = self.iter_stack.len();
                        self.stack.extend_from_slice(&saved_stack.clone());
                        self.iter_stack.extend(saved_iters.clone());
                        let saved_yielded = self.yielded;
                        self.yielded = false;
                        self.depth += 1;
                        let (_, body, _, _) = self.functions[fi];
                        let result = self.exec_from(body, &mut fn_slots, ip);
                        self.depth -= 1;
                        let result = result?;
                        if self.yielded {
                            // Inner coroutine yielded - propagate yield upward
                            self.yielded = false;
                            let resume_ip = self.resume_ip;
                            let remaining = self.stack.split_off(saved_stack_len);
                            let coro_iters: Vec<super::super::types::IterFrame> = self.iter_stack.drain(saved_iter_len..).collect();
                            if let HeapObj::Coroutine(sip, ss, sst, _, si) = self.heap.get_mut(callee) {
                                *sip = resume_ip;
                                *ss = fn_slots;
                                *sst = remaining;
                                *si = coro_iters;
                            }
                            // Propagate: yield the value from this coroutine too
                            self.push(result);
                            self.yielded = true;
                        } else {
                            // Inner coroutine finished - push its return value
                            self.stack.truncate(saved_stack_len);
                            self.iter_stack.truncate(saved_iter_len);
                            self.yielded = saved_yielded;
                            self.push(result);
                        }
                    } else {
                        // Not a coroutine anymore (shouldn't happen)
                        self.push(val);
                    }
                } else {
                    // Not a coroutine - just push the value (sync call already resolved)
                    self.push(val);
                }
            }
            OpCode::YieldFrom => {}
            _ => unreachable!("non-side opcode in handle_side"),
        }
        Ok(())
    }
}