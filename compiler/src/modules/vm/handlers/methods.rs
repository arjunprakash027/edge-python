/*
Attribute resolution for `LoadAttr` / `CallMethod`. Built-in method bodies live in `builtin_methods/`; this file owns `AttrLookup`, the resolver, and the `__getattr__` fallback.
*/

use super::*;
use crate::alloc::string::ToString;
use crate::s;

pub use super::builtin_methods::BuiltinMethodId;
pub(crate) use super::builtin_methods::{dispatch_method, lookup_method};

// `resolve_attr` result, every shape LoadAttr / CallMethod dispatches on.
pub(crate) enum AttrLookup {
    ModuleAttr(Val),
    ClassMember(Val),
    InstanceField(Val),
    // `class` is where `func` was found; the called frame needs it so `super()` knows where to resume.
    InstanceMethod { recv: Val, func: Val, class: Val },
    BuiltinMethod(BuiltinMethodId),
    // `e.args` on ExcInstance, caller picks: LoadAttr materialises the tuple, CallMethod errors.
    ExcArgs(Vec<Val>),
    // Property descriptor on an instance, `LoadAttr` invokes `getter(recv)`.
    PropertyGet { recv: Val, getter: Val },
    // `prop.setter` access, `LoadAttr` materialises a `PropertySetter` value bound to the source property.
    PropertySetterRef(Val),
    // `__name__` on a function, type, or class; `LoadAttr` materialises the str.
    Name(String),
}

impl<'a> VM<'a> {
    // Direct-then-DFS member lookup; first hit wins. Cycles are impossible: bases are validated at `MakeClass` time and `HeapObj::Class` is immutable, so the class graph is a static DAG. Returns `(value, defining_class)` so callers building `BoundUserMethod` / `InstanceMethod` can record where the method came from for `super()`.
    pub(crate) fn lookup_class_member(&self, cls: Val, name: &str) -> Option<(Val, Val)> {
        if !cls.is_heap() { return None; }
        let HeapObj::Class(_, bases, members) = self.heap.get(cls) else { return None; };
        if let Some((_, v)) = members.iter().find(|(n, _)| n == name) { return Some((*v, cls)); }
        for &b in bases {
            if let Some(found) = self.lookup_class_member(b, name) { return Some(found); }
        }
        None
    }

    // Same lookup but skipping `cls` itself; powers `super()` which must search strictly above the current class.
    pub(crate) fn lookup_class_member_after(&self, cls: Val, name: &str) -> Option<(Val, Val)> {
        if !cls.is_heap() { return None; }
        let HeapObj::Class(_, bases, _) = self.heap.get(cls) else { return None; };
        for &b in bases {
            if let Some(found) = self.lookup_class_member(b, name) { return Some(found); }
        }
        None
    }

    // `obj.<name>` resolution shared by `handle_load_attr` and `exec_call_method`.
    pub(crate) fn resolve_attr(&self, obj: Val, name: &str) -> Result<AttrLookup, VmErr> {
        let bare = crate::modules::parser::ssa_strip(name);

        // Module attr: linear scan; the table is sized for around 30 entries.
        if obj.is_heap()
            && let HeapObj::Module(mod_name, attrs) = self.heap.get(obj) {
                if let Some((_, v)) = attrs.iter().find(|(n, _)| n == bare) {
                    return Ok(AttrLookup::ModuleAttr(*v));
                }
                return Err(VmErr::Attribute(s!("module '", str mod_name, "' has no attribute '", str bare, "'")));
            }

        // ExcInstance attr: only `e.args` is defined.
        if obj.is_heap()
            && let HeapObj::ExcInstance(_, args) = self.heap.get(obj) {
                if bare == "args" { return Ok(AttrLookup::ExcArgs(args.clone())); }
                let ty = self.type_name(obj);
                return Err(VmErr::Attribute(s!("'", str ty, "' object has no attribute '", str bare, "'")));
            }

        // `__name__` on callables and types resolves to their declared name.
        if obj.is_heap() && bare == "__name__" {
            let resolved = match self.heap.get(obj) {
                HeapObj::Func(fi, _, _) => self.function_names.get(*fi).cloned(),
                HeapObj::Type(n) => Some(n.clone()),
                HeapObj::Class(n, _, _) => Some(n.clone()),
                _ => None,
            };
            if let Some(n) = resolved { return Ok(AttrLookup::Name(n)); }
        }

        // Class attr: `MyClass.method` returns the unbound function (no `self` prepended).
        if obj.is_heap()
            && let HeapObj::Class(cls_name, _, _) = self.heap.get(obj) {
                if let Some((v, _)) = self.lookup_class_member(obj, bare) { return Ok(AttrLookup::ClassMember(v)); }
                let cls_name = cls_name.clone();
                return Err(VmErr::Attribute(s!("type object '", str &cls_name, "' has no attribute '", str bare, "'")));
            }

        // Instance attribute lookup: check `__dict__` first, then the class chain (direct + bases).
        if obj.is_heap()
            && let HeapObj::Instance(cls_val, attrs) = self.heap.get(obj) {
                let cls_val = *cls_val;
                let found = attrs.borrow().entries.iter()
                    .find(|(k, _)| k.is_heap() && matches!(self.heap.get(*k), HeapObj::Str(s) if s == name))
                    .map(|(_, v)| *v);
                if let Some(v) = found { return Ok(AttrLookup::InstanceField(v)); }
                if let Some((mv, defining)) = self.lookup_class_member(cls_val, bare) {
                    // A Property member triggers getter invocation in `handle_load_attr`; plain methods stay bound to the receiver.
                    if let HeapObj::Property(getter, _) = self.heap.get(mv) {
                        return Ok(AttrLookup::PropertyGet { recv: obj, getter: *getter });
                    }
                    return Ok(AttrLookup::InstanceMethod { recv: obj, func: mv, class: defining });
                }
                let ty = self.type_name(obj);
                return Err(VmErr::Attribute(s!("'", str ty, "' object has no attribute '", str name, "'")));
            }

        // `super().<name>`: search strictly above the proxy's stored class; methods bind to the proxy's `recv`.
        if obj.is_heap()
            && let HeapObj::Super(cls_val, recv) = self.heap.get(obj) {
                let (cls_val, recv) = (*cls_val, *recv);
                if let Some((mv, defining)) = self.lookup_class_member_after(cls_val, name) {
                    return Ok(AttrLookup::InstanceMethod { recv, func: mv, class: defining });
                }
                return Err(VmErr::Attribute(s!("'super' object has no attribute '", str name, "'")));
            }

        // `prop.setter` produces a callable that re-builds the property with a new setter (powers `@x.setter`).
        if obj.is_heap()
            && matches!(self.heap.get(obj), HeapObj::Property(..))
            && bare == "setter" {
                return Ok(AttrLookup::PropertySetterRef(obj));
            }

        // Builtin type method.
        let ty = self.type_name(obj);
        lookup_method(ty, name)
            .map(AttrLookup::BuiltinMethod)
            .ok_or_else(|| VmErr::Attribute(s!("'", str ty, "' object has no attribute '", str name, "'")))
    }

