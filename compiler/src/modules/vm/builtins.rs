use crate::s;

use super::VM;
use super::types::*;

/* Static parent map for built-in exception types. Walked by matches_exc_class
   so `except Exception` catches RuntimeError, ValueError, etc. — paradigm
   keeps user classes flat; only the standard exception tree is encoded here. */
const EXC_PARENTS: &[(&str, &str)] = &[
    ("RuntimeError",        "Exception"),
    ("ValueError",          "Exception"),
    ("TypeError",           "Exception"),
    ("KeyError",            "Exception"),
    ("IndexError",          "Exception"),
    ("AttributeError",      "Exception"),
    ("ZeroDivisionError",   "Exception"),
    ("OverflowError",       "Exception"),
    ("NameError",           "Exception"),
    ("StopIteration",       "Exception"),
    ("StopAsyncIteration",  "Exception"),
    ("NotImplementedError", "RuntimeError"),
    ("RecursionError",      "RuntimeError"),
    ("MemoryError",         "Exception"),
    ("TimeoutError",        "Exception"),
    ("CancelledError",      "Exception"),
    ("Exception",           "BaseException"),
];

pub(super) fn matches_exc_class(actual: &str, expected: &str) -> bool {
    let mut cur = actual;
    loop {
        if cur == expected { return true; }
        match EXC_PARENTS.iter().find(|(c, _)| *c == cur) {
            Some(&(_, p)) => cur = p,
            None => return false,
        }
    }
}

use core::cell::RefCell;
use alloc::{string::{String, ToString}, vec::Vec, vec, rc::Rc};
use crate::modules::fx::FxHashSet as HashSet;

fn i64_to_radix(n: i64, radix: u32, prefix: &str) -> String {
    if n == 0 {
        let mut s = String::with_capacity(prefix.len() + 1);
        s.push_str(prefix); s.push('0'); return s;
    }
    let neg = n < 0;
    let mut abs = (n as i128).unsigned_abs();
    let mut buf = [0u8; 64];
    let mut i = buf.len();
    while abs > 0 {
        i -= 1;
        let d = (abs % radix as u128) as u8;
        buf[i] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
        abs /= radix as u128;
    }
    let mut s = String::with_capacity(prefix.len() + buf.len() - i + 1);
    if neg { s.push('-'); }
    s.push_str(prefix);
    s.push_str(unsafe { core::str::from_utf8_unchecked(&buf[i..]) });
    s
}

fn normalize_index(i: i64, len: usize) -> usize {
    (if i < 0 { len as i64 + i } else { i }) as usize
}

enum SliceSource { List(Vec<Val>), Tuple(Vec<Val>), Str(Vec<char>), Bytes(Vec<u8>) }

impl SliceSource {
    fn len(&self) -> i64 {
        match self {
            Self::List(v)  => v.len() as i64,
            Self::Tuple(v) => v.len() as i64,
            Self::Str(v)   => v.len() as i64,
            Self::Bytes(v) => v.len() as i64,
        }
    }
}

impl<'a> VM<'a> {

    #[inline]
    pub(super) fn mark_impure(&mut self) {
        if let Some(top) = self.observed_impure.last_mut() {
            *top = true;
        }
    }

    /* Pops N args, joins with single spaces. Calls `print_hook` if set (streaming),
       otherwise buffers into `output`. */
    pub fn call_print(&mut self, op: u16) -> Result<(), VmErr> {
        let args = self.pop_n(op as usize)?;
        let mut out = String::new();
        for (i, v) in args.iter().enumerate() {
            if i > 0 { out.push(' '); }
            out.push_str(&self.display(*v));
        }
        match self.print_hook {
            Some(hook) => hook(&out),
            None       => self.output.push(out),
        }
        Ok(())
    }

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

    pub fn call_abs(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let v = if o.is_int() {
            self.int_or_overflow(o.as_int().checked_abs())?
        } else if o.is_float() {
            Val::float(o.as_float().abs())
        } else {
            return Err(cold_type("abs() requires a number"));
        };
        self.push(v); Ok(())
    }

    pub fn call_str(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        self.alloc_and_push_str(self.display(o))
    }

    /* Heap-alloc `s` and push the resulting Val. Used by ~10 builtins
       (str / repr / chr / format / ...) that produce string results. */
    fn alloc_and_push_str(&mut self, s: String) -> Result<(), VmErr> {
        let v = self.heap.alloc(HeapObj::Str(s))?;
        self.push(v); Ok(())
    }

    /* Allocate a List from items and push. Centralises the
       Rc::new(RefCell::new(items)) construction inlined ~15 times. */
    pub(crate) fn alloc_list(&mut self, items: Vec<Val>) -> Result<Val, VmErr> {
        self.heap.alloc(HeapObj::List(Rc::new(RefCell::new(items))))
    }

    /* Allocate a List, push it, return Ok. */
    pub(crate) fn alloc_and_push_list(&mut self, items: Vec<Val>) -> Result<(), VmErr> {
        let v = self.alloc_list(items)?;
        self.push(v); Ok(())
    }

    /* Allocate a Set from a Vec (deduping by Val's bit-eq), push, return
       Ok. Mirrors `alloc_and_push_list`. Used by `set` methods that yield
       a fresh set value (copy, union, intersection, difference, etc.). */
    pub(crate) fn alloc_and_push_set(&mut self, items: Vec<Val>) -> Result<(), VmErr> {
        let v = self.alloc_set(items)?;
        self.push(v); Ok(())
    }

    /* Allocate a Dict from a DictMap and push. */
    pub(crate) fn alloc_and_push_dict(&mut self, dm: DictMap) -> Result<(), VmErr> {
        let v = self.heap.alloc(HeapObj::Dict(Rc::new(RefCell::new(dm))))?;
        self.push(v); Ok(())
    }

    /* Allocate a Tuple and push. */
    pub(crate) fn alloc_and_push_tuple(&mut self, items: Vec<Val>) -> Result<(), VmErr> {
        let v = self.heap.alloc(HeapObj::Tuple(items))?;
        self.push(v); Ok(())
    }

    pub fn call_int(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let i = if o.is_int() { o.as_int() }
            else if o.is_float() { o.as_float() as i64 }
            else if o.is_bool() { o.as_bool() as i64 }
            else if o.is_heap() && let HeapObj::Str(s) = self.heap.get(o) {
                s.trim().parse().map_err(|_| cold_value("int(): invalid literal"))?
            }
            else { return Err(cold_type("int() requires a number or string")); };
        let v = self.int_or_overflow(Some(i))?;
        self.push(v); Ok(())
    }

