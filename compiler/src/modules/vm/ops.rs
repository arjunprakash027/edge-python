use crate::s;

use super::types::*;
use crate::modules::parser::types::OpCode;

use alloc::{string::{String, ToString}, vec::Vec, rc::Rc};
use core::cell::RefCell;

/* Render `bytes` as `b'...'` matching CPython's repr (printable ASCII verbatim, rest escaped). */
fn format_bytes(buf: &[u8]) -> String {
    let mut out = String::with_capacity(buf.len() + 3);
    out.push_str("b'");
    for &b in buf {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7E => out.push(b as char),
            _ => {
                out.push_str("\\x");
                const HEX: &[u8; 16] = b"0123456789abcdef";
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0x0F) as usize] as char);
            }
        }
    }
    out.push('\'');
    out
}

/* Coerce a numeric pair to f64; returns None if neither operand is a float. */
fn coerce_floats(a: Val, b: Val) -> Option<(f64, f64)> {
    if !a.is_float() && !b.is_float() { return None; }
    let extract_f64 = |v: &Val| -> Option<f64> {
        if v.is_float() { Some(v.as_float()) }
        else if v.is_int() { Some(v.as_int() as f64) }
        else { None }
    };
    Some((extract_f64(&a)?, extract_f64(&b)?))
}

/* Record heap type tags so the IC can promote a stable binop to FastOp. */
macro_rules! cached_binop {
    ($heap:expr, $rip:expr, $opcode:expr, $a:expr, $b:expr, $cache:expr) => {{
        let ta = $heap.val_tag($a);
        let tb = $heap.val_tag($b);
        $cache.record($rip, $opcode, ta, tb);
    }};
}
pub(crate) use cached_binop;

use super::VM;

impl<'a> VM<'a> {
    pub fn truthy(&self, v: Val) -> bool {
        if v.is_none() || v.is_false() { return false; }
        if v.is_true() { return true; }
        if v.is_int() { return v.as_int() != 0; }
        if v.is_float() { return v.as_float() != 0.0; }
        match self.heap.get(v) {
            HeapObj::Str(s) => !s.is_empty(),
            HeapObj::Bytes(b) => !b.is_empty(),
            HeapObj::LongInt(i) => *i != 0,
            HeapObj::List(l) => !l.borrow().is_empty(),
            HeapObj::Tuple(t) => !t.is_empty(),
            HeapObj::Dict(d) => !d.borrow().is_empty(),
            HeapObj::Set(s) => !s.borrow().is_empty(),
            HeapObj::FrozenSet(s) => !s.is_empty(),
            HeapObj::Range(s,e,st) => if *st > 0 { s < e } else { s > e },
            HeapObj::Type(_) => true,
            HeapObj::Func(_, _, _) => true,
            HeapObj::Slice(..) => true,
            HeapObj::BoundMethod(..) => true,
            HeapObj::NativeFn(_) => true,
            HeapObj::Class(..) => true,
            HeapObj::BoundUserMethod(..) => true,
            HeapObj::Instance(..) => true,
            HeapObj::Coroutine(..) => true,
            HeapObj::Module(..) => true,
            HeapObj::Extern(_) => true,
            HeapObj::ExcInstance(..) => true,
            HeapObj::Ellipsis => true,
        }
    }

    pub fn bitwise_op(&mut self, a: Val, b: Val, op: impl Fn(i128, i128) -> i128) -> Result<Val, VmErr> {
        let ai = as_i128(a, &self.heap).ok_or(cold_type("bitwise op requires integer operands"))?;
        let bi = as_i128(b, &self.heap).ok_or(cold_type("bitwise op requires integer operands"))?;
        let r = op(ai, bi);
        self.int_to_val(Some(r))
    }

    /* Set bitwise ops (|, &, ^); caller has verified both operands are sets. */
    pub(crate) fn set_binop_and_push(&mut self, a: Val, b: Val, op: OpCode) -> Result<(), VmErr> {
        let (sa, sb) = match (self.heap.get(a), self.heap.get(b)) {
            (HeapObj::Set(x), HeapObj::Set(y)) => (x.borrow().clone(), y.borrow().clone()),
            _ => return Err(cold_runtime("set_binop on non-set operands")),
        };
        let items: Vec<Val> = match op {
            OpCode::BitOr => sa.union(&sb).copied().collect(),
            OpCode::BitAnd => sa.intersection(&sb).copied().collect(),
            OpCode::BitXor => sa.symmetric_difference(&sb).copied().collect(),
            _ => return Err(cold_runtime("set_binop with non-bitwise opcode")),
        };
        self.alloc_and_push_set(items)
    }

