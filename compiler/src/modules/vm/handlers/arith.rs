use super::*;

use cache::OpcodeCache;
use ops::cached_binop;

impl<'a> VM<'a> {

    /* Add/Sub/Mul/Div with IC; Mod/Pow/FloorDiv via BigInt; Minus is unary. */
    pub(crate) fn handle_arith(&mut self, op: OpCode, rip: usize, cache: &mut OpcodeCache) -> Result<(), VmErr> {
        if op == OpCode::Minus {
            return self.exec_neg();
        }

        let (a, b) = self.pop2()?;
        // Register-based FastOps (Add/Sub/Mul/Mod/FloorDiv) are cached; Div/Pow are not.
        if matches!(op, OpCode::Add | OpCode::Sub | OpCode::Mul | OpCode::Mod | OpCode::FloorDiv) {
            cached_binop!(self.heap, rip, &op, a, b, cache);
        }

        let result = match op {
            OpCode::Add => self.add_vals(a, b)?,
            OpCode::Sub => self.sub_vals(a, b)?,
            OpCode::Mul => self.mul_vals(a, b)?,
            OpCode::Div => self.div_vals(a, b)?,
            OpCode::Mod => self.exec_mod(a, b)?,
            OpCode::Pow => self.exec_pow(a, b)?,
            OpCode::FloorDiv => self.exec_floordiv(a, b)?,
            _ => return Err(cold_runtime("non-arith opcode in handle_arith")),
        };
        self.push(result);
        Ok(())
    }

    fn exec_neg(&mut self) -> Result<(), VmErr> {
        let v = self.pop()?;
        let result = if v.is_int() {
            self.i128_to_val(-(v.as_int() as i128))?
        } else if v.is_float() {
            Val::float(-v.as_float())
        } else if v.is_heap() {
            match self.heap.get(v) {
                HeapObj::BigInt(b) => { let n = b.neg(); self.bigint_to_val(n)? }
                _ => return Err(cold_type("unary - requires a number")),
            }
        } else {
            return Err(cold_type("unary - requires a number"));
        };
        self.push(result);
        Ok(())
    }

    fn exec_mod(&mut self, a: Val, b: Val) -> Result<Val, VmErr> {
        if a.is_float() || b.is_float() {
            let af = self.to_f64_coerce(a).map_err(|_| cold_type("% requires numeric operands"))?;
            let bf = self.to_f64_coerce(b).map_err(|_| cold_type("% requires numeric operands"))?;
            if bf == 0.0 { return Err(VmErr::ZeroDiv); }
            // Floor-division semantics: result takes the divisor's sign.
            let r = af - ffloor(af / bf) * bf;
            return Ok(Val::float(r));
        }
        let (Some(ba), Some(bb)) = (self.to_bigint(a), self.to_bigint(b))
            else { return Err(cold_type("% requires numeric operands")); };
        let (_, r) = ba.divmod(&bb).ok_or(VmErr::ZeroDiv)?;
        self.bigint_to_val(r)
    }

    fn exec_floordiv(&mut self, a: Val, b: Val) -> Result<Val, VmErr> {
        if a.is_float() || b.is_float() {
            let af = self.to_f64_coerce(a).map_err(|_| cold_type("// requires numeric operands"))?;
            let bf = self.to_f64_coerce(b).map_err(|_| cold_type("// requires numeric operands"))?;
            if bf == 0.0 { return Err(VmErr::ZeroDiv); }
            // ffloor() handles all magnitudes; `as i64` would overflow for large floats.
            return Ok(Val::float(ffloor(af / bf)));
        }
        let (Some(ba), Some(bb)) = (self.to_bigint(a), self.to_bigint(b))
            else { return Err(cold_type("// requires numeric operands")); };
        let (q, _) = ba.divmod(&bb).ok_or(VmErr::ZeroDiv)?;
        self.bigint_to_val(q)
    }

    fn exec_pow(&mut self, a: Val, b: Val) -> Result<Val, VmErr> {
        self.pow_vals(a, b, "** requires numeric operands")
    }

