use crate::s;

use alloc::{vec, vec::Vec};

use super::super::VM;
use super::super::types::*;

/* Lazy walker over any iterable Val. Used by `all`/`any`/`sum`/`min`/
   `max` so a `range(10**6)` argument doesn't materialise 10 million Val
   slots before the consumer touches the first one. The Vec variant
   still copies once for list/set/dict (their items aren't streamable
   without holding a mutable borrow on the heap), but Range and Bytes
   produce items by index without pre-allocation, and short-circuiting
   builtins exit on the first match without walking the rest. */
pub(crate) enum IterCursor {
    Range { cur: i64, end: i64, step: i64 },
    Vec { items: Vec<Val>, idx: usize },
    Bytes { bytes: Vec<u8>, idx: usize },
    StrChars { chars: Vec<char>, idx: usize },
}

impl IterCursor {
    /* Yield the next value, allocating into `heap` only when the source
       requires it (currently just StrChars). Returns Err for an
       allocation failure; Ok(None) for exhaustion. */
    pub fn next(&mut self, heap: &mut HeapPool) -> Result<Option<Val>, VmErr> {
        match self {
            Self::Range { cur, end, step } => {
                let (c, e, s) = (*cur, *end, *step);
                let live = if s > 0 { c < e } else if s < 0 { c > e } else { false };
                if !live { return Ok(None); }
                *cur = c + s;
                Ok(Some(Val::int(c)))
            }
            Self::Vec { items, idx } => {
                if *idx >= items.len() { return Ok(None); }
                let v = items[*idx];
                *idx += 1;
                Ok(Some(v))
            }
            Self::Bytes { bytes, idx } => {
                if *idx >= bytes.len() { return Ok(None); }
                let b = bytes[*idx];
                *idx += 1;
                Ok(Some(Val::int(b as i64)))
            }
            Self::StrChars { chars, idx } => {
                if *idx >= chars.len() { return Ok(None); }
                let mut s = alloc::string::String::new();
                s.push(chars[*idx]);
                *idx += 1;
                Ok(Some(heap.alloc(HeapObj::Str(s))?))
            }
        }
    }
}

impl<'a> VM<'a> {

    pub fn call_len(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let n: i64 = if o.is_heap() { match self.heap.get(o) {
            HeapObj::Str(s) => s.chars().count() as i64,
            HeapObj::Bytes(b) => b.len() as i64,
            HeapObj::List(v) => v.borrow().len() as i64,
            HeapObj::Tuple(v) => v.len() as i64,
            HeapObj::Dict(v) => v.borrow().len() as i64,
            HeapObj::Set(v) => v.borrow().len() as i64,
            HeapObj::Range(s,e,st) => { let st=*st; ((e-s+st-st.signum())/st).max(0) }
            _ => return Err(cold_type("object has no len()")),
        }} else { return Err(cold_type("object has no len()")); };
        self.push(Val::int(n)); Ok(())
    }

    pub fn call_sorted(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let mut items = self.extract_iter(o, false)?;
        self.sort_by_lt(&mut items)?;
        self.alloc_and_push_list(items)
    }

    /* sorted(iterable, key=fn). Decorate-sort-undecorate: pre-apply `key`
       to every item, sort the parallel keys vec, then re-emit items in the
       new order. Key=None falls back to the no-key path. */
    pub fn call_sorted_with_key(
        &mut self, key: Option<Val>,
        chunk: &crate::modules::parser::SSAChunk, slots: &mut [Val],
    ) -> Result<(), VmErr> {
        let key = match key {
            Some(k) if !k.is_none() => k,
            _ => return self.call_sorted(),
        };
        let o = self.pop()?;
        let items = self.extract_iter(o, false)?;
        let mut keys: Vec<Val> = Vec::with_capacity(items.len());
        for &item in &items {
            self.push(key);
            self.push(item);
            self.exec_call(1, chunk, slots)?;
            keys.push(self.pop()?);
        }
        let mut indices: Vec<usize> = (0..items.len()).collect();
        let mut sort_err: Option<VmErr> = None;
        indices.sort_by(|&a, &b| {
            if sort_err.is_some() { return core::cmp::Ordering::Equal; }
            match self.lt_vals(keys[a], keys[b]) {
                Ok(true) => core::cmp::Ordering::Less,
                Ok(false) => match self.lt_vals(keys[b], keys[a]) {
                    Ok(true) => core::cmp::Ordering::Greater,
                    Ok(false) => core::cmp::Ordering::Equal,
                    Err(e) => { sort_err = Some(e); core::cmp::Ordering::Equal }
                },
                Err(e) => { sort_err = Some(e); core::cmp::Ordering::Equal }
            }
        });
        if let Some(e) = sort_err { return Err(e); }
        let sorted: Vec<Val> = indices.into_iter().map(|i| items[i]).collect();
        self.alloc_and_push_list(sorted)
    }

