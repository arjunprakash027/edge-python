use crate::s;
use super::*;

impl<'a> VM<'a> {
    /* Dispatch every function-shaped opcode (Call, MakeFunction, builtins). */
    pub(crate) fn handle_function(
        &mut self, op: OpCode, operand: u16,
        chunk: &SSAChunk, slots: &mut [Val]
    ) -> Result<(), VmErr> {
        match op {
            OpCode::Call => self.exec_call(operand, chunk, slots),
            OpCode::MakeFunction | OpCode::MakeCoroutine => self.exec_make_function(op, operand, chunk, slots),
            OpCode::CallLen => self.call_len(),
            OpCode::CallAbs => self.call_abs(),
            OpCode::CallStr => self.call_str(),
            OpCode::CallInt => self.call_int(),
            OpCode::CallFloat => self.call_float(),
            OpCode::CallBool => self.call_bool(),
            OpCode::CallType => self.call_type(),
            OpCode::CallChr => self.call_chr(),
            OpCode::CallOrd => self.call_ord(),
            OpCode::CallSorted => self.call_sorted(),
            OpCode::CallList => self.call_list(),
            OpCode::CallTuple => self.call_tuple(),
            OpCode::CallEnumerate => self.call_enumerate(),
            OpCode::CallIsInstance => self.call_isinstance(),
            OpCode::CallRange => self.call_range(operand),
            OpCode::CallRound => self.call_round(operand),
            OpCode::CallMin => self.call_min(operand),
            OpCode::CallMax => self.call_max(operand),
            OpCode::CallSum => self.call_sum(operand),
            OpCode::CallZip => self.call_zip(operand),
            OpCode::CallDict => self.call_dict(operand),
            OpCode::CallSet => self.call_set(operand),
            OpCode::CallPrint => { self.mark_impure(); self.call_print(operand) }
            OpCode::CallInput => { self.mark_impure(); self.call_input() }
            OpCode::CallAll      => self.call_all(operand),
            OpCode::CallAny      => self.call_any(operand),
            OpCode::CallBin      => self.call_bin(),
            OpCode::CallOct      => self.call_oct(),
            OpCode::CallHex      => self.call_hex(),
            OpCode::CallDivmod   => self.call_divmod(),
            OpCode::CallPow      => self.call_pow(operand),
            OpCode::CallRepr     => self.call_repr(),
            OpCode::CallReversed => self.call_reversed(),
            OpCode::CallCallable => self.call_callable(),
            OpCode::CallId       => self.call_id(),
            OpCode::CallHash     => self.call_hash(),
            _ => Err(cold_runtime("non-function opcode in handle_function")),
        }
    }

    pub(crate) fn exec_bound_method(
        &mut self, recv: Val,
        id: super::methods::BuiltinMethodId,
        pos: Vec<Val>, kw: Vec<Val>,
    ) -> Result<(), VmErr> {
        super::methods::dispatch_method(self, id, recv, pos, kw)
    }

    fn exec_make_function(&mut self, opcode: OpCode, operand: u16, chunk: &SSAChunk, slots: &[Val]) -> Result<(), VmErr> {
        let chunk_ptr = chunk as *const _;
        let global = self.fn_index.iter()
            .find(|(p, _)| *p == chunk_ptr)
            .and_then(|(_, v)| v.get(operand as usize).copied())
            .ok_or(cold_runtime("MakeFunction: unknown function index"))? as usize;

        if opcode == OpCode::MakeCoroutine {
            if self.is_async.len() <= global { self.is_async.resize(global + 1, false); }
            self.is_async[global] = true;
        }
        let n_defaults = self.functions[global].2 as usize;
        let defaults = if n_defaults > 0 { self.pop_n(n_defaults)? } else { vec![] };

        let (params, body, _, _) = self.functions[global];
        let param_names: crate::modules::fx::FxHashSet<String> = params.iter().map(|p| s!(str p.trim_start_matches('*'), "_0")).collect();
        let mut captures: Vec<(usize, Val)> = Vec::new();
        // Capture closure values once per canonical (coalesced) slot, skipping
        // names already bound as formal parameters. The body.names list is
        // typically <30, so a linear scan over chunk.names is competitive
        // with a HashMap and avoids a per-call monomorphization.
        let mut seen_canonical: crate::modules::fx::FxHashSet<usize> = crate::modules::fx::FxHashSet::default();
        for (bi, bname) in body.names.iter().enumerate() {
            if param_names.contains(bname.as_str()) { continue; }
            let canon = body.alias_groups.get(bi)
                .and_then(|g| g.first().copied())
                .unwrap_or(bi as u16) as usize;
            if !seen_canonical.insert(canon) { continue; }
            if let Some((si, _)) = chunk.names.iter().enumerate().find(|(_, n)| n.as_str() == bname.as_str())
                && let Some(&v) = slots.get(si)
                && !v.is_undef() {
                    captures.push((canon, v));
                }
        }

        let val = self.heap.alloc(HeapObj::Func(global, defaults, captures))?;
        self.push(val);
        Ok(())
    }

