use alloc::{string::String, vec, vec::Vec};

use super::super::VM;
use super::super::types::*;

fn i128_to_radix(n: i128, radix: u32, prefix: &str) -> String {
    if n == 0 {
        let mut s = String::with_capacity(prefix.len() + 1);
        s.push_str(prefix); s.push('0'); return s;
    }
    let neg = n < 0;
    // unsigned_abs handles i128::MIN safely: returns 2^127 in u128.
    let mut abs = n.unsigned_abs();
    // Max length: i128 in base 2 is 128 digits + sign + prefix. 144 fits all.
    let mut buf = [0u8; 144];
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

impl<'a> VM<'a> {

    pub fn call_abs(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let v = if o.is_float() {
            Val::float(o.as_float().abs())
        } else if let Some(i) = self.as_i128(o) {
            // i128::MIN.checked_abs() is None, trap as OverflowError.
            self.int_to_val(i.checked_abs())?
        } else {
            return Err(cold_type("abs() requires a number"));
        };
        self.push(v); Ok(())
    }

    pub fn call_int(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        // Already an int (inline or LongInt), pass through unchanged.
        if o.is_int() { self.push(o); return Ok(()); }
        if o.is_heap() && matches!(self.heap.get(o), HeapObj::LongInt(_)) {
            self.push(o); return Ok(());
        }
        let i: i128 = if o.is_float() { o.as_float() as i128 }
            else if o.is_bool() { o.as_bool() as i128 }
            else if o.is_heap() && let HeapObj::Str(s) = self.heap.get(o) {
                // Parse directly into i128 so "9999999999999999999" (>i64) works.
                s.trim().parse::<i128>().map_err(|_| cold_value("int(): invalid literal"))?
            }
            else { return Err(cold_type("int() requires a number or string")); };
        let v = self.int_to_val(Some(i))?;
        self.push(v); Ok(())
    }

    /* Converts int or parseable string to floating point. */
    pub fn call_float(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let f = if o.is_float() { o.as_float() }
            else if o.is_bool() { o.as_bool() as i64 as f64 }
            else if o.is_int() { o.as_int() as f64 }
            else if o.is_heap() && let HeapObj::LongInt(i) = self.heap.get(o) { *i as f64 }
            else if o.is_heap() && let HeapObj::Str(s) = self.heap.get(o) {
                s.trim().parse().map_err(|_| cold_value("float(): invalid literal"))?
            }
            else { return Err(cold_type("float() requires a number or string")); };
        self.push(Val::float(f)); Ok(())
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

    pub fn call_round(&mut self, op: u16) -> Result<(), VmErr> {
        let args = self.pop_n(op as usize)?;
        let v = match (args.first(), args.get(1)) {
            (Some(o), Some(n)) if o.is_float() && n.is_int() => {
                let factor = fpowi(10.0, n.as_int() as i32);
                Val::float(fround(o.as_float() * factor) / factor)
            }
            (Some(o), None) if o.is_float() => Val::int(fround(o.as_float()) as i64),
            (Some(o), _) if o.is_int() => *o,
            (Some(o), _) if o.is_heap() && matches!(self.heap.get(*o), HeapObj::LongInt(_)) => *o,
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

    /* If a single arg is a list/tuple/set, return its items; otherwise pass args through unchanged. Used by min/max / etc. for varargs vs iterable. */
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

    pub fn call_sum(&mut self, op: u16) -> Result<(), VmErr> {
        let args = self.pop_n(op as usize)?;
        if args.is_empty() { return Err(cold_type("sum() requires at least 1 argument")); }
        let start = if args.len() > 1 { args[1] } else { Val::int(0) };
        let mut cur = self.iter_cursor(args[0])?;
        let mut acc = start;
        while let Some(item) = cur.next(&mut self.heap)? {
            acc = self.add_vals(acc, item)?;
        }
        self.push(acc); Ok(())
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
        if let Some(i) = self.as_i128(v) {
            return Ok(i128_to_radix(i, radix, prefix));
        }
        Err(cold_type("integer required"))
    }

    pub fn call_divmod(&mut self) -> Result<(), VmErr> {
        let b = self.pop()?;
        let a = self.pop()?;
        let (Some(ai), Some(bi)) = (self.as_i128(a), self.as_i128(b)) else { return Err(cold_type("divmod() requires integer operands")); };
        if bi == 0 { return Err(VmErr::ZeroDiv); }
        // checked_div guards i128::MIN / -1.
        let q = ai.checked_div(bi).ok_or(cold_overflow())?;
        let r = ai - q * bi;
        // Floor-div sign correction so divmod matches `(a // b, a % b)`.
        let (q, r) = if (r != 0) && ((r < 0) != (bi < 0)) { (q - 1, r + bi) } else { (q, r) };
        let qv = self.int_to_val(Some(q))?;
        let rv = self.int_to_val(Some(r))?;
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
                // Modular exponentiation: (a ** b) % c on i128.
                let (Some(base), Some(modulus)) = (self.as_i128(args[0]), self.as_i128(args[2]))
                else { return Err(cold_type("pow() with 3 args requires integers")); };
                let exp = self.as_i128(args[1]).ok_or(cold_type("pow() with 3 args requires integer exponent"))?;
                if exp < 0 { return Err(cold_value("pow() exponent must be non-negative")); }
                if modulus == 0 { return Err(VmErr::ZeroDiv); }

                let m = modulus.unsigned_abs();
                // Cap |m| < 2^63 so m*m fits in i128; larger moduli would overflow silently.
                if m > (1u128 << 63) { return Err(cold_value("pow() modulus too large; must be < 2^63 (no arbitrary precision)")); }
                let m = m as i128;
                let mut result = 1i128;
                let mut b = base.rem_euclid(m);
                let mut e = exp;
                while e > 0 {
                    if e & 1 == 1 {
                        result = (result * b).rem_euclid(m);
                    }
                    e >>= 1;
                    if e > 0 {
                        b = (b * b).rem_euclid(m);
                    }
                }
                let r = self.int_to_val(Some(result))?;
                self.push(r);
                Ok(())
            }
            _ => Err(cold_type("pow() takes 2 or 3 arguments")),
        }
    }

    fn pow_2arg(&mut self, a: Val, b: Val) -> Result<Val, VmErr> {
        self.pow_vals(a, b, "pow() requires numeric operands")
    }

    /* Two-arg power for `pow()` and `**`. int**non-neg int stays i128 (overflow trap); floats or negative exponents promote to f64. */
    pub(crate) fn pow_vals(&mut self, a: Val, b: Val, err_msg: &'static str) -> Result<Val, VmErr> {
        if let (Some(ai), true) = (self.as_i128(a), b.is_int()) {
            let exp = b.as_int();
            if exp >= 0 {
                // i128 exp-by-squaring; overflow at any step -> OverflowError. Bases +/- 1/0 never overflow regardless of exp size.
                let mut result: i128 = 1;
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
                return self.int_to_val(Some(result));
            }
            return Ok(Val::float(fpowi(ai as f64, exp as i32)));
        }
        let to_f = |v: Val| -> Result<f64, VmErr> {
            if v.is_int() { Ok(v.as_int() as f64) }
            else if v.is_float() { Ok(v.as_float()) }
            else if v.is_heap() {
                if let HeapObj::LongInt(i) = self.heap.get(v) { Ok(*i as f64) }
                else { Err(cold_type(err_msg)) }
            }
            else { Err(cold_type(err_msg)) }
        };
        Ok(Val::float(fpowf(to_f(a)?, to_f(b)?)))
    }
}