    // `resolve_attr` that swallows `AttributeError` into `None`; other VmErrs still propagate, dunder probes need a miss to be silent.
    pub(crate) fn resolve_attr_silent(&self, obj: Val, name: &str) -> Result<Option<AttrLookup>, VmErr> {
        match self.resolve_attr(obj, name) {
            Ok(lookup) => Ok(Some(lookup)),
            Err(VmErr::Attribute(_)) => Ok(None),
            Err(other) => Err(other),
        }
    }

    /* instance fallback via `__getattr__(name)`. Called by `LoadAttr` / `CallMethod` after the normal lookup raises `AttributeError`. */
    pub(crate) fn try_getattr_fallback(&mut self, obj: Val, name: &str, chunk: &SSAChunk, slots: &mut [Val]) -> Result<Option<Val>, VmErr> {
        if !obj.is_heap() || !matches!(self.heap.get(obj), HeapObj::Instance(..)) { return Ok(None); }
        let bare = crate::modules::parser::ssa_strip(name);
        let name_val = self.heap.alloc(HeapObj::Str(bare.to_string()))?;
        self.try_call_dunder(obj, "__getattr__", &[name_val], chunk, slots)
    }

    pub(crate) fn handle_load_attr(&mut self, name_idx: u16, chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        let name = chunk.names.get(name_idx as usize).ok_or(VmErr::Runtime("LoadAttr: bad name index"))?.clone();
        let obj = self.pop()?;
        let lookup = match self.resolve_attr(obj, &name) {
            Ok(l) => l,
            Err(VmErr::Attribute(msg)) => {
                if let Some(v) = self.try_getattr_fallback(obj, &name, chunk, slots)? {
                    self.push(v);
                    return Ok(());
                }
                return Err(VmErr::Attribute(msg));
            }
            Err(other) => return Err(other),
        };
        match lookup {
            AttrLookup::ModuleAttr(v)
            | AttrLookup::ClassMember(v)
            | AttrLookup::InstanceField(v) => {
                self.push(v);
                Ok(())
            }
            AttrLookup::InstanceMethod { recv, func, class } => {
                let bound = self.heap.alloc(HeapObj::BoundUserMethod(recv, func, class))?;
                self.push(bound);
                Ok(())
            }
            AttrLookup::BuiltinMethod(id) => {
                let bound = self.heap.alloc(HeapObj::BoundMethod(obj, id))?;
                self.push(bound);
                Ok(())
            }
            AttrLookup::ExcArgs(args) => {
                let v = self.heap.alloc(HeapObj::Tuple(args))?;
                self.push(v);
                Ok(())
            }
            AttrLookup::PropertyGet { recv, getter } => {
                // Inline getter call: matches `BoundUserMethod` dispatch (push func, push self, call).
                if self.depth >= self.max_calls { return Err(cold_depth()); }
                self.push(getter);
                self.push(recv);
                self.exec_call(1, chunk, slots)
            }
            AttrLookup::PropertySetterRef(prop) => {
                let v = self.heap.alloc(HeapObj::PropertySetter(prop))?;
                self.push(v);
                Ok(())
            }
            AttrLookup::Name(s) => {
                let v = self.heap.alloc(HeapObj::Str(s))?;
                self.push(v);
                Ok(())
            }
        }
    }
}