    /* BitAnd/Or/Xor via closure; BitNot is unary; Shl/Shr through BigInt. */
    pub(crate) fn handle_bitwise(&mut self, op: OpCode) -> Result<(), VmErr> {
        if op == OpCode::BitNot {
            let v = self.pop()?;
            let b = self.to_bigint(v).ok_or(cold_type("~ requires an integer"))?;
            let one = BigInt::from_i64(1);
            let result = b.add(&one).neg();
            let out = self.bigint_to_val(result)?;
            self.push(out);
            return Ok(());
        }

        let (a, b) = self.pop2()?;
        let result = match op {
            OpCode::BitAnd => self.bitwise_op(a, b, |x, y| x & y)?,
            OpCode::BitOr => self.bitwise_op(a, b, |x, y| x | y)?,
            OpCode::BitXor => self.bitwise_op(a, b, |x, y| x ^ y)?,
            OpCode::Shl => self.exec_shl(a, b)?,
            OpCode::Shr => self.exec_shr(a, b)?,
            _ => return Err(cold_runtime("non-bitwise opcode in handle_bitwise")),
        };
        self.push(result);
        Ok(())
    }

    fn exec_shl(&mut self, a: Val, b: Val) -> Result<Val, VmErr> {
        if !b.is_int() { return Err(cold_type("shift count must be an integer")); }
        let shift = b.as_int();
        if shift < 0 { return Err(cold_value("negative shift count")); }
        let ba = self.to_bigint(a).ok_or(cold_type("<< requires an integer"))?;
        if shift >= 512 { return Err(cold_value("shift too large")); }
        let factor = BigInt::from_i64(1).shl_u32(shift as u32);
        self.bigint_to_val(ba.mul(&factor))
    }

    fn exec_shr(&mut self, a: Val, b: Val) -> Result<Val, VmErr> {
        if !b.is_int() { return Err(cold_type("shift count must be an integer")); }
        let shift = b.as_int();
        if shift < 0 { return Err(cold_value("negative shift count")); }
        if a.is_int() {
            return Ok(Val::int(a.as_int() >> shift.min(63)));
        }
        let ba = self.to_bigint(a).ok_or(cold_type(">> requires an integer"))?;
        self.bigint_to_val(ba.shr_u32(shift.min(1024) as u32))
    }

    pub(crate) fn handle_compare(&mut self, op: OpCode, rip: usize, cache: &mut OpcodeCache) -> Result<(), VmErr> {
        let (a, b) = self.pop2()?;
        // Record type-key for every compare op; cache::specialize() decides
        // which ones have a FastOp variant (all of them currently).
        cached_binop!(self.heap, rip, &op, a, b, cache);
        let result = match op {
            OpCode::Eq => eq_vals_with_heap(a, b, &self.heap),
            OpCode::NotEq => !eq_vals_with_heap(a, b, &self.heap),
            OpCode::Lt => self.lt_vals(a, b)?,
            OpCode::Gt => self.lt_vals(b, a)?,
            OpCode::LtEq => !self.lt_vals(b, a)?,
            OpCode::GtEq => !self.lt_vals(a, b)?,
            _ => return Err(cold_runtime("non-compare opcode in handle_compare")),
        };
        self.push(Val::bool(result));
        Ok(())
    }

    /* Short-circuiting is done by the parser via Jump-If-Or-Pop opcodes; this
       only handles plain `not`. And/Or never reach here. */
    pub(crate) fn handle_logic(&mut self, op: OpCode) -> Result<(), VmErr> {
        match op {
            OpCode::Not => {
                let v = self.pop()?;
                self.push(Val::bool(!self.truthy(v)));
            }
            _ => return Err(cold_runtime("non-logic opcode in handle_logic")),
        }
        Ok(())
    }

    /* `is` / `is not` compare tag bits inline; `in` / `not in` delegate to contains(). */
    pub(crate) fn handle_identity(&mut self, op: OpCode) -> Result<(), VmErr> {
        let (a, b) = self.pop2()?;
        let result = match op {
            OpCode::In => self.contains(b, a),
            OpCode::NotIn => !self.contains(b, a),
            OpCode::Is => a.0 == b.0,
            OpCode::IsNot => a.0 != b.0,
            _ => return Err(cold_runtime("non-identity opcode in handle_identity")),
        };
        self.push(Val::bool(result));
        Ok(())
    }
}