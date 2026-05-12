/* 
Dunder dispatch protocol: probe an instance method, invoke with `self` prepended, treat `NotImplemented` as a miss so reflected ops / generic fallbacks take over. 
*/

use super::*;
use super::methods::AttrLookup;
use crate::alloc::string::ToString;

impl<'a> VM<'a> {
    /* `recv.<name>(*args)`: `Some(v)` on return, `None` on miss / `NotImplemented`, `Err` only on a raised dunder. */
    pub(crate) fn try_call_dunder(&mut self, recv: Val, name: &str, args: &[Val], chunk: &SSAChunk, slots: &mut [Val]) -> Result<Option<Val>, VmErr> {
        // Built-in types route through their native handlers; dunder dispatch only fires on user instances.
        if !recv.is_heap() { return Ok(None); }
        if !matches!(self.heap.get(recv), HeapObj::Instance(..)) { return Ok(None); }

        let Some(AttrLookup::InstanceMethod { recv, func, class }) = self.resolve_attr_silent(recv, name)? else { return Ok(None); };

        // Mirror `__init__` dispatch: depth guard before pushing so a recursive blow-up leaves no half-built frame.
        if self.depth >= self.max_calls { return Err(cold_depth()); }

        self.pending.method_binding = Some((class, recv));
        self.push(func);
        self.push(recv);
        for &a in args { self.push(a); }
        let argc = (1 + args.len()) as u16;
        self.exec_call(argc, chunk, slots)?;

        let result = self.pop()?;
        if self.heap.is_not_implemented(result) { return Ok(None); }
        Ok(Some(result))
    }

    /* Class of an Instance, or `None` for built-in operands; powers the subclass-first ordering rule. */
    fn instance_class(&self, v: Val) -> Option<Val> {
        if !v.is_heap() { return None; }
        match self.heap.get(v) { HeapObj::Instance(c, _) => Some(*c), _ => None }
    }

    /* Binary arithmetic dunder dispatch with Python's subclass-first ordering: if `type(b)` is a strict subclass of `type(a)`, the reflected op runs first so overrides win. */
    pub(crate) fn try_binary_dunder(&mut self, op: OpCode, a: Val, b: Val, chunk: &SSAChunk, slots: &mut [Val]) -> Result<Option<Val>, VmErr> {
        let a_cls = self.instance_class(a);
        let b_cls = self.instance_class(b);
        if a_cls.is_none() && b_cls.is_none() { return Ok(None); }

        let (lname, rname) = match op {
            OpCode::Add => ("__add__", "__radd__"),
            OpCode::Sub => ("__sub__", "__rsub__"),
            OpCode::Mul => ("__mul__", "__rmul__"),
            OpCode::Div => ("__truediv__", "__rtruediv__"),
            OpCode::FloorDiv => ("__floordiv__", "__rfloordiv__"),
            OpCode::Mod => ("__mod__", "__rmod__"),
            OpCode::Pow => ("__pow__", "__rpow__"),
            _ => return Ok(None),
        };

        let b_overrides = match (a_cls, b_cls) {
            (Some(ac), Some(bc)) => ac.0 != bc.0 && self.heap.is_subclass(bc, ac),
            _ => false,
        };

        if b_overrides {
            if let Some(r) = self.try_call_dunder(b, rname, &[a], chunk, slots)? { return Ok(Some(r)); }
            if let Some(r) = self.try_call_dunder(a, lname, &[b], chunk, slots)? { return Ok(Some(r)); }
        } else {
            if let Some(r) = self.try_call_dunder(a, lname, &[b], chunk, slots)? { return Ok(Some(r)); }
            if let Some(r) = self.try_call_dunder(b, rname, &[a], chunk, slots)? { return Ok(Some(r)); }
        }
        Ok(None)
    }

