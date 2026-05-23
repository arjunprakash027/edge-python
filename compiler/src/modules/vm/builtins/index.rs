use core::cell::RefCell;
use alloc::{rc::Rc, string::{String, ToString}, vec::Vec};

use super::super::VM;
use super::super::types::*;

fn normalize_index(i: i64, len: usize) -> usize {
    (if i < 0 { len as i64 + i } else { i }) as usize
}

enum SliceSource { List(Vec<Val>), Tuple(Vec<Val>), Str(Vec<char>), Bytes(Vec<u8>) }

impl SliceSource {
    fn len(&self) -> i64 {
        match self {
            Self::List(v) => v.len() as i64,
            Self::Tuple(v) => v.len() as i64,
            Self::Str(v) => v.len() as i64,
            Self::Bytes(v) => v.len() as i64,
        }
    }
}

impl<'a> VM<'a> {

    pub fn get_item(&mut self, ip: usize, chunk: &crate::modules::parser::SSAChunk, slots: &mut [Val], cache: &mut crate::modules::vm::cache::OpcodeCache) -> Result<bool, VmErr> {
        let idx = self.pop()?;
        let obj = self.pop()?;

        // instance `__getitem__` runs before built-in indexing; slices pass through as a single Slice arg.
        if let Some(r) = self.try_call_dunder(obj, "__getitem__", &[idx], chunk, slots)? {
            // F4: record monomorphic hit so the next iteration skips `resolve_attr_silent`.
            self.record_dunder_hit(ip, cache, obj, "__getitem__", 2);
            self.push(r);
            return Ok(true);
        }

        self.get_item_builtin(obj, idx)
    }

    /* No-dunder indexing path. Used by callers without a bytecode frame (FFI re-entry); also the post-dunder fallback inside `get_item`. */
    pub fn get_item_builtin(&mut self, obj: Val, idx: Val) -> Result<bool, VmErr> {
        if idx.is_heap()
            && let HeapObj::Slice(start, stop, step) = self.heap.get(idx).clone() {
                let v = self.slice_val(obj, start, stop, step)?;
                self.push(v);
                return Ok(true);
        }

        if obj.is_heap() && idx.is_int()
            && let HeapObj::Str(s) = self.heap.get(obj) {
                let chars: Vec<char> = s.chars().collect();
                let i = idx.as_int();
                let ui = normalize_index(i, chars.len());
                let c = chars.get(ui).copied().ok_or(cold_value("string index out of range"))?;
                let val = self.heap.alloc(HeapObj::Str(c.to_string()))?;
                self.push(val);
                return Ok(true);
        }

        // `bytes[i]` returns the byte as int (`0..=255`), unlike `str[i]` (length-1 str).
        if obj.is_heap() && idx.is_int()
            && let HeapObj::Bytes(b) = self.heap.get(obj) {
                let i = idx.as_int();
                let ui = normalize_index(i, b.len());
                let byte = *b.get(ui).ok_or(cold_value("bytes index out of range"))?;
                self.push(Val::int(byte as i64));
                return Ok(true);
        }

        let v = self.getitem_val(obj, idx)?;
        self.push(v);
        Ok(false)
    }

    fn slice_val(&mut self, obj: Val, start: Val, stop: Val, step: Val) -> Result<Val, VmErr> {
        if !obj.is_heap() { return Err(cold_type("slice requires a sequence")); }
        let st = if step.is_none() { 1 } else if step.is_int() { step.as_int() } else {
            return Err(cold_type("slice step must be an integer"));
        };
        if st == 0 { return Err(cold_value("slice step cannot be zero")); }

        let source = match self.heap.get(obj) {
            HeapObj::List(v) => SliceSource::List(v.borrow().clone()),
            HeapObj::Tuple(v) => SliceSource::Tuple(v.clone()),
            HeapObj::Str(s) => SliceSource::Str(s.chars().collect()),
            HeapObj::Bytes(b) => SliceSource::Bytes(b.clone()),
            _ => return Err(cold_type("object is not sliceable")),
        };

        let len = source.len();

        let clamp = |v: Val, def: i64| -> i64 {
            if v.is_none() { def }
            else if v.is_int() { let i = v.as_int(); if i < 0 { (len+i).max(0) } else { i.min(len) } }
            else { def }
        };

        let (s, e) = if st > 0 {
            (clamp(start, 0), clamp(stop, len))
        } else {
            (clamp(start, len - 1), clamp(stop, -1))
        };

        let mut indices = Vec::new();
        let mut cur = s;
        if st > 0 { while cur < e { indices.push(cur as usize); cur += st; } }
        else { while cur > e { indices.push(cur as usize); cur += st; } }

        let pick = |v: &[Val]| -> Vec<Val> {
            indices.iter().filter_map(|&i| v.get(i).copied()).collect()
        };

        match source {
            SliceSource::List(v) => self.heap.alloc(HeapObj::List(Rc::new(RefCell::new(pick(&v))))),
            SliceSource::Tuple(v) => self.heap.alloc(HeapObj::Tuple(pick(&v))),
            SliceSource::Str(chars) => {
                let sliced: String = indices.iter().filter_map(|&i| chars.get(i)).collect();
                self.heap.alloc(HeapObj::Str(sliced))
            }
            SliceSource::Bytes(buf) => {
                let sliced: Vec<u8> = indices.iter().filter_map(|&i| buf.get(i).copied()).collect();
                self.heap.alloc(HeapObj::Bytes(sliced))
            }
        }
    }