    /* In-place sort using lt_vals for ordering. Captures the first error
       and surfaces it after the sort completes — sort_by closures can't
       return Result directly. */
    pub(crate) fn sort_by_lt(&self, items: &mut [Val]) -> Result<(), VmErr> {
        let mut sort_err: Option<VmErr> = None;
        items.sort_by(|&a, &b| {
            if sort_err.is_some() { return core::cmp::Ordering::Equal; }
            match self.lt_vals(a, b) {
                Ok(true) => core::cmp::Ordering::Less,
                Ok(false) => match self.lt_vals(b, a) {
                    Ok(true) => core::cmp::Ordering::Greater,
                    Ok(false) => core::cmp::Ordering::Equal,
                    Err(e) => { sort_err = Some(e); core::cmp::Ordering::Equal }
                },
                Err(e) => { sort_err = Some(e); core::cmp::Ordering::Equal }
            }
        });
        sort_err.map_or(Ok(()), Err)
    }

    pub fn call_reversed(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        if !o.is_heap() { return Err(cold_type("reversed() requires a sequence")); }
        let mut items = if let HeapObj::Str(s) = self.heap.get(o) {
            let s = s.clone();
            self.str_to_char_vals(&s)?
        } else {
            self.extract_iter(o, true)?
        };
        items.reverse();
        self.alloc_and_push_list(items)
    }

    pub fn call_enumerate(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let src = self.extract_iter(o, false)?;
        let mut pairs: Vec<Val> = Vec::with_capacity(src.len());
        for (i, x) in src.into_iter().enumerate() {
            let t = self.heap.alloc(HeapObj::Tuple(vec![Val::int(i as i64), x]))?;
            pairs.push(t);
        }
        self.alloc_and_push_list(pairs)
    }

    /* Pairs elements from N iterables into tuples, truncating to the shortest. */
    pub fn call_zip(&mut self, op: u16) -> Result<(), VmErr> {
        let mut iters: Vec<Vec<Val>> = Vec::with_capacity(op as usize);
        let mut vals = Vec::with_capacity(op as usize);
        for _ in 0..op { vals.push(self.pop()?); }
        vals.reverse();
        for v in vals { iters.push(self.extract_iter(v, false)?); }
        let len = iters.iter().map(|v| v.len()).min().unwrap_or(0);
        let mut pairs: Vec<Val> = Vec::with_capacity(len);
        for i in 0..len {
            let tuple: Vec<Val> = iters.iter().map(|v| v[i]).collect();
            let t = self.heap.alloc(HeapObj::Tuple(tuple))?;
            pairs.push(t);
        }
        self.alloc_and_push_list(pairs)
    }

    /* Build an IterCursor over any iterable Val. Used by short-circuit
       builtins so `all(range(10**6))` walks at most until the first
       falsy element instead of materialising 10M ints first. Returns
       a TypeError for non-iterables, mirroring extract_iter. */
    pub(in crate::modules::vm) fn iter_cursor(&self, o: Val) -> Result<IterCursor, VmErr> {
        if !o.is_heap() {
            return Err(VmErr::TypeMsg(s!("'", str self.type_name(o), "' object is not iterable")));
        }
        Ok(match self.heap.get(o) {
            HeapObj::Range(s, e, st) => IterCursor::Range { cur: *s, end: *e, step: *st },
            HeapObj::Bytes(b) => IterCursor::Bytes { bytes: b.clone(), idx: 0 },  // Vec<u8> clone
            HeapObj::Str(s) => IterCursor::StrChars { chars: s.chars().collect(), idx: 0 },
            HeapObj::List(v) => IterCursor::Vec { items: v.borrow().clone(), idx: 0 },
            HeapObj::Tuple(v) => IterCursor::Vec { items: v.clone(), idx: 0 },
            HeapObj::Set(v) => IterCursor::Vec { items: v.borrow().iter().cloned().collect(), idx: 0 },
            HeapObj::FrozenSet(v) => IterCursor::Vec { items: v.iter().cloned().collect(), idx: 0 },
            HeapObj::Dict(d) => IterCursor::Vec { items: d.borrow().keys().collect(), idx: 0 },
            _ => return Err(VmErr::TypeMsg(s!("'", str self.type_name(o), "' object is not iterable"))),
        })
    }

