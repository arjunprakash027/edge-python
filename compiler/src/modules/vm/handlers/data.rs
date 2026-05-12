use super::*;

impl<'a> VM<'a> {

    /* StoreName: single SSA slot write after register coalescing. */
    pub(crate) fn handle_store(&mut self, operand: u16, slots: &mut [Val]) -> Result<(), VmErr> {
        let v = self.pop()?;
        slots[operand as usize] = v;
        Ok(())
    }

    /* Container constructors: list / tuple / dict / set / slice / string. */
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
                for pair in flat.chunks(2) { self.require_hashable(pair[0])?; }
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
            _ => return Err(cold_runtime("non-build opcode in handle_build")),
        }
        Ok(())
    }

    /* Indexed access/store, unpacking, and `{value!s:spec}` formatting. `GetItem`/`StoreItem`/`DelItem` are dispatched directly from the hot loop; the arms below cover legacy callers that may route through here. */
    pub(crate) fn handle_container(&mut self, op: OpCode, operand: u16, chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        match op {
            OpCode::StoreItem => {
                self.mark_impure();
                self.store_item(chunk, slots)?;
            }
            OpCode::DelItem => {
                self.mark_impure();
                self.del_item(chunk, slots)?;
            }
            OpCode::UnpackSequence => self.exec_unpack_seq(operand as usize)?,
            OpCode::UnpackEx => self.unpack_ex(operand)?,
            OpCode::FormatValue => {
                /* Operand layout: bit 0 has_spec, bits 1..=2 conversion (0 none, 1 !r, 2 !s, 3 !a). See parser/literals.rs. */
                let has_spec = (operand & 1) != 0;
                let conv = (operand >> 1) & 0b11;
                let spec_val = if has_spec { Some(self.pop()?) } else { None };
                let v = self.pop()?;

                // F2.8: conversion flags consult the dunder-aware helpers so `f"{x!s}"` honours `__str__`.
                let converted = match conv {
                    1 => { let s = self.repr_op(v, chunk, slots)?; self.heap.alloc(HeapObj::Str(s))? }
                    2 => { let s = self.display_op(v, chunk, slots)?; self.heap.alloc(HeapObj::Str(s))? }
                    3 => self.heap.alloc(HeapObj::Str(super::format::display_inline(v, &self.heap).escape_default().collect::<String>()))?,
                    _ => v,
                };

                let result = match spec_val {
                    Some(sv) => {
                        let spec = match self.heap.get(sv) {
                            HeapObj::Str(s) => s.clone(),
                            _ => return Err(cold_type("format spec must be a string")),
                        };
                        // F2.11: instance `__format__(spec)` runs through `format_op`; built-ins fall through to the spec engine.
                        self.format_op(converted, &spec, chunk, slots)?
                    }
                    None => {
                        if conv != 0 && let HeapObj::Str(s) = self.heap.get(converted) {
                            s.clone()
                        } else {
                            self.display_op(converted, chunk, slots)?
                        }
                    }
                };
                let val = self.heap.alloc(HeapObj::Str(result))?;
                self.push(val);
            }
            _ => return Err(cold_runtime("non-container opcode in handle_container")),
        }
        Ok(())
    }

    /* Append/add to the comprehension accumulator at the top of the stack. */
    pub(crate) fn handle_comprehension(&mut self, op: OpCode) -> Result<(), VmErr> {
        let (kind, value, key) = match op {
            OpCode::ListAppend => ("list",  self.pop()?, None),
            OpCode::SetAdd => ("set", self.pop()?, None),
            OpCode::MapAdd => { let v = self.pop()?; let k = self.pop()?; ("dict", v, Some(k)) }
            _ => return Err(cold_runtime("non-comprehension opcode in handle_comprehension")),
        };
        let acc = *self.stack.last().ok_or(VmErr::Runtime("stack underflow"))?;
        let corrupt = || VmErr::Runtime(match kind {
            "list" => "list accumulator corrupted",
            "set" => "set accumulator corrupted",
            _ => "dict accumulator corrupted",
        });
        if !acc.is_heap() { return Err(corrupt()); }
        match (kind, self.heap.get(acc)) {
            ("list", HeapObj::List(rc)) => { rc.borrow_mut().push(value); }
            ("set",  HeapObj::Set(rc))  => {
                let already = rc.borrow().iter().any(|&x| eq_vals_with_heap(x, value, &self.heap));
                if !already && let HeapObj::Set(rc) = self.heap.get(acc) {
                    rc.borrow_mut().insert(value);
                }
            }
            ("dict", HeapObj::Dict(rc)) => { rc.borrow_mut().insert(key.unwrap(), value); }
            _ => return Err(corrupt()),
        }
        Ok(())
    }

    /* Yield: keep the value on the stack and flag the executor to suspend. */
    pub(crate) fn handle_yield(&mut self) -> Result<(), VmErr> {
        let v = self.pop()?;
        self.push(v);
        self.yielded = true;
        Ok(())
    }

    /* Side-effecting / impure ops: assert, del, global/nonlocal, import, type alias, raise, await. */
    pub(crate) fn handle_side(&mut self, op: OpCode, operand: u16, chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        match op {
            OpCode::Assert => {
                let v = self.pop()?;
                if !self.truthy_op(v, chunk, slots)? { return Err(VmErr::Runtime("AssertionError")); }
            }
            OpCode::Del => {
                let slot = operand as usize;
                if slot < slots.len() { slots[slot] = Val::undef(); }
            }
            OpCode::Global | OpCode::Nonlocal => self.mark_impure(),
            OpCode::Raise | OpCode::RaiseFrom => {
                self.mark_impure();
                // RaiseFrom emits both `expr` then `from expr` — the topmost value is the cause, but the exception to raise is the LHS.
                if op == OpCode::RaiseFrom { let _cause = self.pop()?; }
                let exc = self.pop()?;
                // Stash the Val for `except as e` binding; non-Exc values use `display()`.
                self.pending.exc_val = None;
                let msg = if exc.is_heap() {
                    match self.heap.get(exc) {
                        HeapObj::ExcInstance(n, _) => {
                            let n = n.clone();
                            self.pending.exc_val = Some(exc);
                            n
                        }
                        HeapObj::Type(n) => {
                            // Bare `raise X`: build empty ExcInstance so `e.args` is `()`.
                            let n = n.clone();
                            let inst = self.heap.alloc(
                                HeapObj::ExcInstance(n.clone(), Vec::new()))?;
                            self.pending.exc_val = Some(inst);
                            n
                        }
                        _ => self.display(exc),
                    }
                } else {
                    self.display(exc)
                };
                return Err(VmErr::Raised(msg));
            }
            OpCode::Await => {
                // Coroutine: resume it (yield propagates via `self.yielded`); sync values pass through.
                let val = self.pop()?;
                if val.is_heap() && matches!(self.heap.get(val), HeapObj::Coroutine(..)) {
                    self.push(val);
                    let result = self.resume_coroutine(val)?;
                    self.push(result);
                } else {
                    self.push(val);
                }
            }
            _ => return Err(cold_runtime("non-side opcode in handle_side")),
        }
        Ok(())
    }
}