    /* Comparison dunder dispatch. `__eq__` reflects to itself; `__ne__` falls back to `not __eq__`; `<` reflects to `>` and vice-versa. */
    pub(crate) fn try_compare_dunder(&mut self, op: OpCode, a: Val, b: Val, chunk: &SSAChunk, slots: &mut [Val]) -> Result<Option<bool>, VmErr> {
        let a_cls = self.instance_class(a);
        let b_cls = self.instance_class(b);
        if a_cls.is_none() && b_cls.is_none() { return Ok(None); }

        let (lname, rname, negate) = match op {
            OpCode::Eq => ("__eq__", "__eq__", false),
            OpCode::NotEq => ("__eq__", "__eq__", true),
            OpCode::Lt => ("__lt__", "__gt__", false),
            OpCode::LtEq => ("__le__", "__ge__", false),
            OpCode::Gt => ("__gt__", "__lt__", false),
            OpCode::GtEq => ("__ge__", "__le__", false),
            _ => return Ok(None),
        };

        let b_overrides = match (a_cls, b_cls) {
            (Some(ac), Some(bc)) => ac.0 != bc.0 && self.heap.is_subclass(bc, ac),
            _ => false,
        };

        let raw = if b_overrides {
            match self.try_call_dunder(b, rname, &[a], chunk, slots)? {
                Some(r) => Some(r),
                None => self.try_call_dunder(a, lname, &[b], chunk, slots)?,
            }
        } else {
            match self.try_call_dunder(a, lname, &[b], chunk, slots)? {
                Some(r) => Some(r),
                None => self.try_call_dunder(b, rname, &[a], chunk, slots)?,
            }
        };

        let Some(r) = raw else { return Ok(None); };
        let truthy = self.truthy(r);
        Ok(Some(if negate { !truthy } else { truthy }))
    }

    /* Python `bool()` semantics: try `__bool__`, then `__len__` (0 = False), else default True for instances. Pass-through for built-in types. */
    pub(crate) fn truthy_op(&mut self, v: Val, chunk: &SSAChunk, slots: &mut [Val]) -> Result<bool, VmErr> {
        if !v.is_heap() || !matches!(self.heap.get(v), HeapObj::Instance(..)) {
            return Ok(self.truthy(v));
        }
        if let Some(r) = self.try_call_dunder(v, "__bool__", &[], chunk, slots)? {
            if !matches!(r, x if x.is_bool()) {
                return Err(cold_type("__bool__ should return bool"));
            }
            return Ok(r.as_bool());
        }
        if let Some(r) = self.try_call_dunder(v, "__len__", &[], chunk, slots)? {
            return self.len_to_bool(r);
        }
        Ok(true)
    }

    /* `in` operator: prefer the container's `__contains__`; for built-in sequences with an instance item, iterate using `__eq__` so user equality is honoured. */
    pub(crate) fn contains_op(&mut self, container: Val, item: Val, chunk: &SSAChunk, slots: &mut [Val]) -> Result<bool, VmErr> {
        if let Some(r) = self.try_call_dunder(container, "__contains__", &[item], chunk, slots)? {
            return Ok(self.truthy(r));
        }

        let item_is_instance = item.is_heap() && matches!(self.heap.get(item), HeapObj::Instance(..));

        // Built-in sequence container + instance item: walk and compare with `__eq__` so user equality wins over pointer eq.
        if item_is_instance && container.is_heap() {
            let items: Option<Vec<Val>> = match self.heap.get(container) {
                HeapObj::List(v) => Some(v.borrow().clone()),
                HeapObj::Tuple(v) => Some(v.clone()),
                HeapObj::Set(s) => Some(s.borrow().iter().copied().collect()),
                HeapObj::FrozenSet(s) => Some(s.iter().copied().collect()),
                _ => None,
            };
            if let Some(items) = items {
                for x in items {
                    if self.eq_op(item, x, chunk, slots)? { return Ok(true); }
                }
                return Ok(false);
            }
        }

        // User instance container with `__iter__`: walk via the iterator protocol, comparing items with `__eq__`.
        if container.is_heap() && matches!(self.heap.get(container), HeapObj::Instance(..))
            && let Some(iter) = self.try_call_dunder(container, "__iter__", &[], chunk, slots)? {
            loop {
                match self.try_call_dunder(iter, "__next__", &[], chunk, slots) {
                    Ok(Some(v)) => {
                        if self.eq_op(item, v, chunk, slots)? { return Ok(true); }
                    }
                    Ok(None) => return Ok(false),
                    Err(VmErr::Raised(ref m)) if m == "StopIteration" => return Ok(false),
                    Err(e) => return Err(e),
                }
            }
        }

        Ok(self.contains(container, item))
    }