    /* Extract a Vec<Val> from any iterable: list/tuple/set/frozenset/dict
       (yields keys)/range/str (yields one-char strs)/bytes (yields ints).
       `include_range` is preserved for callers that need to reject Range. */
    pub(in crate::modules::vm) fn extract_iter(&mut self, o: Val, include_range: bool) -> Result<Vec<Val>, VmErr> {
        if !o.is_heap() {
            return Err(VmErr::TypeMsg(s!("'", str self.type_name(o), "' object is not iterable")));
        }
        // Snapshot the variant out so the &self borrow ends before any allocation.
        let snapshot = match self.heap.get(o) {
            HeapObj::List(v)      => Some(v.borrow().clone()),
            HeapObj::Tuple(v)     => Some(v.clone()),
            HeapObj::Set(v)       => Some(v.borrow().iter().cloned().collect()),
            HeapObj::FrozenSet(v) => Some(v.iter().cloned().collect()),
            HeapObj::Range(s, e, st) if include_range => {
                let (mut cur, end, step) = (*s, *e, *st);
                let mut out = Vec::new();
                if step > 0 { while cur < end { out.push(Val::int(cur)); cur += step; } }
                else        { while cur > end { out.push(Val::int(cur)); cur += step; } }
                Some(out)
            }
            HeapObj::Dict(d)      => Some(d.borrow().keys().collect()),
            HeapObj::Bytes(b)     => Some(b.iter().map(|&x| Val::int(x as i64)).collect()),
            HeapObj::Str(_)       => None, // handled below — needs heap allocation
            _ => return Err(VmErr::TypeMsg(s!("'", str self.type_name(o), "' object is not iterable"))),
        };
        if let Some(v) = snapshot { return Ok(v); }
        // Str path materialises one-char heap strings via the existing helper.
        if let HeapObj::Str(s) = self.heap.get(o) {
            let s = s.clone();
            return self.str_to_char_vals(&s);
        }
        unreachable!()
    }

    /* Flatten any iterable to a fresh `Vec<Val>` — the union of every
       sequence-like type the VM exposes. Used by iter()/map()/filter() so
       all three accept the same set of inputs (lists, tuples, sets, dicts
       — keys —, ranges, and strings — chars allocated as length-1 Strs). */
    pub(crate) fn iter_to_vec_general(&mut self, o: Val) -> Result<Vec<Val>, VmErr> {
        if !o.is_heap() {
            return Err(VmErr::TypeMsg(s!("'", str self.type_name(o), "' object is not iterable")));
        }
        if let HeapObj::Str(s) = self.heap.get(o) {
            let s = s.clone();
            return self.str_to_char_vals(&s);
        }
        if let HeapObj::Bytes(b) = self.heap.get(o) {
            // bytes iterates as ints (the byte values), not as length-1
            // bytes. Matches Python and the indexing behavior above.
            return Ok(b.iter().map(|&byte| Val::int(byte as i64)).collect());
        }
        if let HeapObj::Dict(rc) = self.heap.get(o) {
            return Ok(rc.borrow().keys().collect());
        }
        self.extract_iter(o, true)
    }

    /* `iter(x)` — flatten any iterable into a fresh List that `next()`
       drains front-to-back. Eager; the original collection is never
       mutated. Mirrors the universal ABI's `Op::Iter` shape. */
    pub fn call_iter(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let items = self.iter_to_vec_general(o)?;
        self.alloc_and_push_list(items)
    }

