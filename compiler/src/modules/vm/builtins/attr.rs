use crate::s;

use alloc::string::{String, ToString};

use super::super::VM;
use super::super::types::*;

impl<'a> VM<'a> {

    // getattr(obj, name [, default]).
    pub fn call_getattr(&mut self, op: u16) -> Result<(), VmErr> {
        if op != 2 && op != 3 {
            return Err(cold_type("getattr() takes 2 or 3 arguments"));
        }
        let default = if op == 3 { Some(self.pop()?) } else { None };
        let name = self.expect_str_arg("getattr() name must be a string")?;
        let obj = self.pop()?;

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

    // hasattr(obj, name).
    pub fn call_hasattr(&mut self) -> Result<(), VmErr> {
        let name = self.expect_str_arg("hasattr() name must be a string")?;
        let obj = self.pop()?;
        let ty = self.type_name(obj);
        let exists = super::super::handlers::methods::lookup_method(ty, &name).is_some();
        self.push(Val::bool(exists));
        Ok(())
    }

    /* setattr(obj, name, value) — store an attribute on a user instance.
       Mirrors `obj.name = value`. Errors on non-instances since builtin
       types (str/list/dict/...) have no mutable attribute table. */
    pub fn call_setattr(&mut self) -> Result<(), VmErr> {
        let value = self.pop()?;
        let name = self.expect_str_arg("setattr() name must be a string")?;
        let obj = self.pop()?;
        if !obj.is_heap() || !matches!(self.heap.get(obj), HeapObj::Instance(..)) {
            return Err(cold_type("setattr() target must be an instance"));
        }
        let key = self.heap.alloc(HeapObj::Str(name))?;
        if let HeapObj::Instance(_, attrs) = self.heap.get_mut(obj) {
            attrs.borrow_mut().insert(key, value);
        }
        self.push(Val::none());
        Ok(())
    }

    /* delattr(obj, name) — remove an attribute from a user instance. */
    pub fn call_delattr(&mut self) -> Result<(), VmErr> {
        let name = self.expect_str_arg("delattr() name must be a string")?;
        let obj = self.pop()?;
        if !obj.is_heap() || !matches!(self.heap.get(obj), HeapObj::Instance(..)) {
            return Err(cold_type("delattr() target must be an instance"));
        }
        // Strings <= 128 bytes are interned, so re-allocating the name yields
        // the same Val that StoreAttr used as the key — bit-eq lookup hits.
        let key = self.heap.alloc(HeapObj::Str(name))?;
        if let HeapObj::Instance(_, attrs) = self.heap.get(obj) {
            attrs.borrow_mut().remove(&key);
        }
        self.push(Val::none());
        Ok(())
    }

    /* Pops the top of stack and returns its String contents, or errors with
       `msg` if it is not a heap string. */
    fn expect_str_arg(&mut self, msg: &'static str) -> Result<String, VmErr> {
        let v = self.pop()?;
        if v.is_heap() && let HeapObj::Str(s) = self.heap.get(v) { return Ok(s.clone()); }
        Err(cold_type(msg))
    }

    /* vars(obj) — Instance: copy of __dict__ as a dict; Module: dict from
       its attrs table. The no-arg form (returning the local frame) is not
       supported; use `locals()` instead. */
    pub fn call_vars(&mut self) -> Result<(), VmErr> {
        use alloc::vec::Vec;
        let obj = self.pop()?;
        if !obj.is_heap() {
            return Err(cold_type("vars() requires an instance or module"));
        }
        // Two passes so we can drop the immutable borrow on heap before any
        // alloc(). For modules, materialise the attr names first as Vec<String>.
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
        for (k, v) in entries { dm.insert(k, v); }
        self.alloc_and_push_dict(dm)
    }

    /* globals() — module-level bindings as a dict. Combines the VM's
       `globals` HashMap (builtins, types, module references) with the
       entry-chunk slots (user-defined top-level names, which live in
       chunk slots rather than the globals map). Returned dict is a
       *copy* — mutating it does not change the VM. */
    pub fn call_globals(
        &mut self, chunk: &crate::modules::parser::SSAChunk, slots: &[Val],
    ) -> Result<(), VmErr> {
        // Builtin/type/module pairs from self.globals, deduped to bare names.
        let mut out: crate::util::fx::FxHashMap<String, Val> =
            crate::util::fx::FxHashMap::default();
        for (k, v) in self.globals.iter() {
            // Drop SSA-mirrors (`x_0`, `x_1`); keep canonical bare name.
            if let Some((bare, suf)) = k.rsplit_once('_')
                && suf.chars().all(|c| c.is_ascii_digit())
            {
                out.entry(bare.to_string()).or_insert(*v);
                continue;
            }
            out.insert(k.clone(), *v);
        }
        // Walk the entry chunk's slots. When we're already in the entry
        // frame, `chunk == self.chunk` and `slots` is exactly its slot
        // array. From inside a function, the entry slots live at the
        // bottom of `live_slots`.
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
        let mut dm = DictMap::with_capacity(out.len());
        for (k, v) in out {
            let key = self.heap.alloc(HeapObj::Str(k))?;
            dm.insert(key, v);
        }
        self.alloc_and_push_dict(dm)
    }

    /* locals() — current frame's local bindings as a dict. Walks the
       caller-supplied chunk.names + slots, deduping multiple SSA
       versions of the same name (`x_0`, `x_1`, ...) by keeping the
       highest-version live value. Filters out:
         - synthetic slots (leading `#`, e.g. `#match0`)
         - builtins that haven't been rebound in this frame (the slot
           still holds the same Val as the global registration). */
    pub fn call_locals(
        &mut self, chunk: &crate::modules::parser::SSAChunk, slots: &[Val],
    ) -> Result<(), VmErr> {
        // Map bare-name -> (best version, val) so we keep only the latest.
        let mut latest: crate::util::fx::FxHashMap<String, (i64, Val)> =
            crate::util::fx::FxHashMap::default();
        for (i, name) in chunk.names.iter().enumerate() {
            let v = match slots.get(i) {
                Some(v) if !v.is_undef() => *v,
                _ => continue,
            };
            // Synthetic slots (`#match0`, `#match_item0`) are matcher
            // scratch — never user-visible.
            if name.starts_with('#') { continue; }
            // Strip SSA version suffix.
            let (bare, ver) = crate::modules::parser::SsaName::parse_or_bare(name);
            let ver = ver as i64;
            // Skip unmodified builtins: if the global registration for this
            // name points at the *same* Val we'd return, the user never
            // rebound it locally — exclude from locals().
            if let Some(&gv) = self.globals.get(bare)
                && gv.0 == v.0 { continue; }
            let entry = latest.entry(bare.to_string()).or_insert((-1, Val::undef()));
            if ver > entry.0 { *entry = (ver, v); }
        }
        let mut dm = DictMap::with_capacity(latest.len());
        for (name, (_, v)) in latest {
            let key = self.heap.alloc(HeapObj::Str(name))?;
            dm.insert(key, v);
        }
        self.alloc_and_push_dict(dm)
    }
}
