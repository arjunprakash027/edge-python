use crate::s;

use super::VM;
use super::types::*;

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
        let items: Vec<Val> = if o.is_heap() && let HeapObj::Tuple(v) = self.heap.get(o) { v.clone() }
            else if o.is_heap() && let HeapObj::List(v) = self.heap.get(o) { v.borrow().clone() }
            else { return Err(cold_type("tuple() argument must be iterable")); };
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

        // For exception matching: when `obj` is a Type itself, compare names.
        let obj_type_name: Option<String> = if obj.is_heap() {
            if let HeapObj::Type(n) = self.heap.get(obj) { Some(n.clone()) } else { None }
        } else { None };

        let check_one = |t: Val, heap: &HeapPool| -> Result<bool, VmErr> {
            if !t.is_heap() {
                return Err(VmErr::Type("isinstance() arg 2 must be a type or tuple of types"));
            }
            match heap.get(t) {
                HeapObj::Type(name) => Ok(
                    name == obj_ty
                    || (obj_ty == "bool" && name == "int")
                    || obj_type_name.as_deref() == Some(name.as_str())
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
                _ => {}
            }
        }
        Ok(args)
    }

    /* Extract a Vec<Val> from list/tuple/set; optionally materialise Range.
       Str is handled at the call site (it needs heap-allocated chars, not ints). */
    fn extract_iter(&self, o: Val, include_range: bool) -> Result<Vec<Val>, VmErr> {
        if !o.is_heap() {
            return Err(VmErr::TypeMsg(s!("'", str self.type_name(o), "' object is not iterable")));
        }
        Ok(match self.heap.get(o) {
            HeapObj::List(v)  => v.borrow().clone(),
            HeapObj::Tuple(v) => v.clone(),
            HeapObj::Set(v)   => v.borrow().iter().cloned().collect(),
            HeapObj::Range(s, e, st) if include_range => {
                let (mut cur, end, step) = (*s, *e, *st);
                let mut out = Vec::new();
                if step > 0 { while cur < end { out.push(Val::int(cur)); cur += step; } }
                else        { while cur > end { out.push(Val::int(cur)); cur += step; } }
                out
            }
            _ => return Err(VmErr::TypeMsg(s!("'", str self.type_name(o), "' object is not iterable"))),
        })
    }

    fn alloc_set(&mut self, items: Vec<Val>) -> Result<Val, VmErr> {
        let mut set = HashSet::with_capacity_and_hasher(items.len(), Default::default());
        for v in items { set.insert(v); }
        self.heap.alloc(HeapObj::Set(Rc::new(RefCell::new(set))))
    }

    pub fn build_set(&mut self, op: u16) -> Result<(), VmErr> {
        let items = self.pop_n(op as usize)?;
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
            if !o.is_heap() { return Err(cold_type("set() argument must be iterable")); }
            let src: Vec<Val> = match self.heap.get(o) {
                HeapObj::List(v)  => v.borrow().clone(),
                HeapObj::Tuple(v) => v.clone(),
                HeapObj::Set(v)   => v.borrow().iter().cloned().collect(),
                HeapObj::Str(s)   => { let s = s.clone(); self.str_to_char_vals(&s)? },
                _ => return Err(cold_type("set() argument must be iterable")),
            };
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

    // ascii(obj) — repr but with non-ASCII escaped.

    pub fn call_ascii(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let r = self.repr(o);
        let mut out = String::with_capacity(r.len());
        for c in r.chars() {
            if (c as u32) < 0x80 { out.push(c); continue; }
            let (escape, pad) = if (c as u32) < 0x10000 { ("\\u", 4) } else { ("\\U", 8) };
            out.push_str(escape);
            let hex = i64_to_radix(c as i64, 16, "");
            for _ in 0..(pad - hex.len()) { out.push('0'); }
            out.push_str(&hex);
        }
        self.alloc_and_push_str(out)
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

    /* Round-robin coroutine scheduler. Queue items are (coro, sleep_left);
       each tick decrements sleep, otherwise resumes one step. A negative
       yielded int is interpreted as a sleep request in cycles (see call_sleep). */
    pub fn call_run(&mut self, argc: u16) -> Result<(), VmErr> {
        let tasks = self.pop_n(argc as usize)?;
        let mut queue: Vec<(Val, i64)> = tasks.into_iter()
            .filter(|v| v.is_heap() && matches!(self.heap.get(*v), HeapObj::Coroutine(..)))
            .map(|v| (v, 0))
            .collect();

        let mut max_cycles = 10_000_000u64;
        while !queue.is_empty() && max_cycles > 0 {
            max_cycles -= 1;
            let mut next_queue: Vec<(Val, i64)> = Vec::new();

            for (coro, sleep) in queue {
                if sleep > 0 {
                    next_queue.push((coro, sleep - 1));
                    continue;
                }
                let result = self.resume_coroutine(coro)?;
                let was_yielded = self.yielded;
                self.yielded = false;

                if was_yielded {
                    let new_sleep = if result.is_int() && result.as_int() < 0 {
                        -result.as_int()
                    } else { 0 };
                    next_queue.push((coro, new_sleep));
                }
                // Otherwise the coroutine finished; drop it from the queue.
            }
            queue = next_queue;
        }

        self.push(Val::none());
        Ok(())
    }

    /* Yield a negative int as a sleep marker for the scheduler. */
    pub fn call_sleep(&mut self) -> Result<(), VmErr> {
        let n = self.pop()?;
        let cycles = if n.is_int() { n.as_int().max(0) } else { 0 };
        self.push(Val::int(-cycles));
        self.yielded = true;
        Ok(())
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