    /* Converts int or parseable string to floating point. */
    
    pub fn call_float(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let f = if o.is_float() { o.as_float() }
            else if o.is_bool() { o.as_bool() as i64 as f64 }
            else if o.is_int() { o.as_int() as f64 }
            else if o.is_heap() && let HeapObj::Str(s) = self.heap.get(o) {
                s.trim().parse().map_err(|_| cold_value("float(): invalid literal"))?
            }
            else { return Err(cold_type("float() requires a number or string")); };
        self.push(Val::float(f)); Ok(())
    }

    pub fn call_bool(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?; self.push(Val::bool(self.truthy(o))); Ok(())
    }

    pub fn call_type(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let s = self.type_name(o);
        self.alloc_and_push_str(s!("<class '", str s, "'>"))
    }

    pub fn call_chr(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        if !o.is_int() { return Err(cold_type("chr() requires an integer")); }
        let c = char::from_u32(o.as_int() as u32).ok_or(cold_value("chr() arg out of range"))?;
        let mut s = String::with_capacity(4);
        s.push(c);
        self.alloc_and_push_str(s)
    }

    pub fn call_ord(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        if o.is_heap()
            && let HeapObj::Str(s) = self.heap.get(o) {
                let mut cs = s.chars();
                if let (Some(c), None) = (cs.next(), cs.next()) {
                    self.push(Val::int(c as i64)); return Ok(());
                }
        }
        Err(cold_type("ord() requires string of length 1"))
    }

    pub fn call_range(&mut self, op: u16) -> Result<(), VmErr> {
        let args = self.pop_n(op as usize)?;
        let gi = |v: Val| -> Result<i64, VmErr> {
            if v.is_int() { Ok(v.as_int()) } else { Err(cold_type("range() arguments must be integers")) }
        };
        let (s, e, st) = match args.len() {
            1 => (0, gi(args[0])?, 1),
            2 => (gi(args[0])?, gi(args[1])?, 1),
            3 => (gi(args[0])?, gi(args[1])?, gi(args[2])?),
            _ => return Err(cold_type("range() takes 1 to 3 arguments")),
        };
        if st == 0 { return Err(cold_value("range() step cannot be zero")); }
        let val = self.heap.alloc(HeapObj::Range(s, e, st))?;
        self.push(val); Ok(())
    }

    pub fn call_round(&mut self, op: u16) -> Result<(), VmErr> {
        let args = self.pop_n(op as usize)?;
        let v = match (args.first(), args.get(1)) {
            (Some(o), Some(n)) if o.is_float() && n.is_int() => {
                let factor = fpowi(10.0, n.as_int() as i32);
                Val::float(fround(o.as_float() * factor) / factor)
            }
            (Some(o), None) if o.is_float() => Val::int(fround(o.as_float()) as i64),
            (Some(o), _) if o.is_int() => *o,
            _ => return Err(cold_type("round() requires a number")),
        };
        self.push(v); Ok(())
    }

    pub fn call_min(&mut self, op: u16) -> Result<(), VmErr> { self.call_minmax(op, true) }
    pub fn call_max(&mut self, op: u16) -> Result<(), VmErr> { self.call_minmax(op, false) }

    fn call_minmax(&mut self, op: u16, is_min: bool) -> Result<(), VmErr> {
        let args = self.pop_n(op as usize)?;
        let items = self.unwrap_single_iterable(args)?;
        let label = if is_min { "min() arg is an empty sequence" } else { "max() arg is an empty sequence" };
        if items.is_empty() { return Err(cold_value(label)); }
        let m = items[1..].iter().try_fold(items[0], |m, &x| {
            let (l, r) = if is_min { (x, m) } else { (m, x) };
            self.lt_vals(l, r).map(|lt| if lt { x } else { m })
        })?;
        self.push(m); Ok(())
    }

    pub fn call_sum(&mut self, op: u16) -> Result<(), VmErr> {
        let args = self.pop_n(op as usize)?;
        if args.is_empty() { return Err(cold_type("sum() requires at least 1 argument")); }
        let start = if args.len() > 1 { args[1] } else { Val::int(0) };
        let items = self.extract_iter(args[0], false)?;
        let mut acc = start;
        for item in items { acc = self.add_vals(acc, item)?; }
        self.push(acc); Ok(())
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

    /* Returns empty string in sandbox; no stdin access in WASM. */

    pub fn call_input(&mut self) -> Result<(), VmErr> {
        let s = if !self.input_buffer.is_empty() {
            self.input_buffer.remove(0)
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let mut line = String::new();
                let _ = std::io::stdin().read_line(&mut line);
                while line.ends_with('\n') || line.ends_with('\r') { line.pop(); }
                line
            }
            #[cfg(target_arch = "wasm32")]
            { return Err(VmErr::Runtime("input() requires host data in WASM (use set_input)")); }
        };
        let val = self.heap.alloc(HeapObj::Str(s))?;
        self.push(val); Ok(())
    }

    // Shared helpers.

    /* If a single arg is a list/tuple/set, return its items; otherwise pass
       args through unchanged. Used by min/max / etc. for varargs vs iterable. */
    fn unwrap_single_iterable(&self, args: Vec<Val>) -> Result<Vec<Val>, VmErr> {
        if args.len() == 1 && args[0].is_heap() {
            match self.heap.get(args[0]) {
                HeapObj::List(v) => return Ok(v.borrow().clone()),
                HeapObj::Tuple(v) => return Ok(v.clone()),
                HeapObj::Set(v) => return Ok(v.borrow().iter().cloned().collect()),
                HeapObj::FrozenSet(v) => return Ok(v.iter().cloned().collect()),
                _ => {}
            }
        }
        Ok(args)
    }

