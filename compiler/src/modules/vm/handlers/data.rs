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

    /* Indexed access/store, unpacking, and `{value!s:spec}` formatting. */
    pub(crate) fn handle_container(&mut self, op: OpCode, operand: u16) -> Result<(), VmErr> {
        match op {
            OpCode::GetItem => { self.get_item()?; }
            OpCode::StoreItem => {
                self.mark_impure();
                self.store_item()?;
            }
            OpCode::DelItem => {
                self.mark_impure();
                self.del_item()?;
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
            _ => return Err(cold_runtime("non-container opcode in handle_container")),
        }
        Ok(())
    }

    /* Append/add to the comprehension accumulator at the top of the stack. */
    pub(crate) fn handle_comprehension(&mut self, op: OpCode) -> Result<(), VmErr> {
        let (kind, value, key) = match op {
            OpCode::ListAppend => ("list",  self.pop()?, None),
            OpCode::SetAdd     => ("set",   self.pop()?, None),
            OpCode::MapAdd     => { let v = self.pop()?; let k = self.pop()?; ("dict", v, Some(k)) }
            _ => return Err(cold_runtime("non-comprehension opcode in handle_comprehension")),
        };
        let acc = *self.stack.last().ok_or(VmErr::Runtime("stack underflow"))?;
        let corrupt = || VmErr::Runtime(match kind {
            "list" => "list accumulator corrupted",
            "set"  => "set accumulator corrupted",
            _      => "dict accumulator corrupted",
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

    /* Side-effecting / impure ops: assert, del, global/nonlocal, import,
       type alias, raise, await, yield-from. */
    pub(crate) fn handle_side(&mut self, op: OpCode, operand: u16, slots: &mut [Val]) -> Result<(), VmErr> {
        match op {
            OpCode::Assert => {
                let v = self.pop()?;
                if !self.truthy(v) { return Err(VmErr::Runtime("AssertionError")); }
            }
            OpCode::Del => {
                let slot = operand as usize;
                if slot < slots.len() { slots[slot] = Val::undef(); }
            }
            OpCode::Global | OpCode::Nonlocal => self.mark_impure(),
            OpCode::TypeAlias => { self.pop()?; }
            OpCode::Raise | OpCode::RaiseFrom => {
                self.mark_impure();
                let exc = self.pop()?;
                // For Type values, carry the bare class name so `except X`
                // can match it via the global type lookup in the dispatch
                // exception path. Otherwise fall back to display().
                let msg = if exc.is_heap() && let HeapObj::Type(n) = self.heap.get(exc) {
                    n.clone()
                } else {
                    self.display(exc)
                };
                return Err(VmErr::Raised(msg));
            }
            OpCode::Await => {
                // Awaiting a coroutine resumes it; its yield (if any)
                // propagates up via `self.yielded` (set by resume_coroutine).
                // Sync values pass through unchanged.
                let val = self.pop()?;
                if val.is_heap() && matches!(self.heap.get(val), HeapObj::Coroutine(..)) {
                    self.push(val);
                    let result = self.resume_coroutine(val)?;
                    self.push(result);
                } else {
                    self.push(val);
                }
            }
            OpCode::YieldFrom => {}
            _ => return Err(cold_runtime("non-side opcode in handle_side")),
        }
        Ok(())
    }
}