    pub(crate) fn exec_call(&mut self, operand: u16, chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        let raw = operand as usize;

        let base_pos = (raw & 0xFF)        as i32;
        let base_kw  = ((raw >> 8) & 0xFF) as i32;
        let num_pos = (base_pos + self.pending_pos_delta).max(0) as usize;
        let num_kw  = (base_kw  + self.pending_kw_delta ).max(0) as usize;
        self.pending_pos_delta = 0;
        self.pending_kw_delta  = 0;

        let total_items = num_pos + 2 * num_kw;

        if self.depth >= self.max_calls { return Err(cold_depth()); }

        let mut stack_items: Vec<Val> = (0..total_items)
            .map(|_| self.pop())
            .collect::<Result<_, _>>()?;
        stack_items.reverse();

        let kw_flat: Vec<Val> = stack_items.split_off(num_pos);
        let positional = stack_items;

        let callee = self.pop()?;
        if !callee.is_heap() { return Err(cold_type("object is not callable")); }

        if let HeapObj::BoundMethod(recv, id) = self.heap.get(callee) {
            let recv = *recv;
            let id = *id;
            return self.exec_bound_method(recv, id, positional, kw_flat);
        }

        if let HeapObj::NativeFn(id) = self.heap.get(callee) {
            let id = *id;
            return self.dispatch_native(id, positional, kw_flat);
        }

        // Calling a class: create an instance and run __init__ if defined.
        if let HeapObj::Class(_, methods) = self.heap.get(callee) {
            let methods = methods.clone();
            let instance = self.heap.alloc(HeapObj::Instance(callee, Rc::new(RefCell::new(DictMap::new()))))?;
            if let Some((_, init_fn)) = methods.iter().find(|(n, _)| n == "__init__") {
                let init_fn = *init_fn;
                self.push(init_fn);
                let mut args = vec![instance];
                args.extend_from_slice(&positional);
                for a in &args { self.push(*a); }
                let argc = args.len() as u16;
                self.exec_call(argc, chunk, slots)?;
                // Discard __init__'s return value.
                self.pop()?;
            }
            self.push(instance);
            return Ok(());
        }

        // Bound user method: prepend `self` to the arg list and re-dispatch.
        if let HeapObj::BoundUserMethod(recv, func) = self.heap.get(callee) {
            let (recv, func) = (*recv, *func);
            self.push(func);
            self.push(recv);
            for a in &positional { self.push(*a); }
            let argc = (positional.len() + 1) as u16;
            let encoded = ((num_kw as u16) << 8) | argc;
            return self.exec_call(encoded, chunk, slots);
        }

        // Resume a suspended coroutine; the inner yield must NOT propagate
        // to the surrounding function call.
        if let HeapObj::Coroutine(..) = self.heap.get(callee) {
            let result = self.resume_coroutine(callee)?;
            if self.yielded { self.yielded = false; }
            self.push(result);
            return Ok(());
        }

        let fi = match self.heap.get(callee) {
            HeapObj::Func(i, _, _) => *i,
            _ => return Err(cold_type("object is not callable")),
        };

        let outer_impure = self.observed_impure.last().copied().unwrap_or(false);
        if num_kw == 0 && !outer_impure
            && let Some(cached) = self.templates.lookup(fi, &positional, &self.heap) {
                self.push(cached);
                return Ok(());
        }

        self.depth += 1;
        let (params, body, _defaults, name_idx) = self.functions[fi];
        let name_idx = *name_idx;

        // Pre-built slot template (builtins + undef) instead of `fill_builtins`.
        let mut fn_slots = self.slot_templates[fi].clone();

        // Param binding via pre-computed param_slots.
        let pslots = &self.param_slots[fi];
        let mut pos_idx = 0usize;
        for &(kind, slot) in pslots {
            match kind {
                super::super::ParamKind::DoubleStar => {
                    let dm = DictMap::from_pairs(kw_flat.chunks_exact(2).map(|p| (p[0], p[1])).collect());
                    let dict_val = self.heap.alloc(HeapObj::Dict(Rc::new(RefCell::new(dm))))?;
                    if slot < fn_slots.len() { fn_slots[slot] = dict_val; }
                }
                super::super::ParamKind::Star => {
                    let rest: Vec<Val> = positional[pos_idx..].to_vec();
                    pos_idx = positional.len();
                    let list_val = self.heap.alloc(HeapObj::List(Rc::new(RefCell::new(rest))))?;
                    if slot < fn_slots.len() { fn_slots[slot] = list_val; }
                }
                super::super::ParamKind::Normal => {
                    if pos_idx >= positional.len() { continue; }
                    if slot < fn_slots.len() { fn_slots[slot] = positional[pos_idx]; }
                    pos_idx += 1;
                }
            }
        }

        // Kwargs binding (rare path, not optimised).
        if !kw_flat.is_empty() {
            let body_map = &self.body_maps[fi];
            for pair in kw_flat.chunks_exact(2) {
                let key = match self.heap.get(pair[0]) {
                    HeapObj::Str(s) => s.clone(),
                    _ => return Err(cold_runtime("malformed kwarg on stack")),
                };
                if params.iter().any(|p| p.trim_start_matches('*') == key.as_str()) {
                    let pname = s!(str &key, "_0");
                    if let Some(&s) = body_map.get(pname.as_str()) {
                        fn_slots[s] = pair[1];
                    }
                }
            }
        }

        // Defaults: borrow from heap; only fill slots still undef after binding.
        if let HeapObj::Func(_, defaults, _) = self.heap.get(callee)
            && !defaults.is_empty() {
                let ds = &self.default_slots[fi];
                for (di, &dv) in defaults.iter().enumerate() {
                    if let Some(&(slot, _)) = ds.get(di)
                        && slot < fn_slots.len() && fn_slots[slot].is_undef() {
                            fn_slots[slot] = dv;
                        }
                }
            }

        // Closure captures: same rule as defaults — only fill if undef.
        if let HeapObj::Func(_, _, captures) = self.heap.get(callee) {
            for &(bi, val) in captures {
                if bi < fn_slots.len() && fn_slots[bi].is_undef() {
                    fn_slots[bi] = val;
                }
            }
        }

        // Propagate caller slots into matching body slots (closures over the
        // enclosing scope). is_param_slot is the precomputed bitmap of slots
        // bound to formal parameters, which must NOT be overwritten.
        if self.needs_caller_slots[fi] {
            let body_map = &self.body_maps[fi];
            let param_bm = &self.is_param_slot[fi];
            for (si, &v) in slots.iter().enumerate() {
                if !v.is_undef()
                    && let Some(name) = chunk.names.get(si)
                    && let Some(&bs) = body_map.get(name.as_str())
                    && !param_bm.get(bs).copied().unwrap_or(false)
                {
                    fn_slots[bs] = v;
                }
            }
        }

        // Self-reference: bind the function's own name slot to `callee` so
        // recursive calls resolve without a global lookup.
        if name_idx != u16::MAX
            && let Some(raw_name) = chunk.names.get(name_idx as usize)
        {
            let base = raw_name.rfind('_')
                .filter(|&p| raw_name[p+1..].parse::<u32>().is_ok())
                .map(|p| &raw_name[..p])
                .unwrap_or(raw_name.as_str());
            let versioned = s!(str base, "_0");
            let body_map = &self.body_maps[fi];
            if let Some(&slot) = body_map.get(versioned.as_str())
                && fn_slots[slot].is_undef()
            {
                fn_slots[slot] = callee;
            }
        }

        // Generator/coroutine functions return a suspended Coroutine instead
        // of running. `is_generator` is set at parse time, `is_async` at VM
        // init — both O(1) lookups, no per-call body scan.
        let is_async_fn = self.is_async.get(fi).copied().unwrap_or(false);
        if is_async_fn || body.is_generator {
            let coro = self.heap.alloc(HeapObj::Coroutine(0, fn_slots, Vec::new(), fi, Vec::new()))?;
            self.push(coro);
            self.depth -= 1;
            return Ok(());
        }

        let yields_before = self.yields.len();

        // Push caller slots onto live_slots so GC keeps them reachable.
        // mark() short-circuits on non-heap values, so the whole slice is fine.
        let snap = self.live_slots.len();
        self.live_slots.extend_from_slice(slots);

        self.observed_impure.push(false);
        let exec_result = self.exec(body, &mut fn_slots);
        let callee_impure = self.observed_impure.pop().unwrap_or(true);
        self.live_slots.truncate(snap);
        self.depth -= 1;

        // Back-propagate `nonlocal` writes to the caller's matching slots.
        let nl_table = &self.nonlocal_tables[fi];
        if !nl_table.is_empty() {
            for &(canon_body, _) in nl_table {
                if let Some(&val) = fn_slots.get(canon_body) {
                    if val.is_undef() { continue; }
                    for base in &body.nonlocals {
                        for (si, sname) in chunk.names.iter().enumerate() {
                            if let Some(p) = sname.rfind('_')
                                && &sname[..p] == base.as_str() && si < slots.len() {
                                    slots[si] = val;
                            }
                        }
                        // Sync closure-capture entries with the new value.
                        if let HeapObj::Func(_, _, caps) = self.heap.get_mut(callee) {
                            if let Some(cap) = caps.iter_mut().find(|(ci, _)| *ci == canon_body) {
                                cap.1 = val;
                            } else {
                                caps.push((canon_body, val));
                            }
                        }
                    }
                }
            }
        }

        let result = exec_result?;
        if callee_impure { self.mark_impure(); }

        if self.yields.len() > yields_before {
            let fn_yields = self.yields.split_off(yields_before);
            let val = self.heap.alloc(HeapObj::List(Rc::new(RefCell::new(fn_yields))))?;
            self.push(val);
        } else {
            if num_kw == 0 && body.is_pure && !callee_impure {
                self.templates.record(fi, &positional, result, &self.heap);
            }
            self.push(result);
        }
        Ok(())
    }


