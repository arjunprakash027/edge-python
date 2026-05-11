use super::*;

use cache::OpcodeCache;
use ops::cached_binop;

impl<'a> VM<'a> {

    /* Add/Sub/Mul/Div with IC; Mod/Pow/FloorDiv on i64 with overflow trap; Minus is unary. */
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
        let result = if v.is_float() {
            Val::float(-v.as_float())
        } else if let Some(i) = self.as_i128(v) {
            // -i128::MIN overflows; everything else fits.
            self.int_to_val(i.checked_neg())?
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
        let (Some(ai), Some(bi)) = (self.as_i128(a), self.as_i128(b)) else { return Err(cold_type("% requires numeric operands")); };
        if bi == 0 { return Err(VmErr::ZeroDiv); }
        // Floor-mod on i128: result takes the divisor's sign. `checked_rem` guards against i128::MIN % -1 (which would overflow).
        let r = ai.checked_rem(bi).ok_or(cold_overflow())?;
        let r = if (r != 0) && ((r < 0) != (bi < 0)) { r + bi } else { r };
        self.int_to_val(Some(r))
    }

    fn exec_floordiv(&mut self, a: Val, b: Val) -> Result<Val, VmErr> {
        if a.is_float() || b.is_float() {
            let af = self.to_f64_coerce(a).map_err(|_| cold_type("// requires numeric operands"))?;
            let bf = self.to_f64_coerce(b).map_err(|_| cold_type("// requires numeric operands"))?;
            if bf == 0.0 { return Err(VmErr::ZeroDiv); }
            // ffloor() handles all magnitudes; `as i64` would overflow for large floats.
            return Ok(Val::float(ffloor(af / bf)));
        }
        let (Some(ai), Some(bi)) = (self.as_i128(a), self.as_i128(b)) else { return Err(cold_type("// requires numeric operands")); };
        if bi == 0 { return Err(VmErr::ZeroDiv); }
        // Floor-div on i128: round toward negative infinity. checked_div guards i128::MIN / -1 overflow.
        let q = ai.checked_div(bi).ok_or(cold_overflow())?;
        let r = ai - q * bi;
        let q = if (r != 0) && ((r < 0) != (bi < 0)) { q - 1 } else { q };
        self.int_to_val(Some(q))
    }

    fn exec_pow(&mut self, a: Val, b: Val) -> Result<Val, VmErr> {
        self.pow_vals(a, b, "** requires numeric operands")
    }

    /* i128 bitwise + Shl/Shr (overflow trap); BitNot unary. Set/Set on |/&/^ means union/intersection/symmetric-diff; other types use the bitwise path. */
    pub(crate) fn handle_bitwise(&mut self, op: OpCode) -> Result<(), VmErr> {
        if op == OpCode::BitNot {
            let v = self.pop()?;
            let i = self.as_i128(v).ok_or(cold_type("~ requires an integer"))?;
            let out = self.int_to_val(Some(!i))?;
            self.push(out);
            return Ok(());
        }

        let (a, b) = self.pop2()?;
        if a.is_heap() && b.is_heap()
            && matches!(self.heap.get(a), HeapObj::Set(_))
            && matches!(self.heap.get(b), HeapObj::Set(_))
            && matches!(op, OpCode::BitAnd | OpCode::BitOr | OpCode::BitXor) {
            return self.set_binop_and_push(a, b, op);
        }
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
        if shift >= 128 { return Err(cold_overflow()); }
        let ai = self.as_i128(a).ok_or(cold_type("<< requires an integer"))?;
        self.int_to_val(ai.checked_shl(shift as u32))
    }

    fn exec_shr(&mut self, a: Val, b: Val) -> Result<Val, VmErr> {
        if !b.is_int() { return Err(cold_type("shift count must be an integer")); }
        let shift = b.as_int();
        if shift < 0 { return Err(cold_value("negative shift count")); }
        let ai = self.as_i128(a).ok_or(cold_type(">> requires an integer"))?;
        // i128 >> is arithmetic (floor on negatives); `.min(127)` dodges shift-count UB.
        self.int_to_val(Some(ai >> shift.min(127)))
    }

    pub(crate) fn handle_compare(&mut self, op: OpCode, rip: usize, cache: &mut OpcodeCache) -> Result<(), VmErr> {
        let (a, b) = self.pop2()?;
        // Record type-key for every compare op; `cache::specialize` picks the FastOp variant.
        cached_binop!(self.heap, rip, &op, a, b, cache);

        // Set/Set uses subset/superset, NOT total order — the numeric `LtEq = !lt_vals(b, a)` identity is wrong here ({1,2} <= {2,3} would come back True), so we bypass `lt_vals`.
        if a.is_heap() && b.is_heap()
            && matches!(self.heap.get(a), HeapObj::Set(_))
            && matches!(self.heap.get(b), HeapObj::Set(_)) {
            return self.set_compare_and_push(a, b, op);
        }

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

    // Only plain `not`; And/Or are short-circuited by the parser via Jump-If-Or-Pop.
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