    /* Set comparisons with subset/superset semantics; both sides verified as sets. */
    pub(crate) fn set_compare_and_push(&mut self, a: Val, b: Val, op: OpCode) -> Result<(), VmErr> {
        let (sa, sb) = match (self.heap.get(a), self.heap.get(b)) {
            (HeapObj::Set(x), HeapObj::Set(y)) => (x.borrow(), y.borrow()),
            _ => return Err(cold_runtime("set_compare on non-set operands")),
        };
        let result = match op {
            OpCode::Eq => *sa == *sb,
            OpCode::NotEq => *sa != *sb,
            OpCode::Lt => sa.is_subset(&sb) && *sa != *sb,
            OpCode::LtEq => sa.is_subset(&sb),
            OpCode::Gt => sb.is_subset(&sa) && *sa != *sb,
            OpCode::GtEq => sb.is_subset(&sa),
            _ => return Err(cold_runtime("set_compare with non-compare opcode")),
        };
        drop(sa); drop(sb);
        self.push(Val::bool(result));
        Ok(())
    }

    pub fn type_name(&self, v: Val) -> &'static str {
        if v.is_bool() { "bool" }
        else if v.is_int() { "int" }
        else if v.is_float() { "float" }
        else if v.is_none() { "NoneType" }
        else { match self.heap.get(v) {
            HeapObj::Str(_) => "str",
            HeapObj::Bytes(_) => "bytes",
            HeapObj::LongInt(_) => "int",
            HeapObj::List(_) => "list",
            HeapObj::Dict(_) => "dict",
            HeapObj::Set(_) => "set",
            HeapObj::FrozenSet(_) => "frozenset",
            HeapObj::Tuple(_) => "tuple",
            HeapObj::Func(_, _, _) => "function",
            HeapObj::Type(_) => "type",
            HeapObj::Range(..) => "range",
            HeapObj::Slice(..) => "slice",
            HeapObj::BoundMethod(..) => "builtin_function_or_method",
            HeapObj::NativeFn(_) => "builtin_function_or_method",
            HeapObj::Class(_name, _) => "type",
            HeapObj::BoundUserMethod(_, _) => "<bound method>",
            HeapObj::Instance(..) => "object",
            HeapObj::Coroutine(..) => "coroutine",
            HeapObj::Module(..) => "module",
            HeapObj::Extern(_) => "builtin_function_or_method",
            HeapObj::ExcInstance(..) => "exception",
            HeapObj::Ellipsis => "ellipsis",
        }}
    }

    fn append_reprs<'b>(&self, out: &mut String, it: impl Iterator<Item = &'b Val>) {
        let mut first = true;
        for v in it { if !first { out.push_str(", "); } out.push_str(&self.repr(*v)); first = false; }
    }

    pub fn display(&self, v: Val) -> String {
        if v.is_int() { let mut b = itoa::Buffer::new(); return b.format(v.as_int()).into(); }
        if v.is_float() {
            let f = v.as_float();
            if f == 0.0 && f.is_sign_negative() {
                return "-0.0".into();
            }
            const I64_UPPER: f64 = i64::MAX as f64;
            if f.is_finite() && f >= (i64::MIN as f64) && f < I64_UPPER && f == (f as i64) as f64 {
                let i = f as i64;
                let mut b = itoa::Buffer::new();
                if !(Val::INT_MIN..=Val::INT_MAX).contains(&i) { return b.format(i).into(); }
                let mut s = String::new(); s.push_str(b.format(i)); s.push_str(".0"); return s;
            }
            return crate::util::fstr::format_f64(f);
        }
        if v.is_true() { return "True".into(); }
        if v.is_false() { return "False".into(); }
        if v.is_none() { return "None".into(); }
        match self.heap.get(v) {
            HeapObj::Str(s) => s.clone(),
            HeapObj::Bytes(b) => format_bytes(b),
            HeapObj::LongInt(i) => i128_to_dec(*i),
            HeapObj::Type(name) => s!("<class '", str name, "'>"),
            HeapObj::Func(i,_,_) => s!("<function ", int *i),
            HeapObj::Slice(s,e,st) => s!("slice(", str &self.display(*s), ", ", str &self.display(*e), ", ", str &self.display(*st), ")"),
            HeapObj::Range(s,e,st) => if *st == 1 { s!("range(", int *s, ", ", int *e, ")") } else { s!("range(", int *s, ", ", int *e, ", ", int *st, ")") },
            HeapObj::List(l) => { let mut o = s!(cap: 32; "["); self.append_reprs(&mut o, l.borrow().iter()); o.push(']'); o },
            HeapObj::Tuple(t) => if t.len() == 1 { s!("(", str &self.repr(t[0]), ",)") } else { let mut o = s!(cap: 32; "("); self.append_reprs(&mut o, t.iter()); o.push(')'); o },
            HeapObj::Dict(d) => { let mut o = s!(cap: 32; "{"); for (i,(k,v)) in d.borrow().iter().enumerate() { if i>0 { o.push_str(", "); } o.push_str(&self.repr(k)); o.push_str(": "); o.push_str(&self.repr(v)); } o.push('}'); o },
            HeapObj::BoundMethod(_, id) => s!("<built-in method ", str id.name(), ">"),
            HeapObj::NativeFn(id) => s!("<built-in function ", str id.name(), ">"),
            HeapObj::Class(name, _) => crate::s!("<class '", str name, "'>"  ),
            HeapObj::Instance(cls, _) => {
                if cls.is_heap() && let HeapObj::Class(name, _) = self.heap.get(*cls) { return crate::s!("<", str name, " instance>"); }
                "<instance>".into()
            }
            HeapObj::BoundUserMethod(_, _) => "<bound method>".into(),
            HeapObj::Coroutine(..) => "<coroutine>".into(),
            HeapObj::Module(name, _) => s!("<module '", str name, "'>"),
            HeapObj::Extern(f) => s!("<extern function ", str &f.name, ">"),
            HeapObj::ExcInstance(name, args) => {
                // `str(E("x"))` -> "x"; `repr(...)` handled elsewhere.
                if args.len() == 1 {
                    self.display(args[0])
                } else if args.is_empty() {
                    name.clone()
                } else {
                    let mut o = s!(cap: 32; str name, "(");
                    self.append_reprs(&mut o, args.iter());
                    o.push(')');
                    o
                }
            }
            HeapObj::Set(s) => {
                let mut items: Vec<Val> = s.borrow().iter().cloned().collect();
                if items.is_empty() { return "set()".into(); }
                self.sort_set_items(&mut items);
                let mut out = String::new();
                out.push('{');
                self.append_reprs(&mut out, items.iter());
                out.push('}');
                out
            }
            HeapObj::FrozenSet(s) => {
                let mut items: Vec<Val> = s.iter().cloned().collect();
                if items.is_empty() { return "frozenset()".into(); }
                self.sort_set_items(&mut items);
                let mut out = String::from("frozenset({");
                self.append_reprs(&mut out, items.iter());
                out.push_str("})");
                out
            }
            HeapObj::Ellipsis => "Ellipsis".into(),
        }
    }

    pub fn repr(&self, v: Val) -> String {
        if v.is_heap() && let HeapObj::Str(s) = self.heap.get(v) { return s!("'", str s, "'"); }
        self.display(v)
    }

    // Stable set ordering: numerics ascending, then non-numerics by repr.
    pub(crate) fn sort_set_items(&self, items: &mut [Val]) {
        items.sort_by(|a, b| {
            match (a.is_int() || a.is_float(), b.is_int() || b.is_float()) {
                (true, true) => {
                    let fa = if a.is_int() { a.as_int() as f64 } else { a.as_float() };
                    let fb = if b.is_int() { b.as_int() as f64 } else { b.as_float() };
                    fa.partial_cmp(&fb).unwrap_or(core::cmp::Ordering::Equal)
                }
                (true, false) => core::cmp::Ordering::Less,
                (false, true) => core::cmp::Ordering::Greater,
                (false, false) => self.repr(*a).cmp(&self.repr(*b)),
            }
        });
    }

    pub fn lt_vals(&self, a: Val, b: Val) -> Result<bool, VmErr> {
        let a = if a.is_bool() { Val::int(a.as_bool() as i64) } else { a };
        let b = if b.is_bool() { Val::int(b.as_bool() as i64) } else { b };
        if a.is_int() && b.is_int() { return Ok(a.as_int() < b.as_int()); }
        if let Some((af, bf)) = coerce_floats(a, b) { return Ok(af < bf); }
        // Wide-int compare in i128; falls through when either side isn't int-like.
        if let (Some(ai), Some(bi)) = (as_i128(a, &self.heap), as_i128(b, &self.heap)) { return Ok(ai < bi); }
        if a.is_heap() && b.is_heap()
            && let (HeapObj::Str(x), HeapObj::Str(y)) = (self.heap.get(a), self.heap.get(b)) {
                return Ok(x < y);
        }
        Err(VmErr::TypeMsg(s!(
            "'<' not supported between instances of '",
            str self.type_name(a), "' and '", str self.type_name(b), "'"
        )))
    }

    /* Item presence in list/tuple/dict/set, or substring in string. */
    pub fn contains(&self, container: Val, item: Val) -> bool {
        if !container.is_heap() { return false; }
        match self.heap.get(container) {
            HeapObj::List(v) => v.borrow().iter().any(|x| eq_vals_with_heap(*x, item, &self.heap)),
            HeapObj::Tuple(v) => v.iter().any(|x| eq_vals_with_heap(*x, item, &self.heap)),
            HeapObj::Dict(p) => p.borrow().contains_key(&item),
            HeapObj::Set(s) => s.borrow().iter().any(|x| eq_vals_with_heap(*x, item, &self.heap)),
            HeapObj::FrozenSet(s) => s.iter().any(|x| eq_vals_with_heap(*x, item, &self.heap)),
            HeapObj::Str(s) => {
                if item.is_heap() && let HeapObj::Str(sub) = self.heap.get(item) { return s.contains(sub.as_str()); }
                false
            }
            _ => false
        }
    }
    pub fn add_vals(&mut self, a: Val, b: Val) -> Result<Val, VmErr> {
        // Inline-int fast path; overflow falls through to the i128 slow path.
        if a.is_int() && b.is_int()
            && let Some(r) = a.as_int().checked_add(b.as_int())
            && (Val::INT_MIN..=Val::INT_MAX).contains(&r) {
            return Ok(Val::int(r));
        }
        if let Some((af, bf)) = coerce_floats(a, b) { return Ok(Val::float(af + bf)); }
        // Wide-int slow path; int_to_val picks the narrowest storage class.
        if let (Some(ai), Some(bi)) = (as_i128(a, &self.heap), as_i128(b, &self.heap)) {
            return self.int_to_val(ai.checked_add(bi));
        }
        if a.is_heap() && b.is_heap() {
            match (self.heap.get(a), self.heap.get(b)) {
                (HeapObj::Str(sa), HeapObj::Str(sb)) => {
                    let sa = sa.clone();
                    let sb = sb.clone();
                    let mut r = String::with_capacity(sa.len() + sb.len());
                    r.push_str(&sa); r.push_str(&sb);
                    return self.heap.alloc(HeapObj::Str(r));
                }
                (HeapObj::List(va), HeapObj::List(vb)) => {
                    let mut lst = va.borrow().clone(); lst.extend_from_slice(&vb.borrow());
                    return self.heap.alloc(HeapObj::List(Rc::new(RefCell::new(lst))));
                }
                (HeapObj::Tuple(va), HeapObj::Tuple(vb)) => {
                    let mut tup = va.clone(); tup.extend_from_slice(vb);
                    return self.heap.alloc(HeapObj::Tuple(tup));
                }
                _ => {}
            }
        }
        Err(VmErr::TypeMsg(s!(
            "unsupported operand type(s) for +: '",
            str self.type_name(a), "' and '", str self.type_name(b), "'"
        )))
    }

    pub fn sub_vals(&mut self, a: Val, b: Val) -> Result<Val, VmErr> {
        if a.is_int() && b.is_int()
            && let Some(r) = a.as_int().checked_sub(b.as_int())
            && (Val::INT_MIN..=Val::INT_MAX).contains(&r) {
            return Ok(Val::int(r));
        }
        if let Some((af, bf)) = coerce_floats(a, b) { return Ok(Val::float(af - bf)); }
        if let (Some(ai), Some(bi)) = (as_i128(a, &self.heap), as_i128(b, &self.heap)) {
            return self.int_to_val(ai.checked_sub(bi));
        }
        // Set difference: fresh set of `a` elements not in `b`.
        if a.is_heap() && b.is_heap()
            && let (HeapObj::Set(sa), HeapObj::Set(sb)) = (self.heap.get(a), self.heap.get(b)) {
            let items: Vec<Val> = sa.borrow().difference(&sb.borrow()).copied().collect();
            return self.alloc_set_value(items);
        }
        Err(VmErr::TypeMsg(s!(
            "unsupported operand type(s) for -: '",
            str self.type_name(a), "' and '", str self.type_name(b), "'"
        )))
    }

    /* Set counterpart of `alloc_list` for `sub_vals`'s set-difference path. */
    fn alloc_set_value(&mut self, items: Vec<Val>) -> Result<Val, VmErr> {
        let mut s = crate::util::fx::FxHashSet::default();
        for v in items { s.insert(v); }
        self.heap.alloc(HeapObj::Set(Rc::new(RefCell::new(s))))
    }

    pub fn mul_vals(&mut self, a: Val, b: Val) -> Result<Val, VmErr> {
        if a.is_int() && b.is_int()
            && let Some(r) = a.as_int().checked_mul(b.as_int())
            && (Val::INT_MIN..=Val::INT_MAX).contains(&r) {
            return Ok(Val::int(r));
        }
        if let Some((af, bf)) = coerce_floats(a, b) { return Ok(Val::float(af * bf)); }
        // Numeric multiply wins over sequence repetition when both sides are int-like.
        if let (Some(ai), Some(bi)) = (as_i128(a, &self.heap), as_i128(b, &self.heap)) {
            return self.int_to_val(ai.checked_mul(bi));
        }
        // Sequence repetition: str/list/tuple * int (count clamped to i64).
        let (seq_val, count) = if a.is_heap() && b.is_int() && !matches!(self.heap.get(a), HeapObj::LongInt(_)) {
            (a, b.as_int())
        } else if a.is_int() && b.is_heap() && !matches!(self.heap.get(b), HeapObj::LongInt(_)) {
            (b, a.as_int())
        } else {
            return Err(VmErr::TypeMsg(s!(
                "unsupported operand type(s) for *: '",
                str self.type_name(a), "' and '", str self.type_name(b), "'"
            )));
        };
        let n = count.max(0) as usize;
        match self.heap.get(seq_val) {
            HeapObj::Str(s) => {
                let r = s.repeat(n);
                return self.heap.alloc(HeapObj::Str(r));
            }
            HeapObj::List(rc) => {
                let src = rc.borrow().clone();
                let mut out = Vec::with_capacity(src.len() * n);
                for _ in 0..n { out.extend_from_slice(&src); }
                return self.heap.alloc(HeapObj::List(Rc::new(RefCell::new(out))));
            }
            HeapObj::Tuple(v) => {
                let src = v.clone();
                let mut out = Vec::with_capacity(src.len() * n);
                for _ in 0..n { out.extend_from_slice(&src); }
                return self.heap.alloc(HeapObj::Tuple(out));
            }
            _ => {}
        }
        Err(VmErr::TypeMsg(s!(
            "unsupported operand type(s) for *: '",
            str self.type_name(a), "' and '", str self.type_name(b), "'"
        )))
    }

    pub fn div_vals(&mut self, a: Val, b: Val) -> Result<Val, VmErr> {
        let bv = self.to_f64_coerce(b).map_err(|_| cold_type("'/' requires numeric operands"))?;
        if bv == 0.0 { return Err(VmErr::ZeroDiv); }
        let av = self.to_f64_coerce(a).map_err(|_| cold_type("'/' requires numeric operands"))?;
        Ok(Val::float(av / bv))
    }

    /* Method wrapper around the free `as_i128` for borrow-checker ergonomics. */
    #[inline]
    pub(crate) fn as_i128(&self, v: Val) -> Option<i128> {
        as_i128(v, &self.heap)
    }

    pub(crate) fn to_f64_coerce(&self, v: Val) -> Result<f64, VmErr> {
        if v.is_int() { return Ok(v.as_int() as f64); }
        if v.is_float() { return Ok(v.as_float()); }
        if v.is_bool() { return Ok(v.as_bool() as i64 as f64); }
        if v.is_heap() && let HeapObj::LongInt(i) = self.heap.get(v) { return Ok(*i as f64); }
        Err(cold_type("numeric operand required"))
    }

    /* Wrap an i128 into the narrowest Val: None→Overflow, 47-bit→inline, else LongInt. */
    #[inline]
    pub(crate) fn int_to_val(&mut self, r: Option<i128>) -> Result<Val, VmErr> {
        let i = r.ok_or(cold_overflow())?;
        if (Val::INT_MIN as i128..=Val::INT_MAX as i128).contains(&i) {
            return Ok(Val::int(i as i64));
        }
        self.heap.alloc(HeapObj::LongInt(i))
    }
}

/* i128 decimal render via itoa to avoid the heavier `format!` machinery on the hot path. */
fn i128_to_dec(n: i128) -> String {
    let mut buf = itoa::Buffer::new();
    buf.format(n).to_string()
}
