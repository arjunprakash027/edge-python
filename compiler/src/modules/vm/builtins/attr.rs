use crate::s;

use alloc::string::{String, ToString};

use super::super::VM;
use super::super::types::*;

impl<'a> VM<'a> {

    // `getattr(obj, name [, default])`.
    pub fn call_getattr(&mut self, op: u16) -> Result<(), VmErr> {
        if op != 2 && op != 3 {
            return Err(cold_type("getattr() takes 2 or 3 arguments"));
        }
        let default = if op == 3 { Some(self.pop()?) } else { None };
        let name = self.expect_str_arg("getattr() name must be a string")?;
        let obj = self.pop()?;

        // Class target: resolve a class attribute (incl. ones added via setattr / a decorator).
        if obj.is_heap() && matches!(self.heap.get(obj), HeapObj::Class(..))
            && let Some((v, _)) = self.lookup_class_member(obj, &name) {
            self.push(v);
            return Ok(());
        }
        let ty = self.type_name(obj);
        if let Some(method_id) = super::super::handlers::methods::lookup_method(ty, &name) {
            let bound = self.heap.alloc(HeapObj::BoundMethod(obj, method_id))?;
            self.push(bound);
            return Ok(());
        }
        if let Some(d) = default {
            self.push(d);
            return Ok(());
        }
        Err(VmErr::Attribute(s!("'", str ty, "' object has no attribute '", str &name, "'")))
    }

    // `hasattr(obj, name)`.
    pub fn call_hasattr(&mut self) -> Result<(), VmErr> {
        let name = self.expect_str_arg("hasattr() name must be a string")?;
        let obj = self.pop()?;
        let is_class_attr = obj.is_heap()
            && matches!(self.heap.get(obj), HeapObj::Class(..))
            && self.lookup_class_member(obj, &name).is_some();
        let ty = self.type_name(obj);
        let exists = is_class_attr || super::super::handlers::methods::lookup_method(ty, &name).is_some();
        self.push(Val::bool(exists));
        Ok(())
    }

    /* `setattr(obj, name, value)`, mirrors `obj.name = value`. Instance-only: builtin types have no mutable attribute table. */
    pub fn call_setattr(&mut self) -> Result<(), VmErr> {
        let value = self.pop()?;
        let name = self.expect_str_arg("setattr() name must be a string")?;
        let obj = self.pop()?;
        // Class target: insert or replace in the mutable members store.
        if obj.is_heap() && matches!(self.heap.get(obj), HeapObj::Class(..)) {
            if let HeapObj::Class(_, _, members) = self.heap.get(obj) {
                let mut m = members.borrow_mut();
                match m.iter_mut().find(|(n, _)| *n == name) {
                    Some(slot) => slot.1 = value,
                    None => m.push((name, value)),
                }
            }
            self.push(Val::none());
            return Ok(());
        }
        if !obj.is_heap() || !matches!(self.heap.get(obj), HeapObj::Instance(..)) {
            return Err(cold_type("setattr() target must be an instance or class"));
        }
        let key = self.heap.alloc(HeapObj::Str(name))?;
        if let HeapObj::Instance(_, attrs) = self.heap.get(obj) {
            attrs.borrow_mut().insert(key, value, &self.heap);
        }
        self.push(Val::none());
        Ok(())
    }

    /* `delattr(obj, name)`, remove an attribute from a user instance or class. */
    pub fn call_delattr(&mut self) -> Result<(), VmErr> {
        let name = self.expect_str_arg("delattr() name must be a string")?;
        let obj = self.pop()?;
        if obj.is_heap() && matches!(self.heap.get(obj), HeapObj::Class(..)) {
            if let HeapObj::Class(_, _, members) = self.heap.get(obj) {
                members.borrow_mut().retain(|(n, _)| *n != name);
            }
            self.push(Val::none());
            return Ok(());
        }
        if !obj.is_heap() || !matches!(self.heap.get(obj), HeapObj::Instance(..)) {
            return Err(cold_type("delattr() target must be an instance or class"));
        }
        // Strings <=128 bytes are interned, so re-alloc'ing yields the same Val key StoreAttr used.
        let key = self.heap.alloc(HeapObj::Str(name))?;
        if let HeapObj::Instance(_, attrs) = self.heap.get(obj) {
            attrs.borrow_mut().remove(&key, &self.heap);
        }
        self.push(Val::none());
        Ok(())
    }

    // Pops TOS and returns its String, or errors with `msg` if it isn't a heap string.
    fn expect_str_arg(&mut self, msg: &'static str) -> Result<String, VmErr> {
        let v = self.pop()?;
        if v.is_heap() && let HeapObj::Str(s) = self.heap.get(v) { return Ok(s.clone()); }
        Err(cold_type(msg))
    }