    /* `==` with dunder dispatch and pointer-eq fallback; used wherever `contains_op` walks a sequence. */
    pub(crate) fn eq_op(&mut self, a: Val, b: Val, chunk: &SSAChunk, slots: &mut [Val]) -> Result<bool, VmErr> {
        if let Some(r) = self.try_compare_dunder(OpCode::Eq, a, b, chunk, slots)? { return Ok(r); }
        Ok(eq_vals_with_heap(a, b, &self.heap))
    }

    /* Drive a user-defined iterator to a Vec; treats a missing or non-Instance receiver as "no protocol" by returning `None`. Used by `list(custom)`, `tuple(custom)`, etc. */
    pub(crate) fn iter_to_vec_op(&mut self, obj: Val, chunk: &SSAChunk, slots: &mut [Val]) -> Result<Option<Vec<Val>>, VmErr> {
        if !obj.is_heap() || !matches!(self.heap.get(obj), HeapObj::Instance(..)) { return Ok(None); }
        let Some(iter) = self.try_call_dunder(obj, "__iter__", &[], chunk, slots)? else { return Ok(None); };
        let mut out = Vec::new();
        loop {
            match self.try_call_dunder(iter, "__next__", &[], chunk, slots) {
                Ok(Some(v)) => out.push(v),
                Ok(None) => return Ok(Some(out)),
                Err(VmErr::Raised(ref m)) if m == "StopIteration" => return Ok(Some(out)),
                Err(e) => return Err(e),
            }
        }
    }

    /* `str(v)` semantics: instance `__str__` wins, then `__repr__`, else the built-in display. */
    pub(crate) fn display_op(&mut self, v: Val, chunk: &SSAChunk, slots: &mut [Val]) -> Result<String, VmErr> {
        if v.is_heap() && matches!(self.heap.get(v), HeapObj::Instance(..)) {
            if let Some(r) = self.try_call_dunder(v, "__str__", &[], chunk, slots)? {
                return self.require_str(r, "__str__");
            }
            if let Some(r) = self.try_call_dunder(v, "__repr__", &[], chunk, slots)? {
                return self.require_str(r, "__repr__");
            }
        }
        Ok(self.display(v))
    }

    /* `repr(v)` semantics: instance `__repr__` wins; otherwise the built-in repr (which adds quotes for strings, etc.). */
    pub(crate) fn repr_op(&mut self, v: Val, chunk: &SSAChunk, slots: &mut [Val]) -> Result<String, VmErr> {
        if v.is_heap() && matches!(self.heap.get(v), HeapObj::Instance(..))
            && let Some(r) = self.try_call_dunder(v, "__repr__", &[], chunk, slots)? {
            return self.require_str(r, "__repr__");
        }
        Ok(self.repr(v))
    }

    fn require_str(&self, v: Val, name: &str) -> Result<String, VmErr> {
        if v.is_heap() && let HeapObj::Str(s) = self.heap.get(v) { return Ok(s.clone()); }
        Err(VmErr::TypeMsg(crate::s!("'", str name, "' did not return a string")))
    }

    /* `format(v, spec)` dispatch: instance `__format__(spec)` wins; otherwise the built-in spec engine runs. Empty spec on an instance still goes through `__format__` so user formatting can opt in. */
    pub(crate) fn format_op(&mut self, v: Val, spec: &str, chunk: &SSAChunk, slots: &mut [Val]) -> Result<String, VmErr> {
        if v.is_heap() && matches!(self.heap.get(v), HeapObj::Instance(..)) {
            let spec_val = self.heap.alloc(HeapObj::Str(spec.to_string()))?;
            if let Some(r) = self.try_call_dunder(v, "__format__", &[spec_val], chunk, slots)? {
                return self.require_str(r, "__format__");
            }
        }
        super::format::format_value(v, spec, &self.heap).map_err(cold_value)
    }

    /* Coerce a `__len__` / `__length_hint__` return value to bool semantics; rejects negatives. */
    fn len_to_bool(&self, v: Val) -> Result<bool, VmErr> {
        let n = if v.is_int() { v.as_int() as i128 }
        else if let Some(i) = crate::modules::vm::types::as_i128(v, &self.heap) { i }
        else { return Err(cold_type("__len__ must return int")); };
        if n < 0 { return Err(cold_value("__len__() should return >= 0")); }
        Ok(n != 0)
    }
}