    pub fn getitem_val(&self, obj: Val, idx: Val) -> Result<Val, VmErr> {
        if !obj.is_heap() { return Err(cold_type("object is not subscriptable")); }
        match self.heap.get(obj) {
            HeapObj::List(v) => {
                if !idx.is_int() { return Err(cold_type("list indices must be integers")); }
                let b = v.borrow(); let i = idx.as_int();
                let ui = normalize_index(i, b.len());
                b.get(ui).copied().ok_or(cold_value("list index out of range"))
            }
            HeapObj::Tuple(v) => {
                if !idx.is_int() { return Err(cold_type("tuple indices must be integers")); }
                let i = idx.as_int();
                let ui = normalize_index(i, v.len());
                v.get(ui).copied().ok_or(cold_value("tuple index out of range"))
            }
            HeapObj::Dict(p) => {
                p.borrow().get(&idx).copied().ok_or(cold_value("key not found"))
            }
            _ => Err(cold_type("object is not subscriptable")),
        }
    }

    /* Reject mutable types (list/dict/set) used as dict/set keys, plus instances that override `__eq__` without `__hash__`. */
    pub(in crate::modules::vm) fn require_hashable(&self, v: Val) -> Result<(), VmErr> {
        if v.is_heap() {
            match self.heap.get(v) {
                HeapObj::List(_) => return Err(cold_type("unhashable type: 'list'")),
                HeapObj::Dict(_) => return Err(cold_type("unhashable type: 'dict'")),
                HeapObj::Set(_) => return Err(cold_type("unhashable type: 'set'")),
                HeapObj::Instance(cls, _) => {
                    // Same eq-hash invariant as `call_hash`; defining one without the other voids hashability.
                    let cls = *cls;
                    if self.lookup_class_member(cls, "__eq__").is_some()
                        && self.lookup_class_member(cls, "__hash__").is_none() {
                        return Err(cold_type("unhashable type: instance defines __eq__ without __hash__"));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn store_item(&mut self, chunk: &crate::modules::parser::SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        let value = self.pop()?;
        let idx_val = self.pop()?;
        let cont = self.pop()?;
        if !cont.is_heap() { return Err(cold_type("object does not support item assignment")); }
        // instance `__setitem__(idx, value)` short-circuits the built-in dispatch.
        if self.try_call_dunder(cont, "__setitem__", &[idx_val, value], chunk, slots)?.is_some() {
            return Ok(());
        }
        self.store_item_builtin(cont, idx_val, value)
    }

    /* No-dunder item-assignment path. Used by callers without a bytecode frame (FFI re-entry); also the post-dunder fallback inside `store_item`. */
    pub fn store_item_builtin(&mut self, cont: Val, idx_val: Val, value: Val) -> Result<(), VmErr> {
        if !cont.is_heap() { return Err(cold_type("object does not support item assignment")); }
        // Slice assignment: `xs[a:b] = iterable` (step must be 1 for resize). Resolves the target range, materialises RHS, and splices in place.
        if idx_val.is_heap()
            && let HeapObj::Slice(start, stop, step) = self.heap.get(idx_val).clone()
        {
            let new_items = self.extract_iter(value, false)?;
            return self.store_slice(cont, start, stop, step, new_items);
        }
        // Reject mutable keys before borrowing the container mutably below.
        if matches!(self.heap.get(cont), HeapObj::Dict(_)) {
            self.require_hashable(idx_val)?;
        }
        match self.heap.get_mut(cont) {
            HeapObj::List(v) => {
                if !idx_val.is_int() { return Err(cold_type("list indices must be integers")); }
                let mut b = v.borrow_mut();
                let i = idx_val.as_int();
                let ui = normalize_index(i, b.len());
                if ui >= b.len() { return Err(cold_value("list assignment index out of range")); }
                b[ui] = value;
            }
            HeapObj::Dict(p) => { p.borrow_mut().insert(idx_val, value); }
            HeapObj::Tuple(_) => return Err(cold_type("tuple does not support item assignment")),
            _ => return Err(cold_type("object does not support item assignment")),
        }
        Ok(())
    }

    pub fn del_item(&mut self, chunk: &crate::modules::parser::SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        let idx_val = self.pop()?;
        let cont = self.pop()?;
        if !cont.is_heap() { return Err(cold_type("object does not support item deletion")); }
        // instance `__delitem__(idx)` short-circuits the built-in dispatch.
        if self.try_call_dunder(cont, "__delitem__", &[idx_val], chunk, slots)?.is_some() {
            return Ok(());
        }
        // Slice deletion: `del xs[a:b]`, same step=1 restriction as `store_slice`. Reuses `store_slice` with an empty replacement vec.
        if idx_val.is_heap()
            && let HeapObj::Slice(start, stop, step) = self.heap.get(idx_val).clone()
        {
            return self.store_slice(cont, start, stop, step, Vec::new());
        }
        match self.heap.get_mut(cont) {
            HeapObj::List(v) => {
                if !idx_val.is_int() { return Err(cold_type("list indices must be integers")); }
                let mut b = v.borrow_mut();
                let ui = normalize_index(idx_val.as_int(), b.len());
                if ui >= b.len() { return Err(cold_value("list index out of range")); }
                b.remove(ui);
            }
            HeapObj::Dict(p) => {
                if p.borrow_mut().remove(&idx_val).is_none() {
                    return Err(cold_value("KeyError"));
                }
            }
            HeapObj::Tuple(_) => return Err(cold_type("tuple does not support item deletion")),
            _ => return Err(cold_type("object does not support item deletion")),
        }
        Ok(())
    }

    /* Splice for `xs[a:b] = items` and `del xs[a:b]`. step=1 resizes; step≠1 demands exact-length RHS. Lists only, tuples/strings are immutable. */
    fn store_slice(&mut self, cont: Val,start: Val, stop: Val, step: Val, new_items: Vec<Val>) -> Result<(), VmErr> {
        let st = if step.is_none() { 1 }
            else if step.is_int() { step.as_int() }
            else { return Err(cold_type("slice step must be an integer")); };
        if st == 0 { return Err(cold_value("slice step cannot be zero")); }

        let HeapObj::List(rc) = self.heap.get_mut(cont) else {
            return Err(cold_type("object does not support slice assignment"));
        };
        let mut b = rc.borrow_mut();
        let len = b.len() as i64;

        let clamp = |v: Val, def: i64| -> i64 {
            if v.is_none() { def }
            else if v.is_int() { let i = v.as_int(); if i < 0 { (len + i).max(0) } else { i.min(len) } }
            else { def }
        };

        if st == 1 {
            let s = clamp(start, 0).max(0) as usize;
            let e = clamp(stop, len).max(s as i64) as usize;
            b.splice(s..e, new_items);
            return Ok(());
        }

        // Extended slice (step!=1): collect indices; RHS length must match exactly.
        let (s, e) = if st > 0 { (clamp(start, 0), clamp(stop, len)) } else { (clamp(start, len - 1), clamp(stop, -1)) };
        let mut indices: Vec<usize> = Vec::new();
        let mut cur = s;
        if st > 0 { while cur < e { indices.push(cur as usize); cur += st; } }
        else { while cur > e { indices.push(cur as usize); cur += st; } }

        if new_items.is_empty() {
            // Remove highest-index first so earlier indices stay valid.
            let mut sorted = indices.clone();
            sorted.sort_unstable();
            for &i in sorted.iter().rev() { b.remove(i); }
            return Ok(());
        }
        if new_items.len() != indices.len() {
            return Err(cold_value("attempt to assign sequence of one size to extended slice of another"));
        }
        for (i, v) in indices.into_iter().zip(new_items) { b[i] = v; }
        Ok(())
    }

    // `slice(stop)` | `slice(start, stop)` | `slice(start, stop, step)`, builtin; usable as a sequence index.
    pub fn call_slice(&mut self, argc: u16) -> Result<(), VmErr> {
        let args = self.pop_n(argc as usize)?;
        let (start, stop, step) = match args.as_slice() {
            [stop] => (Val::none(), *stop, Val::none()),
            [start, stop] => (*start, *stop, Val::none()),
            [start, stop, step] => (*start, *stop, *step),
            _ => return Err(cold_type("slice() takes 1 to 3 arguments")),
        };
        let v = self.heap.alloc(HeapObj::Slice(start, stop, step))?;
        self.push(v); Ok(())
    }
}