    /* `vars(obj)`, Instance: copy of `__dict__`; Module: dict from its attrs. No-arg form is unsupported, use `locals()`. */
    pub fn call_vars(&mut self) -> Result<(), VmErr> {
        use alloc::vec::Vec;
        let obj = self.pop()?;
        if !obj.is_heap() {
            return Err(cold_type("vars() requires an instance or module"));
        }
        // Two passes: drop the heap borrow before `alloc()`. Modules materialise names as `Vec<String>` first.
        enum Source { Instance(Vec<(Val, Val)>), Module(Vec<(String, Val)>) }
        let src = match self.heap.get(obj) {
            HeapObj::Instance(_, attrs) => Source::Instance(attrs.borrow().entries.clone()),
            HeapObj::Module(_, attrs) => Source::Module(attrs.clone()),
            _ => return Err(cold_type("vars() requires an instance or module")),
        };
        let entries: Vec<(Val, Val)> = match src {
            Source::Instance(e) => e,
            Source::Module(items) => {
                let mut out = Vec::with_capacity(items.len());
                for (name, v) in items {
                    let key = self.heap.alloc(HeapObj::Str(name))?;
                    out.push((key, v));
                }
                out
            }
        };
        let mut dm = DictMap::with_capacity(entries.len());
        for (k, v) in entries { dm.insert(k, v, &self.heap); }
        self.alloc_and_push_dict(dm)
    }

    /* `globals()`, module-level bindings as a dict. User top-level names only (entry-chunk slots + module state); builtins live in a separate namespace, matching CPython. Returned dict is a copy. */
    pub fn call_globals(&mut self, chunk: &crate::modules::parser::SSAChunk, slots: &[Val]) -> Result<(), VmErr> {
        let mut out: crate::util::fx::FxHashMap<String, Val> = crate::util::fx::FxHashMap::default();
        // Inside a function, entry slots sit at the bottom of `live_slots`; at top-level, use `slots` as-is.
        let (entry_chunk, entry_slots): (&crate::modules::parser::SSAChunk, &[Val]) =
            if core::ptr::eq(chunk as *const _, self.chunk as *const _) {
                (chunk, slots)
            } else {
                let n = self.chunk.names.len().min(self.live_slots.len());
                (self.chunk, &self.live_slots[..n])
            };
        for (i, name) in entry_chunk.names.iter().enumerate() {
            if name.starts_with('#') { continue; }
            let v = match entry_slots.get(i) {
                Some(v) if !v.is_undef() => *v,
                _ => continue,
            };
            let bare = crate::modules::parser::ssa_strip(name).to_string();
            // User assignment overrides the builtin entry of the same name.
            out.insert(bare, v);
        }
        // Module state (user-mutated via `global` from inside functions) overrides entry-chunk snapshots.
        for (k, v) in self.module_state.iter() {
            out.insert(k.clone(), *v);
        }
        let mut dm = DictMap::with_capacity(out.len());
        for (k, v) in out {
            let key = self.heap.alloc(HeapObj::Str(k))?;
            dm.insert(key, v, &self.heap);
        }
        self.alloc_and_push_dict(dm)
    }

    /* `locals()`, frame bindings as a dict. Dedupes SSA versions (`x_0`, `x_1`, ...) to the highest live one. Filters synthetic `#`-slots and unrebound builtins (same Val as the global). */
    pub fn call_locals(&mut self, chunk: &crate::modules::parser::SSAChunk, slots: &[Val]) -> Result<(), VmErr> {
        // Map bare-name -> (best version, val) so we keep only the latest.
        let mut latest: crate::util::fx::FxHashMap<String, (i64, Val)> = crate::util::fx::FxHashMap::default();
        for (i, name) in chunk.names.iter().enumerate() {
            let v = match slots.get(i) {
                Some(v) if !v.is_undef() => *v,
                _ => continue,
            };
            // Synthetic `#`-slots are matcher scratch, never user-visible.
            if name.starts_with('#') { continue; }
            // Strip SSA version suffix.
            let (bare, ver) = crate::modules::parser::SsaName::parse_or_bare(name);
            let ver = ver as i64;
            // Skip unrebound builtins: same Val as the global means the user never assigned locally.
            if let Some(&gv) = self.globals.get(bare)
                && gv.0 == v.0 { continue; }
            let entry = latest.entry(bare.to_string()).or_insert((-1, Val::undef()));
            if ver > entry.0 { *entry = (ver, v); }
        }
        let mut dm = DictMap::with_capacity(latest.len());
        for (name, (_, v)) in latest {
            let key = self.heap.alloc(HeapObj::Str(name))?;
            dm.insert(key, v, &self.heap);
        }
        self.alloc_and_push_dict(dm)
    }
}