    pub fn call_next(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        if !o.is_heap() { return Err(cold_type("next() requires an iterator")); }
        // Lists produced by `iter()` (or used directly) are consumed
        // front-to-back. Matches the universal handle ABI's IterNext op
        // (see `host.rs::dispatch_iter_next`) so script-side `next()` and
        // host-side `Op::IterNext` exhibit identical semantics.
        if let HeapObj::List(rc) = self.heap.get(o) {
            let rc = rc.clone();
            let mut v = rc.borrow_mut();
            if v.is_empty() { return Err(VmErr::Raised(s!("StopIteration"))); }
            let item = v.remove(0);
            drop(v);
            self.push(item);
            return Ok(());
        }
        if !matches!(self.heap.get(o), HeapObj::Coroutine(..)) {
            return Err(cold_type("next() requires an iterator"));
        }
        self.push(o);
        let result = self.resume_coroutine(o)?;
        if self.yielded {
            self.yielded = false;
            self.push(result);
            Ok(())
        } else {
            Err(VmErr::Runtime("StopIteration"))
        }
    }

    /* `map(fn, iter)` — eager: applies `fn` to each item, returns a list.
       Re-enters `exec_call` per item so closures with captures behave like
       native calls (caller chunk/slots match the surrounding frame). */
    pub fn call_map(
        &mut self, chunk: &crate::modules::parser::SSAChunk, slots: &mut [Val],
    ) -> Result<(), VmErr> {
        let iterable = self.pop()?;
        let fn_val = self.pop()?;
        let items = self.iter_to_vec_general(iterable)?;
        let mut out: Vec<Val> = Vec::with_capacity(items.len());
        for item in items {
            self.push(fn_val);
            self.push(item);
            self.exec_call(1, chunk, slots)?;
            out.push(self.pop()?);
        }
        self.alloc_and_push_list(out)
    }

    /* `filter(pred, iter)` — eager: keeps items where `pred(item)` is
       truthy. Same call-shape as `map`. A `None` predicate behaves like
       Python's identity-truthy filter. */
    pub fn call_filter(
        &mut self, chunk: &crate::modules::parser::SSAChunk, slots: &mut [Val],
    ) -> Result<(), VmErr> {
        let iterable = self.pop()?;
        let fn_val = self.pop()?;
        let items = self.iter_to_vec_general(iterable)?;
        let mut out: Vec<Val> = Vec::new();
        for item in items {
            let keep = if fn_val.is_none() {
                self.truthy(item)
            } else {
                self.push(fn_val);
                self.push(item);
                self.exec_call(1, chunk, slots)?;
                let r = self.pop()?;
                self.truthy(r)
            };
            if keep { out.push(item); }
        }
        self.alloc_and_push_list(out)
    }

    pub fn call_all(&mut self, op: u16) -> Result<(), VmErr> {
        if op != 1 { return Err(cold_type("all() takes exactly 1 argument")); }
        let o = self.pop()?;
        let mut cur = self.iter_cursor(o)?;
        while let Some(v) = cur.next(&mut self.heap)? {
            if !self.truthy(v) {
                self.push(Val::bool(false));
                return Ok(());
            }
        }
        self.push(Val::bool(true));
        Ok(())
    }

    pub fn call_any(&mut self, op: u16) -> Result<(), VmErr> {
        if op != 1 { return Err(cold_type("any() takes exactly 1 argument")); }
        let o = self.pop()?;
        let mut cur = self.iter_cursor(o)?;
        while let Some(v) = cur.next(&mut self.heap)? {
            if self.truthy(v) {
                self.push(Val::bool(true));
                return Ok(());
            }
        }
        self.push(Val::bool(false));
        Ok(())
    }

    /* Materialises an iterable to a list. Strings expand to chars;
       ranges materialise eagerly; coroutines are drained by repeated resume. */
    pub fn call_list(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        if o.is_heap() {
            match self.heap.get(o) {
                HeapObj::Str(s) => {
                    let s = s.clone();
                    let items = self.str_to_char_vals(&s)?;
                    return self.alloc_and_push_list(items);
                }
                HeapObj::Coroutine(..) => {
                    let mut out = Vec::new();
                    loop {
                        let v = self.resume_coroutine(o)?;
                        if !self.yielded { break; }
                        self.yielded = false;
                        out.push(v);
                    }
                    return self.alloc_and_push_list(out);
                }
                _ => {}
            }
        }
        let items = self.extract_iter(o, true)?;
        self.alloc_and_push_list(items)
    }

    pub fn call_tuple(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let items = self.extract_iter(o, true)?;
        self.alloc_and_push_tuple(items)
    }

}