    /* Extract a Vec<Val> from any iterable: list/tuple/set/frozenset/dict
       (yields keys)/range/str (yields one-char strs)/bytes (yields ints).
       `include_range` is preserved for callers that need to reject Range. */
    pub(super) fn extract_iter(&mut self, o: Val, include_range: bool) -> Result<Vec<Val>, VmErr> {
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

    fn alloc_set(&mut self, items: Vec<Val>) -> Result<Val, VmErr> {
        let mut set = HashSet::with_capacity_and_hasher(items.len(), Default::default());
        for v in items { set.insert(v); }
        self.heap.alloc(HeapObj::Set(Rc::new(RefCell::new(set))))
    }

    pub fn build_set(&mut self, op: u16) -> Result<(), VmErr> {
        let items = self.pop_n(op as usize)?;
        for v in &items { self.require_hashable(*v)?; }
        let val = self.alloc_set(items)?;
        self.push(val); Ok(())
    }

    pub fn build_slice(&mut self, op: u16) -> Result<(), VmErr> {
        let step = if op == 3 { self.pop()? } else { Val::none() };
        let stop = self.pop()?;
        let start = self.pop()?;
        let val = self.heap.alloc(HeapObj::Slice(start, stop, step))?;
        self.push(val); Ok(())
    }

    pub fn unpack_ex(&mut self, op: u16) -> Result<(), VmErr> {
        let obj = self.pop()?;
        if !obj.is_heap() { return Err(cold_type("cannot unpack non-iterable")); }
        let items: Vec<Val> = match self.heap.get(obj) {
            HeapObj::List(v) => v.borrow().clone(),
            HeapObj::Tuple(v) => v.clone(),
            _ => return Err(cold_type("cannot unpack non-iterable")),
        };
        let before = (op >> 8) as usize;
        let after = (op & 0xFF) as usize;
        if items.len() < before + after {
            return Err(cold_value("not enough values to unpack"));
        }
        let mid = items.len() - after;
        for &v in items[mid..].iter().rev() { self.push(v); }
        let star = self.alloc_list(items[before..mid].to_vec())?;
        self.push(star);
        for &v in items[..before].iter().rev() { self.push(v); }
        Ok(())
    }

    pub fn call_dict(&mut self, op: u16) -> Result<(), VmErr> {
        let dm = if op == 0 {
            DictMap::new()
        } else {
            let args = self.pop_n((op as usize) * 2)?;
            let mut dm = DictMap::with_capacity(op as usize);
            for pair in args.chunks(2) { dm.insert(pair[0], pair[1]); }
            dm
        };
        self.alloc_and_push_dict(dm)
    }

    pub fn call_set(&mut self, op: u16) -> Result<(), VmErr> {
        if op == 0 {
            let val = self.alloc_set(Vec::new())?;
            self.push(val);
        } else {
            let o = self.pop()?;
            let src = self.extract_iter(o, true)?;
            let val = self.alloc_set(src)?;
            self.push(val);
        }
        Ok(())
    }

    pub fn get_item(&mut self) -> Result<bool, VmErr> {
        let idx = self.pop()?;
        let obj = self.pop()?;

        if idx.is_heap()
            && let HeapObj::Slice(start, stop, step) = self.heap.get(idx).clone() {
                let v = self.slice_val(obj, start, stop, step)?;
                self.push(v);
                return Ok(true);
        }

        if obj.is_heap() && idx.is_int()
            && let HeapObj::Str(s) = self.heap.get(obj) {
                let chars: Vec<char> = s.chars().collect();
                let i  = idx.as_int();
                let ui = normalize_index(i, chars.len());
                let c  = chars.get(ui).copied().ok_or(cold_value("string index out of range"))?;
                let val = self.heap.alloc(HeapObj::Str(c.to_string()))?;
                self.push(val);
                return Ok(true);
        }

        // bytes[i] yields an int (the byte value 0..=255), distinct from
        // str[i] which yields a length-1 str. Matches Python semantics —
        // the principal reason `bytes` is a separate type.
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
            SliceSource::List(v)  => self.heap.alloc(HeapObj::List(Rc::new(RefCell::new(pick(&v))))),
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
                p.borrow().get(&idx).copied()
                    .ok_or(cold_value("key not found"))
            }
            _ => Err(cold_type("object is not subscriptable")),
        }
    }

    /* Reject mutable types (list/dict/set) used as dict/set keys — CPython
       parity. Called wherever a Val crosses into a hash-keyed container. */
    pub(super) fn require_hashable(&self, v: Val) -> Result<(), VmErr> {
        if v.is_heap() {
            match self.heap.get(v) {
                HeapObj::List(_) => return Err(cold_type("unhashable type: 'list'")),
                HeapObj::Dict(_) => return Err(cold_type("unhashable type: 'dict'")),
                HeapObj::Set(_)  => return Err(cold_type("unhashable type: 'set'")),
                _ => {}
            }
        }
        Ok(())
    }

    pub fn store_item(&mut self) -> Result<(), VmErr> {
        let value = self.pop()?;
        let idx_val = self.pop()?;
        let cont = self.pop()?;
        if !cont.is_heap() { return Err(cold_type("object does not support item assignment")); }
        // Slice assignment: `xs[a:b] = iterable` (step must be 1 for resize).
        // Resolves the target range, materialises RHS, and splices in place.
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

    pub fn del_item(&mut self) -> Result<(), VmErr> {
        let idx_val = self.pop()?;
        let cont    = self.pop()?;
        if !cont.is_heap() { return Err(cold_type("object does not support item deletion")); }
        // Slice deletion: `del xs[a:b]` — same step=1 restriction as
        // store_slice. Reuses store_slice with an empty replacement vec.
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

    /* Splice replacement for `xs[a:b] = items` and `del xs[a:b]`. Only
       step=1 slices resize the list; step != 1 requires the replacement to
       match the slice's element count exactly (Python's extended-slice
       rule). For lists only — tuples/strings are immutable. */
    fn store_slice(
        &mut self, cont: Val,
        start: Val, stop: Val, step: Val,
        new_items: Vec<Val>,
    ) -> Result<(), VmErr> {
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

        // Extended slice (step != 1): collect target indices, require RHS
        // length to match exactly.
        let (s, e) = if st > 0 { (clamp(start, 0), clamp(stop, len)) }
                     else      { (clamp(start, len - 1), clamp(stop, -1)) };
        let mut indices: Vec<usize> = Vec::new();
        let mut cur = s;
        if st > 0 { while cur < e { indices.push(cur as usize); cur += st; } }
        else      { while cur > e { indices.push(cur as usize); cur += st; } }

        if new_items.is_empty() {
            // Extended-slice deletion: remove highest-index first to keep
            // earlier indices valid.
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

    // Logical reductions.

    pub fn call_all(&mut self, op: u16) -> Result<(), VmErr> {
        if op != 1 { return Err(cold_type("all() takes exactly 1 argument")); }
        let o = self.pop()?;
        let items = self.extract_iter(o, true)?;
        for v in items {
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
        let items = self.extract_iter(o, true)?;
        for v in items {
            if self.truthy(v) {
                self.push(Val::bool(true));
                return Ok(());
            }
        }
        self.push(Val::bool(false));
        Ok(())
    }

    // Number formatting.

    pub fn call_bin(&mut self) -> Result<(), VmErr> { self.call_radix(2,  "0b") }
    pub fn call_oct(&mut self) -> Result<(), VmErr> { self.call_radix(8,  "0o") }
    pub fn call_hex(&mut self) -> Result<(), VmErr> { self.call_radix(16, "0x") }

    fn call_radix(&mut self, radix: u32, prefix: &str) -> Result<(), VmErr> {
        let o = self.pop()?;
        let s = self.int_to_radix_string(o, radix, prefix)?;
        self.alloc_and_push_str(s)
    }

    /* Converts int to "<prefix><digits>" with optional sign. */
    fn int_to_radix_string(&self, v: Val, radix: u32, prefix: &str) -> Result<String, VmErr> {
        if v.is_int() {
            return Ok(i64_to_radix(v.as_int(), radix, prefix));
        }
        if v.is_bool() {
            return Ok(i64_to_radix(v.as_bool() as i64, radix, prefix));
        }
        Err(cold_type("integer required"))
    }

    // Arithmetic.

    pub fn call_divmod(&mut self) -> Result<(), VmErr> {
        let b = self.pop()?;
        let a = self.pop()?;
        let (Some(ai), Some(bi)) = (self.as_i64(a), self.as_i64(b))
            else { return Err(cold_type("divmod() requires integer operands")); };
        if bi == 0 { return Err(VmErr::ZeroDiv); }
        let q = ai.checked_div(bi).ok_or(cold_overflow())?;
        let r = ai - q * bi;
        // Floor-div sign correction so divmod matches `(a // b, a % b)`.
        let (q, r) = if (r != 0) && ((r < 0) != (bi < 0)) { (q - 1, r + bi) } else { (q, r) };
        let qv = self.int_or_overflow(Some(q))?;
        let rv = self.int_or_overflow(Some(r))?;
        self.alloc_and_push_tuple(vec![qv, rv])
    }

    pub fn call_pow(&mut self, op: u16) -> Result<(), VmErr> {
        let args = self.pop_n(op as usize)?;
        match args.len() {
            2 => {
                let r = self.pow_2arg(args[0], args[1])?;
                self.push(r);
                Ok(())
            }
            3 => {
                // Modular exponentiation: (a ** b) % c on i64.
                let (Some(base), Some(modulus)) =
                    (self.as_i64(args[0]), self.as_i64(args[2]))
                    else { return Err(cold_type("pow() with 3 args requires integers")); };
                if !args[1].is_int() {
                    return Err(cold_type("pow() with 3 args requires integer exponent"));
                }
                let mut e = args[1].as_int();
                if e < 0 { return Err(cold_value("pow() exponent must be non-negative")); }
                if modulus == 0 { return Err(VmErr::ZeroDiv); }

                let m = (modulus as i128).abs();
                let mut result = 1i128;
                let mut b = (base as i128).rem_euclid(m);
                while e > 0 {
                    if e & 1 == 1 {
                        result = (result * b).rem_euclid(m);
                    }
                    b = (b * b).rem_euclid(m);
                    e >>= 1;
                }
                let r = self.int_or_overflow(Some(result as i64))?;
                self.push(r);
                Ok(())
            }
            _ => Err(cold_type("pow() takes 2 or 3 arguments")),
        }
    }

    fn pow_2arg(&mut self, a: Val, b: Val) -> Result<Val, VmErr> {
        self.pow_vals(a, b, "pow() requires numeric operands")
    }

    /* Two-arg power, shared between the pow() builtin and the `**` operator
       handler. Integer ** non-negative integer stays i64 with overflow trap;
       floats and negative exponents promote to f64. */
    pub(crate) fn pow_vals(&mut self, a: Val, b: Val, err_msg: &'static str) -> Result<Val, VmErr> {
        if let (Some(ai), true) = (self.as_i64(a), b.is_int()) {
            let exp = b.as_int();
            if exp >= 0 {
                // Exponentiation by squaring on i64. Overflow at any step traps
                // as OverflowError; bases ±1/0 finish without overflowing even
                // with very large exponents.
                let mut result: i64 = 1;
                let mut base = ai;
                let mut e = exp;
                while e > 0 {
                    if e & 1 == 1 {
                        result = result.checked_mul(base).ok_or(cold_overflow())?;
                    }
                    e >>= 1;
                    if e > 0 {
                        base = base.checked_mul(base).ok_or(cold_overflow())?;
                    }
                }
                return self.int_or_overflow(Some(result));
            }
            return Ok(Val::float(fpowi(ai as f64, exp as i32)));
        }
        let to_f = |v: Val| -> Result<f64, VmErr> {
            if v.is_int() { Ok(v.as_int() as f64) }
            else if v.is_float() { Ok(v.as_float()) }
            else { Err(cold_type(err_msg)) }
        };
        Ok(Val::float(fpowf(to_f(a)?, to_f(b)?)))
    }

    // Introspection.

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

    // Sequence ops.

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

    // format(value [, spec]).

    pub fn call_format(&mut self, op: u16) -> Result<(), VmErr> {
        if op != 1 && op != 2 {
            return Err(cold_type("format() takes 1 or 2 arguments"));
        }
        let spec_val = if op == 2 { Some(self.pop()?) } else { None };
        let val = self.pop()?;
        let result = match spec_val {
            Some(sv) => {
                let spec = match self.heap.get(sv) {
                    HeapObj::Str(s) => s.clone(),
                    _ => return Err(cold_type("format() spec must be a string")),
                };
                super::handlers::format::format_value(val, &spec, &self.heap)
                    .map_err(cold_value)?
            }
            None => self.display(val),
        };
        self.alloc_and_push_str(result)
    }

    // getattr(obj, name [, default]).

    pub fn call_getattr(&mut self, op: u16) -> Result<(), VmErr> {
        if op != 2 && op != 3 {
            return Err(cold_type("getattr() takes 2 or 3 arguments"));
        }
        let default = if op == 3 { Some(self.pop()?) } else { None };
        let name = self.expect_str_arg("getattr() name must be a string")?;
        let obj = self.pop()?;

        let ty = self.type_name(obj);
        if let Some(method_id) = super::handlers::methods::lookup_method(ty, &name) {
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

    /* globals() — module-level bindings as a dict. Combines the VM's
       `globals` HashMap (builtins, types, module references) with the
       entry-chunk slots (user-defined top-level names, which live in
       chunk slots rather than the globals map). Returned dict is a
       *copy* — mutating it does not change the VM. */
    pub fn call_globals(
        &mut self, chunk: &crate::modules::parser::SSAChunk, slots: &[Val],
    ) -> Result<(), VmErr> {
        // Builtin/type/module pairs from self.globals, deduped to bare names.
        let mut out: crate::modules::fx::FxHashMap<String, Val> =
            crate::modules::fx::FxHashMap::default();
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
            let bare = match name.rfind('_') {
                Some(p) if name[p + 1..].chars().all(|c| c.is_ascii_digit()) =>
                    name[..p].to_string(),
                _ => name.clone(),
            };
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
        let mut latest: crate::modules::fx::FxHashMap<String, (i64, Val)> =
            crate::modules::fx::FxHashMap::default();
        for (i, name) in chunk.names.iter().enumerate() {
            let v = match slots.get(i) {
                Some(v) if !v.is_undef() => *v,
                _ => continue,
            };
            // Synthetic slots (`#match0`, `#match_item0`) are matcher
            // scratch — never user-visible.
            if name.starts_with('#') { continue; }
            // Strip SSA version suffix.
            let (bare, ver) = match name.rfind('_') {
                Some(p) if name[p + 1..].chars().all(|c| c.is_ascii_digit()) =>
                    (&name[..p], name[p + 1..].parse::<i64>().unwrap_or(0)),
                _ => (name.as_str(), 0),
            };
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

    /* frozenset() | frozenset(iter) — construct an immutable, hashable
       set from an iterable. Without args returns the empty frozenset. */
    pub fn call_frozenset(&mut self, argc: u16) -> Result<(), VmErr> {
        let args = self.pop_n(argc as usize)?;
        let items: Vec<Val> = match args.len() {
            0 => Vec::new(),
            1 => self.iter_to_vec_general(args[0])?,
            _ => return Err(cold_type("frozenset() takes 0 or 1 argument")),
        };
        let mut s = HashSet::with_capacity_and_hasher(items.len(), Default::default());
        for v in items { s.insert(v); }
        let v = self.heap.alloc(HeapObj::FrozenSet(Rc::new(s)))?;
        self.push(v); Ok(())
    }

    /* bytes_fromhex(s) — decode a hex string into bytes. Whitespace is
       tolerated (matches CPython's bytes.fromhex). Errors on odd length or
       non-hex characters. Exposed as a free builtin since Edge Python has
       no class methods (`bytes.fromhex` is the CPython spelling). */
    pub fn call_bytes_fromhex(&mut self) -> Result<(), VmErr> {
        let v = self.pop()?;
        let s = match self.heap.get(v) {
            HeapObj::Str(s) => s.clone(),
            _ => return Err(cold_type("bytes_fromhex() argument must be a string")),
        };
        let cleaned: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        if !cleaned.len().is_multiple_of(2) {
            return Err(cold_value("non-hexadecimal number or odd length"));
        }
        let mut out = Vec::with_capacity(cleaned.len() / 2);
        let bytes = cleaned.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let hi = (bytes[i] as char).to_digit(16)
                .ok_or(cold_value("non-hexadecimal digit found"))?;
            let lo = (bytes[i + 1] as char).to_digit(16)
                .ok_or(cold_value("non-hexadecimal digit found"))?;
            out.push(((hi << 4) | lo) as u8);
            i += 2;
        }
        let v = self.heap.alloc(HeapObj::Bytes(out))?;
        self.push(v); Ok(())
    }

    /* int_from_bytes(b, byteorder) — parse a bytes value as an integer.
       byteorder is "big" or "little"; signedness is unsigned (CPython's
       default). Range check against the 47-bit Val cap; OverflowError if
       out of range. */
    pub fn call_int_from_bytes(&mut self) -> Result<(), VmErr> {
        let order = self.pop()?;
        let v = self.pop()?;
        let buf = match self.heap.get(v) {
            HeapObj::Bytes(b) => b.clone(),
            _ => return Err(cold_type("int_from_bytes() first arg must be bytes")),
        };
        let order_s = match self.heap.get(order) {
            HeapObj::Str(s) => s.clone(),
            _ => return Err(cold_type("int_from_bytes() byteorder must be 'big' or 'little'")),
        };
        if buf.len() > 8 { return Err(cold_overflow()); }
        let big = match order_s.as_str() {
            "big" => true,
            "little" => false,
            _ => return Err(cold_value("byteorder must be 'big' or 'little'")),
        };
        let mut acc: u64 = 0;
        if big {
            for &b in &buf { acc = (acc << 8) | b as u64; }
        } else {
            for (i, &b) in buf.iter().enumerate() { acc |= (b as u64) << (i * 8); }
        }
        if acc > Val::INT_MAX as u64 { return Err(cold_overflow()); }
        self.push(Val::int(acc as i64));
        Ok(())
    }

    /* int_to_bytes(n, length, byteorder) — encode a non-negative int into
       a bytes of given length. Errors if the value doesn't fit. Negative
       values aren't supported (CPython supports them via two's complement,
       but the use case here is wire protocols where producers control
       sign). */
    pub fn call_int_to_bytes(&mut self) -> Result<(), VmErr> {
        let order = self.pop()?;
        let length = self.pop()?;
        let n = self.pop()?;
        if !n.is_int() { return Err(cold_type("int_to_bytes() value must be an int")); }
        let n = n.as_int();
        if !length.is_int() { return Err(cold_type("int_to_bytes() length must be an int")); }
        let length = length.as_int() as usize;
        if length > 8 { return Err(cold_value("int_to_bytes() length must be <= 8")); }
        if n < 0 { return Err(cold_value("int_to_bytes() requires a non-negative int")); }
        let order_s = match self.heap.get(order) {
            HeapObj::Str(s) => s.clone(),
            _ => return Err(cold_type("int_to_bytes() byteorder must be 'big' or 'little'")),
        };
        let big = match order_s.as_str() {
            "big" => true,
            "little" => false,
            _ => return Err(cold_value("byteorder must be 'big' or 'little'")),
        };
        let val = n as u64;
        if length < 8 && val >= (1u64 << (length * 8)) {
            return Err(cold_overflow());
        }
        let mut out = Vec::with_capacity(length);
        if big {
            for i in (0..length).rev() { out.push((val >> (i * 8) & 0xff) as u8); }
        } else {
            for i in 0..length { out.push((val >> (i * 8) & 0xff) as u8); }
        }
        let v = self.heap.alloc(HeapObj::Bytes(out))?;
        self.push(v); Ok(())
    }

    /* slice(stop) | slice(start, stop) | slice(start, stop, step). Constructs
       a HeapObj::Slice from up to three numeric arguments. Mirrors Python's
       slice() builtin; the result can be passed as a sequence index. */
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

    /* vars(obj) — Instance: copy of __dict__ as a dict; Module: dict from
       its attrs table. Like Python's vars(), no argument form is not
       supported (no module-level __dict__ accessor in Edge Python). */
    pub fn call_vars(&mut self) -> Result<(), VmErr> {
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

    // hasattr(obj, name).

    pub fn call_hasattr(&mut self) -> Result<(), VmErr> {
        let name = self.expect_str_arg("hasattr() name must be a string")?;
        let obj = self.pop()?;
        let ty = self.type_name(obj);
        let exists = super::handlers::methods::lookup_method(ty, &name).is_some();
        self.push(Val::bool(exists));
        Ok(())
    }

    /* Pops the top of stack and returns its String contents, or errors with
       `msg` if it is not a heap string. */
    fn expect_str_arg(&mut self, msg: &'static str) -> Result<String, VmErr> {
        let v = self.pop()?;
        if v.is_heap() && let HeapObj::Str(s) = self.heap.get(v) { return Ok(s.clone()); }
        Err(cold_type(msg))
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

    /* `bytes()` constructor — three forms, mirroring Python:
         bytes()                    → empty bytes
         bytes(n)        if int     → n zero bytes
         bytes(iter)     if iter    → bytes of those ints, each in 0..=255
         bytes(s, "utf-8")          → encode str s with the given encoding
       Encodings recognised: "utf-8", "utf8", "ascii". Anything else errors
       so silent encoding mismatches don't slip through. */
    pub fn call_bytes(&mut self, argc: u16) -> Result<(), VmErr> {
        let args = self.pop_n(argc as usize)?;
        let buf: Vec<u8> = match args.len() {
            0 => Vec::new(),
            1 => {
                let a = args[0];
                if a.is_int() {
                    let n = a.as_int();
                    if n < 0 { return Err(cold_value("negative count")); }
                    alloc::vec![0u8; n as usize]
                } else if a.is_heap() {
                    if let HeapObj::Bytes(b) = self.heap.get(a) {
                        b.clone()
                    } else {
                        let items = self.iter_to_vec_general(a)?;
                        let mut out = Vec::with_capacity(items.len());
                        for v in items {
                            if !v.is_int() {
                                return Err(cold_type("bytes() iterable must contain ints"));
                            }
                            let n = v.as_int();
                            if !(0..=255).contains(&n) {
                                return Err(cold_value("bytes must be in range(0, 256)"));
                            }
                            out.push(n as u8);
                        }
                        out
                    }
                } else {
                    return Err(cold_type("bytes() requires an int, an iterable of ints, or (str, encoding)"));
                }
            }
            2 => {
                // bytes(s, "utf-8") — string encoding form.
                let (s, enc) = (args[0], args[1]);
                let HeapObj::Str(text) = self.heap.get(s).clone() else {
                    return Err(cold_type("bytes() first argument must be a string when encoding is given"));
                };
                let HeapObj::Str(encoding) = self.heap.get(enc) else {
                    return Err(cold_type("bytes() encoding must be a string"));
                };
                match encoding.as_str() {
                    "utf-8" | "utf8" => text.into_bytes(),
                    "ascii" => {
                        if !text.is_ascii() {
                            return Err(cold_value("'ascii' codec can't encode non-ASCII characters"));
                        }
                        text.into_bytes()
                    }
                    _ => return Err(cold_value("unsupported encoding (expected 'utf-8' or 'ascii')")),
                }
            }
            _ => return Err(cold_type("bytes() takes at most 2 arguments")),
        };
        let v = self.heap.alloc(HeapObj::Bytes(buf))?;
        self.push(v); Ok(())
    }

    /* `import_module(name)` — look up an already-imported module by its
       runtime alias and return the `HeapObj::Module` Val. Lets a script
       choose which of several pre-imported modules to use at runtime
       without a manual dispatch dict:

           import prod_handler
           import dev_handler

           def handler(env):
               return import_module(env + "_handler").handle

       The candidate module must be statically imported elsewhere (so its
       lockfile entry is pinned and its top-level has run). Looking up
       a name not bound to a Module fails with TypeError; looking up a
       name that doesn't exist fails with NameError. Both errors carry
       the offending name so the diagnostic points at the bad arg.

       This is sugar over `globals()[name]` — there's no separate dynamic
       module registry. The static-import + runtime-dispatch pattern is
       deliberate: it preserves the lockfile + integrity guarantees that
       a true `__import__()` would break. */
    pub fn call_import_module(&mut self) -> Result<(), VmErr> {
        let spec = self.pop()?;
        if !spec.is_heap() {
            return Err(cold_type("import_module() argument must be a string"));
        }
        let name = match self.heap.get(spec) {
            HeapObj::Str(s) => s.clone(),
            _ => return Err(cold_type("import_module() argument must be a string")),
        };
        // The parser stores top-level bindings under both bare name and
        // `<name>_0` (SSA version 0); look up either form so users can
        // pass the natural alias they wrote in their `import` statement.
        let val = self.globals.get(&name)
            .or_else(|| self.globals.get(&s!(str &name, "_0")))
            .copied()
            .ok_or_else(|| VmErr::Name(s!(
                "module '", str &name, "' not imported in this scope")))?;
        if !val.is_heap() || !matches!(self.heap.get(val), HeapObj::Module(..)) {
            return Err(VmErr::TypeMsg(s!("'", str &name, "' is not a module")));
        }
        self.push(val); Ok(())
    }

    /* `iter(x)` — flatten any iterable into a fresh List that `next()`
       drains front-to-back. Eager; the original collection is never
       mutated. Mirrors the universal ABI's `Op::Iter` shape. */
    pub fn call_iter(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let items = self.iter_to_vec_general(o)?;
        self.alloc_and_push_list(items)
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

    /* Resume a suspended coroutine. On yield: persists ip/slots/stack/iters
       back into the Coroutine object and leaves self.yielded = true. On
       return: restores caller stack/iter state and self.yielded. */
    pub fn resume_coroutine(&mut self, callee: Val) -> Result<Val, VmErr> {
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
                let resume_ip = self.resume_ip;
                let remaining = self.stack.split_off(saved_stack_len);
                let coro_iters: Vec<super::types::IterFrame> = self.iter_stack.drain(saved_iter_len..).collect();
                if let HeapObj::Coroutine(sip, ss, sst, _, si) = self.heap.get_mut(callee) {
                    *sip = resume_ip;
                    *ss = fn_slots;
                    *sst = remaining;
                    *si = coro_iters;
                }
                Ok(result)
            } else {
                self.stack.truncate(saved_stack_len);
                self.iter_stack.truncate(saved_iter_len);
                self.yielded = saved_yielded;
                Ok(result)
            }
        } else {
            Err(cold_type("not a coroutine"))
        }
    }

    /* `run(*coros)` — drive a cooperative scheduler until the *first*
       argument finishes. Multiple positional args run concurrently;
       additional ones are still drained until they resolve so caller
       semantics match `gather`. Returns the first coroutine's result.

       Behaviour reference:
       - `Ready`     -> resume one step
       - `Sleeping`  -> wait (advance clock to min(until_ns) when all are)
       - `CancelPending` -> next resume raises CancelledError into the coro
       - `Done/Errored/Cancelled` -> terminal; reaped from the scheduler

       Errors in non-target coros are recorded on their handle and propagate
       only when the user explicitly awaits them via `gather`/`with_timeout`. */
    pub fn call_run(&mut self, argc: u16) -> Result<(), VmErr> {
        let tasks = self.pop_n(argc as usize)?;
        if tasks.is_empty() {
            self.push(Val::none());
            return Ok(());
        }
        let target = tasks[0];
        // Reset per-run virtual clock so deterministic tests don't drift.
        if self.time_hook.is_none() { self.virtual_clock_ns = 0; }
        for v in &tasks {
            if v.is_heap() && matches!(self.heap.get(*v), HeapObj::Coroutine(..)) {
                self.scheduler.push(super::types::CoroutineHandle {
                    coro: *v,
                    state: super::types::CoroState::Ready,
                });
            }
        }
        // Drain everything so concurrent coroutines all run to completion;
        // remember the target handle's result for the return value (single
        // arg: that coro's value; multi-arg: first arg's value, peers get
        // their own values dropped — same as `gather` semantics).
        self.run_until_all_done()?;
        let mut result = Val::none();
        if let Some(h) = self.scheduler.iter().find(|h| h.coro == target) {
            match &h.state {
                super::types::CoroState::Done(v) => result = *v,
                super::types::CoroState::Errored(e) => {
                    let e = e.clone();
                    self.scheduler.clear();
                    return Err(e);
                }
                _ => {}
            }
        }
        self.scheduler.clear();
        self.push(result);
        Ok(())
    }

    /* Drive the scheduler until every handle is in a terminal state
       (Done / Errored / Cancelled). Errors are recorded on the handle —
       only the caller decides whether to propagate them via target
       lookup or `gather`'s fail-fast semantics. */
    pub(crate) fn run_until_all_done(&mut self) -> Result<(), VmErr> {
        loop {
            let alive = self.scheduler.iter().any(|h| matches!(
                h.state,
                super::types::CoroState::Ready
                | super::types::CoroState::CancelPending
                | super::types::CoroState::Sleeping(_)
            ));
            if !alive { return Ok(()); }
            // Pick a Ready handle; if none, advance clock to the earliest
            // wakeup. If everyone is Done/Errored/Cancelled, bail out — we
            // already returned target above so this means target is gone.
            let mut next_ready: Option<usize> = None;
            let mut min_wake: Option<u64> = None;
            for (i, h) in self.scheduler.iter().enumerate() {
                match &h.state {
                    super::types::CoroState::Ready => { next_ready = Some(i); break; }
                    super::types::CoroState::CancelPending => { next_ready = Some(i); break; }
                    super::types::CoroState::Sleeping(w)
                        if min_wake.is_none_or(|m| *w < m) => { min_wake = Some(*w); }
                    _ => {}
                }
            }
            if let Some(i) = next_ready {
                self.scheduler_step(i)?;
                continue;
            }
            match min_wake {
                Some(w) => {
                    let now = self.now_ns();
                    // With a real clock we don't busy-wait: we still
                    // advance virtual_clock_ns logically so subsequent
                    // sleeps are relative to the new "now".
                    if w > now && self.time_hook.is_none() {
                        self.virtual_clock_ns = w;
                    }
                    let now = self.now_ns();
                    for h in self.scheduler.iter_mut() {
                        if let super::types::CoroState::Sleeping(w) = h.state
                            && w <= now
                        {
                            h.state = super::types::CoroState::Ready;
                        }
                    }
                }
                None => return Ok(()),
            }
        }
    }

    fn scheduler_step(&mut self, idx: usize) -> Result<(), VmErr> {
        let coro = self.scheduler[idx].coro;
        // CancelPending -> inject a CancelledError raise instead of resuming.
        if matches!(self.scheduler[idx].state, super::types::CoroState::CancelPending) {
            self.scheduler[idx].state = super::types::CoroState::Cancelled;
            return Ok(());
        }
        // Snapshot before resume so a yield during sleep() can read it.
        self.pending_sleep_until_ns = None;
        let result = self.resume_coroutine(coro);
        let yielded = self.yielded;
        self.yielded = false;
        let new_state = match result {
            Err(e) => super::types::CoroState::Errored(e),
            Ok(v) if yielded => {
                // sleep() set pending_sleep_until_ns; receive() may also
                // park indefinitely (we keep it Ready and re-drain queue).
                if let Some(until) = self.pending_sleep_until_ns.take() {
                    super::types::CoroState::Sleeping(until)
                } else {
                    let _ = v;
                    super::types::CoroState::Ready
                }
            }
            Ok(v) => super::types::CoroState::Done(v),
        };
        self.scheduler[idx].state = new_state;
        Ok(())
    }

    /* Suspend until `s` real seconds elapse. With a host time_hook installed
       this becomes a wall-clock wait managed by the scheduler; without one
       it advances a per-run virtual clock so coroutines still yield in
       order. Negative or non-numeric `s` sleeps zero (single yield). */
    pub fn call_sleep(&mut self) -> Result<(), VmErr> {
        let n = self.pop()?;
        let secs: f64 = if n.is_int() { n.as_int() as f64 }
                        else if n.is_float() { n.as_float() }
                        else if n.is_bool() { n.as_bool() as i64 as f64 }
                        else { 0.0 };
        let secs = if secs < 0.0 { 0.0 } else { secs };
        let until = self.now_ns().saturating_add((secs * 1_000_000_000.0) as u64);
        self.pending_sleep_until_ns = Some(until);
        // Push None as the yield value; the scheduler ignores it.
        self.push(Val::none());
        self.yielded = true;
        Ok(())
    }

    /* gather(*coros) — concurrent fan-out. Adds every argument to the
       running scheduler, drains until each is terminal, then returns a
       list of their results in argument order. If any errors, peers are
       cancelled and the first error propagates (CPython asyncio style). */
    pub fn call_gather(&mut self, argc: u16) -> Result<(), VmErr> {
        let tasks = self.pop_n(argc as usize)?;
        let coros: Vec<Val> = tasks.into_iter()
            .filter(|v| v.is_heap() && matches!(self.heap.get(*v), HeapObj::Coroutine(..)))
            .collect();
        for v in &coros {
            self.scheduler.push(super::types::CoroutineHandle {
                coro: *v,
                state: super::types::CoroState::Ready,
            });
        }
        self.run_until_all_done()?;
        // Cancel-rest-and-raise on first error.
        let mut first_err: Option<VmErr> = None;
        for v in &coros {
            if let Some(h) = self.scheduler.iter().find(|h| h.coro == *v)
                && let super::types::CoroState::Errored(e) = &h.state
            {
                first_err = Some(e.clone());
                break;
            }
        }
        let mut results = Vec::with_capacity(coros.len());
        for v in &coros {
            let res = self.scheduler.iter().find(|h| h.coro == *v)
                .map(|h| match &h.state {
                    super::types::CoroState::Done(r) => *r,
                    _ => Val::none(),
                }).unwrap_or(Val::none());
            results.push(res);
        }
        // Drop only the gather'd handles — leave any unrelated scheduler
        // entries (set up by an outer run()) alone.
        self.scheduler.retain(|h| !coros.contains(&h.coro));
        if let Some(e) = first_err { return Err(e); }
        self.alloc_and_push_list(results)
    }

    /* with_timeout(seconds, coro) — adds the coroutine to the scheduler,
       drains it until terminal or `seconds` elapse. On timeout the
       coroutine is cancelled and a TimeoutError is raised. */
    pub fn call_with_timeout(&mut self) -> Result<(), VmErr> {
        let coro = self.pop()?;
        let secs_v = self.pop()?;
        if !(coro.is_heap() && matches!(self.heap.get(coro), HeapObj::Coroutine(..))) {
            return Err(cold_type("with_timeout() requires a coroutine"));
        }
        let secs: f64 = if secs_v.is_int() { secs_v.as_int() as f64 }
                        else if secs_v.is_float() { secs_v.as_float() }
                        else { return Err(cold_type("with_timeout() seconds must be a number")); };
        let deadline = self.now_ns().saturating_add((secs.max(0.0) * 1_000_000_000.0) as u64);
        self.scheduler.push(super::types::CoroutineHandle {
            coro, state: super::types::CoroState::Ready,
        });
        // Drive one step at a time so the deadline check stays tight.
        let mut timed_out = false;
        while let Some(idx) = self.scheduler.iter().position(|h| h.coro == coro) {
            match self.scheduler[idx].state.clone() {
                super::types::CoroState::Done(_)
                | super::types::CoroState::Errored(_)
                | super::types::CoroState::Cancelled => break,
                super::types::CoroState::Sleeping(until) => {
                    // Coro asked to sleep past our deadline -> time out now.
                    if until >= deadline {
                        self.scheduler[idx].state = super::types::CoroState::CancelPending;
                        timed_out = true;
                        self.scheduler_step(idx)?;
                        break;
                    }
                    // Sleep wakes before deadline -> advance clock & wake.
                    if self.time_hook.is_none() && until > self.virtual_clock_ns {
                        self.virtual_clock_ns = until;
                    }
                    self.scheduler[idx].state = super::types::CoroState::Ready;
                }
                _ => {}
            }
            if self.now_ns() >= deadline {
                self.scheduler[idx].state = super::types::CoroState::CancelPending;
                timed_out = true;
                self.scheduler_step(idx)?;
                break;
            }
            self.scheduler_step(idx)?;
        }
        let result = self.scheduler.iter().find(|h| h.coro == coro)
            .map(|h| match &h.state {
                super::types::CoroState::Done(v) => Ok(*v),
                super::types::CoroState::Errored(e) => Err(e.clone()),
                _ => Ok(Val::none()),
            }).unwrap_or(Ok(Val::none()));
        self.scheduler.retain(|h| h.coro != coro);
        if timed_out { return Err(VmErr::Raised("TimeoutError".into())); }
        let v = result?;
        self.push(v); Ok(())
    }

    /* cancel(coro) — flag the coroutine for cancellation. The next time
       the scheduler resumes it, a CancelledError raise is injected. If
       the coroutine isn't currently registered with the scheduler the
       call is a no-op (cancellation only applies inside `run`/`gather`/
       `with_timeout`). */
    pub fn call_cancel(&mut self) -> Result<(), VmErr> {
        let coro = self.pop()?;
        if let Some(h) = self.scheduler.iter_mut().find(|h| h.coro == coro) {
            h.state = super::types::CoroState::CancelPending;
        }
        self.push(Val::none()); Ok(())
    }

    /* Pop the oldest queued message, or yield None to signal "still waiting". */
    pub fn call_receive(&mut self) -> Result<(), VmErr> {
        if !self.event_queue.is_empty() {
            let val = self.event_queue.remove(0);
            self.push(val);
        } else {
            self.push(Val::none());
            self.yielded = true;
        }
        Ok(())
    }
}