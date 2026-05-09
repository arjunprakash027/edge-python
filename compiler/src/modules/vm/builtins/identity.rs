use alloc::{string::String, vec::Vec};

use super::super::VM;
use super::super::types::*;
use super::matches_exc_class;

impl<'a> VM<'a> {

    pub fn call_repr(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        self.alloc_and_push_str(self.repr(o))
    }

    pub fn call_callable(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let result = if o.is_heap() {
            matches!(self.heap.get(o),
                HeapObj::Func(..) | HeapObj::BoundMethod(..)
                | HeapObj::Type(_) | HeapObj::NativeFn(_))
        } else { false };
        self.push(Val::bool(result));
        Ok(())
    }

    pub fn call_id(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        // Use the NaN-boxed bit pattern as identity. Truncate to fit INT_MAX.
        let id = ((o.0 as i64).abs()) & Val::INT_MAX;
        self.push(Val::int(id));
        Ok(())
    }

    pub fn call_hash(&mut self) -> Result<(), VmErr> {
        use core::hash::{Hash, Hasher};
        let o = self.pop()?;
        let mut h = crate::modules::fx::FxHasher::default();
        if o.is_int()        { o.as_int().hash(&mut h); }
        else if o.is_float() { o.as_float().to_bits().hash(&mut h); }
        else if o.is_bool()  { o.as_bool().hash(&mut h); }
        else if o.is_none()  { 0u64.hash(&mut h); }
        else if o.is_heap() {
            match self.heap.get(o) {
                HeapObj::Str(s) => s.hash(&mut h),
                HeapObj::Bytes(b) => b.hash(&mut h),
                HeapObj::Tuple(items) => {
                    for v in items { v.0.hash(&mut h); }
                }
                HeapObj::List(_) | HeapObj::Dict(_) | HeapObj::Set(_) => {
                    return Err(cold_type("unhashable type"));
                }
                _ => o.0.hash(&mut h),
            }
        }
        self.push(Val::int(h.finish() as i64 & Val::INT_MAX));
        Ok(())
    }

    /* Type-name based isinstance check. Accepts Type or NativeFn (for the
       builtins-as-types case) on the right; allows int↔bool aliasing. */
    pub fn call_isinstance(&mut self) -> Result<(), VmErr> {
        let (arg2, obj) = (self.pop()?, self.pop()?);
        let obj_ty = self.type_name(obj);

        // For exception matching: when `obj` is a Type itself or an
        // ExcInstance, compare names against the asserted type.
        let obj_type_name: Option<String> = if obj.is_heap() {
            match self.heap.get(obj) {
                HeapObj::Type(n) => Some(n.clone()),
                HeapObj::ExcInstance(n, _) => Some(n.clone()),
                _ => None,
            }
        } else { None };

        let check_one = |t: Val, heap: &HeapPool| -> Result<bool, VmErr> {
            if !t.is_heap() {
                return Err(VmErr::Type("isinstance() arg 2 must be a type or tuple of types"));
            }
            let exc_match = |name: &str| -> bool {
                obj_type_name.as_deref()
                    .map(|n| matches_exc_class(n, name))
                    .unwrap_or(false)
            };
            match heap.get(t) {
                HeapObj::Type(name) => Ok(
                    matches_exc_class(obj_ty, name)
                    || (obj_ty == "bool" && name == "int")
                    || exc_match(name)
                ),
                HeapObj::NativeFn(id) => {
                    let name = id.name();
                    if !matches!(name, "int"|"str"|"bytes"|"float"|"bool"|"list"|"tuple"|"dict"|"set") {
                        return Err(VmErr::Type("isinstance() arg 2 must be a type or tuple of types"));
                    }
                    Ok(
                        name == obj_ty
                        || (obj_ty == "bool" && name == "int")
                        )
                }
                _ => Err(VmErr::Type("isinstance() arg 2 must be a type or tuple of types")),
            }
        };

        if !arg2.is_heap() {
            return Err(VmErr::Type("isinstance() arg 2 must be a type or tuple of types"));
        }

        let result = match self.heap.get(arg2) {
            HeapObj::Type(_) | HeapObj::NativeFn(_) => check_one(arg2, &self.heap)?,
            HeapObj::Tuple(items) => {
                let items: Vec<Val> = items.clone();
                items.iter().any(|&t| check_one(t, &self.heap).unwrap_or(false))
            }
            _ => return Err(VmErr::Type("isinstance() arg 2 must be a type or tuple of types")),
        };

        self.push(Val::bool(result));
        Ok(())
    }
}