    pub(crate) fn dispatch_native(
        &mut self, id: super::super::types::NativeFnId,
        positional: Vec<Val>, kw: Vec<Val>,
    ) -> Result<(), VmErr> {
        if !kw.is_empty() {
            return Err(cold_type("native function takes no keyword arguments"));
        }
        let argc = positional.len() as u16;

        use super::super::types::NativeFnId::*;

        // Pre-validate fixed arity to keep the stack clean on error.
        let expected: Option<u16> = match id {
            Input | Receive => Some(0),
            Len | Abs | Str | Int | Float | Bool | Type | Chr | Ord
            | Sorted | Enumerate | List | Tuple | Bin | Oct | Hex
            | Repr | Reversed | Callable | Id | Hash | Ascii | Next | Sleep => Some(1),
            Divmod | IsInstance | HasAttr => Some(2),
            _ => None,
        };
        if let Some(n) = expected
            && argc != n {
                return Err(cold_type("wrong number of arguments to builtin"));
        }

        for v in positional { self.push(v); }

        match id {
            // Variadic
            Print => {
                // CallPrint is statement-shaped: the dedicated opcode is emitted
                // without a trailing Pop. When `print` is reached via Call (e.g.
                // `p = print; p(42)`), the parser does emit Pop, so we must push
                // an explicit None to keep the stack balanced.
                self.call_print(argc)?;
                self.push(Val::none());
                Ok(())
            }
            Range => self.call_range(argc),
            Round => self.call_round(argc),
            Min => self.call_min(argc),
            Max => self.call_max(argc),
            Sum => self.call_sum(argc),
            Zip => self.call_zip(argc),
            Dict => self.call_dict(argc),
            Set => self.call_set(argc),
            Pow => self.call_pow(argc),
            All => self.call_all(argc),
            Any => self.call_any(argc),
            GetAttr => self.call_getattr(argc),
            Format => self.call_format(argc),
            // 0/1/2-arg
            Input => self.call_input(),
            Len => self.call_len(),
            Abs => self.call_abs(),
            Str => self.call_str(),
            Int => self.call_int(),
            Float => self.call_float(),
            Bool => self.call_bool(),
            Type => self.call_type(),
            Chr => self.call_chr(),
            Ord => self.call_ord(),
            Sorted => self.call_sorted(),
            Enumerate => self.call_enumerate(),
            List => self.call_list(),
            Tuple => self.call_tuple(),
            Bin => self.call_bin(),
            Oct => self.call_oct(),
            Hex => self.call_hex(),
            Repr => self.call_repr(),
            Reversed => self.call_reversed(),
            Callable => self.call_callable(),
            Id => self.call_id(),
            Hash => self.call_hash(),
            Ascii => self.call_ascii(),
            Divmod => self.call_divmod(),
            IsInstance => self.call_isinstance(),
            HasAttr => self.call_hasattr(),
            Next => self.call_next(),
            Run => self.call_run(argc),
            Sleep => self.call_sleep(),
            Receive => self.call_receive(),
        }
    }
}