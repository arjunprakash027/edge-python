use crate::s;
use super::*;
use super::super::ParamKind;

use crate::alloc::string::ToString;

// Builtin conversion-type name -> its constructor; None for exception/other types.
fn constructor_native(name: &str) -> Option<super::super::types::NativeFnId> {
    use super::super::types::NativeFnId::*;
    Some(match name {
        "int" => Int, "float" => Float, "str" => Str, "bytes" => Bytes,
        "bool" => Bool, "list" => List, "tuple" => Tuple, "dict" => Dict,
        "set" => Set, "frozenset" => FrozenSet, "range" => Range, "type" => Type,
        _ => return None,
    })
}

impl<'a> VM<'a> {
    /* Dispatch every function-shaped opcode (Call, MakeFunction, builtins). */
    pub(crate) fn handle_function(&mut self, op: OpCode, operand: u16, chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        match op {
            OpCode::Call => self.exec_call(operand, chunk, slots),
            OpCode::MakeFunction | OpCode::MakeCoroutine => self.exec_make_function(op, operand, chunk, slots),
            OpCode::CallLen => self.call_len(chunk, slots),
            OpCode::CallAbs => self.call_abs(),
            OpCode::CallStr => self.call_str(chunk, slots),
            OpCode::CallInt => self.call_int(operand),
            OpCode::CallFloat => self.call_float(),
            OpCode::CallBool => self.call_bool(chunk, slots),
            OpCode::CallType => self.call_type(),
            OpCode::CallChr => self.call_chr(),
            OpCode::CallOrd => self.call_ord(),
            OpCode::CallSorted => self.call_sorted(false),
            OpCode::CallList => self.call_list(chunk, slots),
            OpCode::CallTuple => self.call_tuple(chunk, slots),
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
            OpCode::CallPrint => { self.mark_impure(); self.call_print(operand, chunk, slots) }
            OpCode::CallInput => { self.mark_impure(); self.call_input() }
            OpCode::CallAll => self.call_all(operand),
            OpCode::CallAny => self.call_any(operand),
            OpCode::CallBin => self.call_bin(),
            OpCode::CallOct => self.call_oct(),
            OpCode::CallHex => self.call_hex(),
            OpCode::CallDivmod => self.call_divmod(),
            OpCode::CallPow => self.call_pow(operand),
            OpCode::CallRepr => self.call_repr(chunk, slots),
            OpCode::CallReversed => self.call_reversed(),
            OpCode::CallCallable => self.call_callable(),
            OpCode::CallId => self.call_id(),
            OpCode::CallHash => self.call_hash(chunk, slots),
            OpCode::CallExtern => self.call_extern(operand, chunk),
            _ => Err(cold_runtime("non-function opcode in handle_function")),
        }
    }

    pub(crate) fn exec_bound_method(&mut self, recv: Val, id: super::methods::BuiltinMethodId, pos: &[Val], kw: &[Val]) -> Result<(), VmErr> {
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
        let param_names: crate::util::fx::FxHashSet<String> = params.iter().map(|p| s!(str crate::modules::parser::types::param_base_name(p), "_0")).collect();
        let mut captures: Vec<(usize, Val)> = Vec::new();
        // Capture once per canonical slot, skipping formal params. Linear scan over `chunk.names` beats a HashMap at typical body sizes (<30) and avoids a per-call monomorphisation.
        let mut seen_canonical: crate::util::fx::FxHashSet<usize> = crate::util::fx::FxHashSet::default();
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

        // Entry-chunk top-level defs go into `globals` so forward refs resolve at call time. Module-level defs stay in the module's bindings (via `fn_module[fi]`) to keep cross-module helpers with the same name isolated.
        if core::ptr::eq(chunk, self.chunk) {
            let name_idx = self.functions[global].3 as usize;
            if name_idx < chunk.names.len() {
                let bare = ssa_strip(&chunk.names[name_idx]).to_string();
                self.globals.insert(bare, val);
            }
        }

        self.push(val);
        Ok(())
    }

    /* `Call` orchestrator. Only user `Func` callees build a fresh `fn_slots` and run the body inline; every other callee kind short-circuits in `try_dispatch_non_func_callable`. */
    pub(crate) fn exec_call(&mut self, operand: u16, chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        let (positional, kw_flat, _num_pos, num_kw) = self.parse_call_args(operand)?;

        if self.depth >= self.max_calls { return Err(cold_depth()); }
        // Charge each call so wide recursion is op-budget bounded.
        self.charge_step()?;

        let callee = self.pop()?;
        if !callee.is_heap() { return Err(cold_type("object is not callable")); }

        if self.try_dispatch_non_func_callable(callee, &positional, &kw_flat, num_kw, chunk, slots)? {
            return Ok(());
        }

        // Snapshot defaults/captures once, both are tiny (<10), and cloning beats the 3+ heap re-reads later phases would do. Back-prop still uses `get_mut` since it writes.
        let (fi, defaults, captures) = match self.heap.get(callee) {
            HeapObj::Func(i, d, c) => (*i, d.clone(), c.clone()),
            _ => return Err(cold_type("object is not callable")),
        };

        // Pure-call memoisation. Disabled under impure outer frames (stale-view risk) or kwargs (cache key only spans positionals).
        let outer_impure = self.observed_impure.last().copied().unwrap_or(false);
        if num_kw == 0 && !outer_impure
            && let Some(cached) = self.templates.lookup(fi, &positional, &self.heap) {
                self.push(cached);
                return Ok(());
        }

        self.depth += 1;
        let (_params, body, _, _) = self.functions[fi];
        let mut fn_slots = self.slot_templates[fi].clone();

        self.bind_function_args(fi, &defaults, &captures, &positional, &kw_flat, &mut fn_slots)?;

        if self.needs_caller_slots[fi] {
            self.apply_caller_slot_propagation(fi, &captures, chunk, slots, &mut fn_slots);
        }

        self.bind_self_reference(fi, callee, &mut fn_slots);

        // Generator/coroutine: return a suspended Coroutine instead of running. Both flags are O(1).
        let is_async_fn = self.is_async.get(fi).copied().unwrap_or(false);
        if is_async_fn || body.is_generator {
            let coro = self.heap.alloc(HeapObj::Coroutine(0, fn_slots, Vec::new(), BodyRef::Fn(fi), Vec::new(), Vec::new(), Vec::new()))?;
            self.push(coro);
            self.depth -= 1;
            return Ok(());
        }

        // Snapshot caller-visible depths so we can split the helper's stack/iter/exception contributions out if it suspends mid-body via a yielding builtin.
        let stack_base = self.stack.len();
        let iter_base = self.iter_stack.len();
        let exc_base = self.exception_stack.len();
        let yields_before = self.yields.len();
        let (callee_impure, exec_result) = self.run_body_with_frame(fi, body, chunk, &mut fn_slots, slots);
        self.depth -= 1;

        self.back_propagate_nonlocals(fi, body, callee, chunk, slots, &fn_slots);

        let result = exec_result?;
        if callee_impure { self.mark_impure(); }

        if self.yielded {
            // Sync helper suspended mid-execution (e.g. `sleep(0)` from inside a sync fn called by an async coro). Stage its frame on the VM-level buffer; `resume_coroutine` drains it onto the enclosing coro so the helper is re-entered from the right ip. Without this, the outer's resume_ip would skip past the unfinished helper and the next StoreName would underflow. A nested sync call inside this helper would already have pushed its own frame first, so the buffer ends up innermost-last.
            let helper_resume_ip = self.resume_ip;
            self.resume_ip = 0;
            let helper_stack_delta = if self.stack.len() > stack_base { self.stack.split_off(stack_base) } else { Vec::new() };
            let helper_iter_delta: Vec<IterFrame> = if self.iter_stack.len() > iter_base { self.iter_stack.drain(iter_base..).collect() } else { Vec::new() };
            let helper_exc_delta: Vec<ExceptionFrame> = if self.exception_stack.len() > exc_base {
                self.exception_stack.drain(exc_base..)
                    .map(|mut f| {
                        f.stack_depth = f.stack_depth.saturating_sub(stack_base);
                        f.iter_depth = f.iter_depth.saturating_sub(iter_base);
                        f
                    })
                    .collect()
            } else { Vec::new() };
            self.pending_sync_frames.push(SyncFrame {
                ip: helper_resume_ip, fi, slots: fn_slots,
                stack_delta: helper_stack_delta, iter_delta: helper_iter_delta, exception_delta: helper_exc_delta,
            });
            return Ok(());
        }

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

    /* Decode `operand` (lo=positional, hi=kw pairs) + star-spread deltas, then pop into positional/kw buffers. kw is flat alternating name/value as the parser emits. */
    pub(crate) fn parse_call_args(&mut self, operand: u16) -> Result<(Vec<Val>, Vec<Val>, usize, usize), VmErr> {
        let raw = operand as usize;

        let base_pos = (raw & 0xFF) as i32;
        let base_kw = ((raw >> 8) & 0xFF) as i32;
        let num_pos = (base_pos + self.pending.pos_delta).max(0) as usize;
        let num_kw = (base_kw + self.pending.kw_delta ).max(0) as usize;
        self.pending.pos_delta = 0;
        self.pending.kw_delta = 0;

        let total_items = num_pos + 2 * num_kw;
        let mut stack_items: Vec<Val> = (0..total_items).map(|_| self.pop()).collect::<Result<_, _>>()?;
        stack_items.reverse();

        let kw_flat: Vec<Val> = stack_items.split_off(num_pos);
        Ok((stack_items, kw_flat, num_pos, num_kw))
    }

    /* Pack a flat `[name, val, name, val, ...]` slice into a heap dict for the trailing kwargs slot. `None` when there are no kwargs so the FFI layer can serialize handle 0 on the wire. */
    pub(crate) fn pack_kw_dict(heap: &mut super::super::types::HeapPool, kw_flat: &[Val]) -> Result<Option<Val>, VmErr> {
        if kw_flat.is_empty() { return Ok(None); }
        let dm = super::super::types::DictMap::from_pairs(kw_flat.chunks_exact(2).map(|p| (p[0], p[1])).collect());
        Ok(Some(heap.alloc(super::super::types::HeapObj::Dict(Rc::new(RefCell::new(dm))))?))
    }

    /* Dispatch non-Func callees. Returns Ok(true) when handled here; Ok(false) means the caller falls through to the Func path. */
    fn try_dispatch_non_func_callable(&mut self, callee: Val, positional: &[Val], kw_flat: &[Val], num_kw: usize, chunk: &SSAChunk, slots: &mut [Val]) -> Result<bool, VmErr> {
        if let HeapObj::BoundMethod(recv, id) = self.heap.get(callee) {
            let recv = *recv;
            let id = *id;
            if id.name() == "sort" && !kw_flat.is_empty() {
                if !positional.is_empty() {
                    return Err(cold_type("list.sort() takes no positional arguments"));
                }
                let mut sort_key: Option<Val> = None;
                let mut sort_reverse = false;
                for pair in kw_flat.chunks(2) {
                    let (name_v, val_v) = (pair[0], pair[1]);
                    let is_key = name_v.is_heap() && matches!(self.heap.get(name_v), HeapObj::Str(s) if s == "key");
                    let is_reverse = name_v.is_heap() && matches!(self.heap.get(name_v), HeapObj::Str(s) if s == "reverse");
                    if is_key { sort_key = Some(val_v); }
                    else if is_reverse { sort_reverse = self.truthy(val_v); }
                    else { return Err(cold_type("list.sort() got unexpected keyword argument")); }
                }
                self.call_list_sort_keyed(recv, sort_key, sort_reverse, chunk, slots)?;
                return Ok(true);
            }
            self.exec_bound_method(recv, id, positional, kw_flat)?;
            return Ok(true);
        }

        if let HeapObj::NativeFn(id) = self.heap.get(callee) {
            let id = *id;
            self.dispatch_native(id, positional, kw_flat, chunk, slots)?;
            return Ok(true);
        }

        if let HeapObj::Extern(extern_fn) = self.heap.get(callee) {
            let func = extern_fn.func.clone();
            let pure = extern_fn.pure;
            if !pure { self.mark_impure(); }
            let kwargs = Self::pack_kw_dict(&mut self.heap, kw_flat)?;
            let result = func(&mut self.heap, positional, kwargs)?;
            self.push(result);
            return Ok(true);
        }

        if let HeapObj::Type(name) = self.heap.get(callee) {
            let name = name.clone();
            if let Some(id) = constructor_native(&name) {
                self.dispatch_native(id, positional, kw_flat, chunk, slots)?; // int/set/list/... construct
                return Ok(true);
            }
            // Other Type objects are exception classes: build an ExcInstance for `raise X("msg")`.
            if !kw_flat.is_empty() {
                return Err(cold_type("exception class takes no keyword arguments"));
            }
            let exc = self.heap.alloc(HeapObj::ExcInstance(name, positional.to_vec()))?;
            self.push(exc);
            return Ok(true);
        }

        // Calling a class: create an instance and run `__init__` if defined (walks bases).
        if let HeapObj::Class(..) = self.heap.get(callee) {
            // The recursive `exec_call` below only encodes positional count, kwargs would silently disappear before reaching `__init__`, so reject them here.
            if !kw_flat.is_empty() {
                return Err(cold_type("class constructor takes no keyword arguments"));
            }
            let instance = self.heap.alloc(HeapObj::Instance(callee, Rc::new(RefCell::new(DictMap::new()))))?;
            if let Some((init_fn, defining)) = self.lookup_class_member(callee, "__init__") {
                // Fail-fast before pushing, the inner check fires only after parse_call_args pops.
                if self.depth >= self.max_calls { return Err(cold_depth()); }
                self.pending.method_binding = Some((defining, instance));
                self.push(init_fn);
                self.push(instance);
                for a in positional { self.push(*a); }
                let argc = (1 + positional.len()) as u16;
                self.exec_call(argc, chunk, slots)?;
                // Discard `__init__` return value.
                self.pop()?;
            }
            self.push(instance);
            return Ok(true);
        }

        // Bound user method: prepend `self` to the arg list and re-dispatch.
        if let HeapObj::BoundUserMethod(recv, func, class) = self.heap.get(callee) {
            // Same as Class branch: depth check before mutating the stack.
            if self.depth >= self.max_calls { return Err(cold_depth()); }
            let (recv, func, class) = (*recv, *func, *class);
            self.pending.method_binding = Some((class, recv));
            self.push(func);
            self.push(recv);
            for a in positional { self.push(*a); }
            let argc = (positional.len() + 1) as u16;
            let encoded = ((num_kw as u16) << 8) | argc;
            self.exec_call(encoded, chunk, slots)?;
            return Ok(true);
        }

        // `prop.setter(fn)` returns a new `Property` carrying the original getter plus the supplied setter.
        if let HeapObj::PropertySetter(prop_val) = self.heap.get(callee) {
            if positional.len() != 1 || !kw_flat.is_empty() {
                return Err(cold_type("property.setter takes exactly 1 argument"));
            }
            let prop_val = *prop_val;
            let getter = match self.heap.get(prop_val) {
                HeapObj::Property(g, _) => *g,
                _ => return Err(cold_runtime("PropertySetter wraps a non-Property value")),
            };
            let new_setter = positional[0];
            let new_prop = self.heap.alloc(HeapObj::Property(getter, new_setter))?;
            self.push(new_prop);
            return Ok(true);
        }

        // Instance with `__call__`, bind and dispatch through `BoundUserMethod`-style flow.
        if let HeapObj::Instance(..) = self.heap.get(callee)
            && let Some((func, class)) = self.lookup_class_member(
                match self.heap.get(callee) { HeapObj::Instance(c, _) => *c, _ => unreachable!() },
                "__call__")
        {
            if !kw_flat.is_empty() { return Err(cold_type("__call__ does not accept keyword arguments")); }
            if self.depth >= self.max_calls { return Err(cold_depth()); }
            self.pending.method_binding = Some((class, callee));
            self.push(func);
            self.push(callee);
            for a in positional { self.push(*a); }
            let argc = (positional.len() + 1) as u16;
            self.exec_call(argc, chunk, slots)?;
            return Ok(true);
        }

        // Resume a suspended coroutine; the inner yield must NOT propagate to the caller.
        if let HeapObj::Coroutine(..) = self.heap.get(callee) {
            let result = self.resume_coroutine(callee)?;
            if self.yielded { self.yielded = false; }
            self.push(result);
            return Ok(true);
        }

        Ok(false)
    }

    /* Bind formal params from positional/kw buffers, then fill remaining undef slots with defaults and captures. `defaults`/`captures` are pre-snapshotted by `exec_call`. */
    fn bind_function_args(&mut self, fi: usize, defaults: &[Val], captures: &[(usize, Val)], positional: &[Val], kw_flat: &[Val], fn_slots: &mut [Val]) -> Result<(), VmErr> {
        // Index by position to avoid an iterator borrow on `param_slots` across `heap.alloc`.
        let n_params = self.param_slots[fi].len();
        // Without a `*args` sink, positionals past the normal params are an error.
        let has_star = self.param_slots[fi].iter().any(|(k, _)| matches!(k, ParamKind::Star));
        let normal_count = self.param_slots[fi].iter().filter(|(k, _)| matches!(k, ParamKind::Normal)).count();
        if !has_star && positional.len() > normal_count {
            return Err(cold_type("too many positional arguments"));
        }
        let mut pos_idx = 0usize;
        for i in 0..n_params {
            let (kind, slot) = self.param_slots[fi][i];
            match kind {
                ParamKind::DoubleStar => {
                    let dm = DictMap::from_pairs(kw_flat.chunks_exact(2).map(|p| (p[0], p[1])).collect());
                    let dict_val = self.heap.alloc(HeapObj::Dict(Rc::new(RefCell::new(dm))))?;
                    if slot < fn_slots.len() { fn_slots[slot] = dict_val; }
                }
                ParamKind::Star => {
                    // *args binds to an immutable tuple.
                    let rest: Vec<Val> = positional[pos_idx..].to_vec();
                    pos_idx = positional.len();
                    let tuple_val = self.heap.alloc(HeapObj::Tuple(rest))?;
                    if slot < fn_slots.len() { fn_slots[slot] = tuple_val; }
                }
                ParamKind::Normal => {
                    if pos_idx >= positional.len() { continue; }
                    if slot < fn_slots.len() { fn_slots[slot] = positional[pos_idx]; }
                    pos_idx += 1;
                }
                // KwOnly slots are NOT consumed positionally; they bind only via kwargs.
                ParamKind::KwOnly => {}
            }
        }

        // Kwargs binding (rare path, not optimised).
        if !kw_flat.is_empty() {
            let params = &self.functions[fi].0;
            let body_map = &self.body_maps[fi];
            for pair in kw_flat.chunks_exact(2) {
                // Malformed `**`/kwarg bytecode can leave a non-string in the name slot; guard the heap access.
                let key = match self.heap.try_get(pair[0]) {
                    Some(HeapObj::Str(s)) => s.clone(),
                    _ => return Err(cold_runtime("malformed kwarg on stack")),
                };
                if params.iter().any(|p| crate::modules::parser::types::param_base_name(p) == key.as_str()) {
                    let pname = s!(str &key, "_0");
                    if let Some(&s) = body_map.get(pname.as_str()) {
                        fn_slots[s] = pair[1];
                    }
                }
            }
        }

        // Defaults: only fill slots still undef after binding.
        if !defaults.is_empty() {
            let ds = &self.default_slots[fi];
            for (di, &dv) in defaults.iter().enumerate() {
                if let Some(&(slot, _)) = ds.get(di)
                    && slot < fn_slots.len() && fn_slots[slot].is_undef() {
                        fn_slots[slot] = dv;
                    }
            }
        }

        // Closure captures: same rule as defaults, only fill if undef.
        for &(bi, val) in captures {
            if bi < fn_slots.len() && fn_slots[bi].is_undef() {
                fn_slots[bi] = val;
            }
        }

        Ok(())
    }

    /* Push caller slots into body slots. Same scope: late-binding, overwrite freely. Different scope: skip capture-filled slots (fixes stacked-decorator clobber). */
    fn apply_caller_slot_propagation(&self, fi: usize, captures: &[(usize, Val)], chunk: &SSAChunk, slots: &[Val], fn_slots: &mut [Val]) {
        let body_map = &self.body_maps[fi];
        let param_bm = &self.is_param_slot[fi];
        let caller_fi = self.body_to_fi.get(&(chunk as *const _)).copied();
        let callee_parent_fi = self.function_parents.get(fi).and_then(|x| *x);
        // Same-scope also requires same module, keeps top-level imports (`parent_fi == None`) isolated.
        let caller_module = caller_fi.and_then(|cf| self.fn_module.get(cf).cloned().flatten());
        let callee_module = self.fn_module.get(fi).cloned().flatten();
        let same_scope = caller_fi == callee_parent_fi && caller_module == callee_module;
        let captured_set: crate::util::fx::FxHashSet<usize> = if same_scope {
            crate::util::fx::FxHashSet::default()
        } else {
            captures.iter().map(|(s, _)| *s).collect()
        };
        for (si, &v) in slots.iter().enumerate() {
            if !v.is_undef()
                && let Some(name) = chunk.names.get(si)
                && let Some(&bs) = body_map.get(name.as_str())
                && !param_bm.get(bs).copied().unwrap_or(false)
                && !captured_set.contains(&bs)
            {
                fn_slots[bs] = v;
            }
        }

        // Bare-name fallback: body refs `<base>_0` but caller may store a higher SSA version. `resolve_free_name` centralises the three-layer order; captured slots are skipped.
        let free_loads = &self.body_free_loads[fi];
        for (bare, bs) in free_loads {
            if captured_set.contains(bs) { continue; }
            if let Some(v) = self.resolve_free_name(fi, bare, chunk, slots) {
                fn_slots[*bs] = v;
            }
        }
    }

    /* Three-layer fallback for a bare free-load name: caller's latest SSA -> callee module attrs -> entry globals. First hit wins. Centralised so the order is auditable. */
    fn resolve_free_name(&self, fi: usize, bare: &str, chunk: &SSAChunk, slots: &[Val]) -> Option<Val> {
        // Layer 1: caller's most-recent SSA version of `bare`.
        if let Some(idx) = self.chunk_name_versions.get(&(chunk as *const _)) && let Some(versions) = idx.get(bare)
        {
            let mut latest_ver: i64 = -1;
            let mut latest_v: Val = Val::undef();
            for &(v, si) in versions {
                if si < slots.len() && !slots[si].is_undef() && v > latest_ver {
                    latest_ver = v;
                    latest_v = slots[si];
                }
            }
            if !latest_v.is_undef() { return Some(latest_v); }
        }
        // Layer 2: callee's module attrs, keeps `a.helper` and `b.helper` isolated.
        if let Some(Some(spec)) = self.fn_module.get(fi).cloned()
            && let Some(mod_val) = self.module_table.get(&spec).copied()
            && mod_val.is_heap()
            && let HeapObj::Module(_, attrs) = self.heap.get(mod_val)
            && let Some((_, v)) = attrs.iter().find(|(n, _)| n == bare)
        {
            return Some(*v);
        }
        // Layer 3: globals, catches forward-ref mutual recursion in the entry chunk.
        self.globals.get(bare).copied()
    }

    /* Bind the function's own name slot to `callee` so recursive calls skip the global lookup. No-op for lambdas or when an earlier phase already filled the slot. */
    fn bind_self_reference(&self, fi: usize, callee: Val, fn_slots: &mut [Val]) {
        if let Some(slot) = self.self_ref_slot.get(fi).copied().flatten()
            && slot < fn_slots.len()
            && fn_slots[slot].is_undef()
        {
            fn_slots[slot] = callee;
        }
    }

    /* Run the body with caller slots pinned in `live_slots` (GC roots) and a CallFrame on `call_stack` (traceback). Frame popped on success only; the dispatch catch clears it on swallowed exceptions. Returns `(callee_impure, exec_result)`. */
    fn run_body_with_frame(&mut self, fi: usize, body: &SSAChunk, chunk: &SSAChunk, fn_slots: &mut [Val], slots: &[Val]) -> (bool, Result<Val, VmErr>) {
        // `mark()` short-circuits on non-heap values, so the whole slice is fine.
        let snap = self.live_slots.len();
        self.live_slots.extend_from_slice(slots);

        // Frame snapshots caller's source/path so render doesn't borrow live chunk pointers.
        let call_byte_pos = self.pending.call_byte_pos.take().unwrap_or(0);
        // Method-call paths set `method_binding` immediately before invoking `exec_call`; plain function calls leave it `None`.
        let (current_class, current_self) = match self.pending.method_binding.take() {
            Some((c, s)) => (Some(c), Some(s)),
            None => (None, None),
        };
        self.call_stack.push(super::super::types::CallFrame {
            fi,
            call_byte_pos,
            caller_source: chunk.source.clone(),
            caller_path: chunk.path.clone(),
            current_class,
            current_self,
        });

        self.observed_impure.push(false);
        let exec_result = self.exec(body, fn_slots);
        let callee_impure = self.observed_impure.pop().unwrap_or(true);
        self.live_slots.truncate(snap);
        if exec_result.is_ok() {
            self.call_stack.pop();
        }
        (callee_impure, exec_result)
    }

    /* Back-propagate `nonlocal` writes to the caller's slots and sync the callee Func's capture entries so the next call sees the new value. No-op if no `nonlocal`. */
    fn back_propagate_nonlocals(&mut self, fi: usize, body: &SSAChunk, callee: Val, chunk: &SSAChunk, slots: &mut [Val], fn_slots: &[Val]) {
        if self.nonlocal_tables[fi].is_empty() { return; }
        // Snapshot to release borrows on self before the `heap.get_mut` writes.
        let nl_pairs: Vec<(usize, usize)> = self.nonlocal_tables[fi].clone();
        let name_index = self.chunk_name_versions.get(&(chunk as *const _));
        for (canon_body, _) in nl_pairs {
            if let Some(&val) = fn_slots.get(canon_body) {
                if val.is_undef() { continue; }
                for base in &body.nonlocals {
                    if let Some(idx) = name_index && let Some(versions) = idx.get(base.as_str())
                    {
                        for &(_, si) in versions {
                            if si < slots.len() { slots[si] = val; }
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


    /* CallExtern: operand packs `(extern_idx<<8)|(kw<<4)|pos`. Pop kw `name,val` pairs then `pos` positional vals, pack pairs into a heap dict via `pack_kw_dict` and hand it off as the explicit `Option<Val>` kwargs slot. Pure externs leave the impurity flag alone, bodies whose only side-effects are pure externs stay memoizable. */
    pub(crate) fn call_extern(&mut self, operand: u16, chunk: &SSAChunk) -> Result<(), VmErr> {
        let extern_idx = (operand >> 8) as usize;
        let kw = ((operand >> 4) & 0xF) as usize;
        let pos = (operand & 0xF) as usize;
        let extern_fn = chunk.extern_table.get(extern_idx).ok_or(cold_runtime("CallExtern: extern index out of bounds"))?;
        let func = extern_fn.func.clone(); // Arc clone, refcount bump only
        let pure = extern_fn.pure;
        let kw_flat = if kw > 0 { self.pop_n(kw * 2)? } else { Vec::new() };
        let positional = self.pop_n(pos)?;
        let kwargs = Self::pack_kw_dict(&mut self.heap, &kw_flat)?;
        if !pure { self.mark_impure(); }
        match func(&mut self.heap, &positional, kwargs) {
            Ok(result) => { self.push(result); Ok(()) }
            // Native deferred; assign a correlation id and park with a `None` placeholder that
            // `set_host_result_by_id` overwrites before resume.
            Err(VmErr::HostCallDeferred) => {
                self.push(Val::none());
                self.pending.host_call_id = self.next_host_call_id;
                self.next_host_call_id += 1;
                self.pending.host_call_request = true;
                self.yielded = true;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub(crate) fn dispatch_native(&mut self, id: super::super::types::NativeFnId, positional: &[Val], kw: &[Val], chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        use super::super::types::NativeFnId::*;

        // `sorted()` is the only builtin taking kwargs (`key=`, `reverse=`); extract them before the no-kw guard.
        let mut sort_key: Option<Val> = None;
        let mut sort_reverse = false;
        let leftover_storage: Vec<Val>;
        let kw_remaining: &[Val] = if id == Sorted {
            let mut leftover: Vec<Val> = Vec::new();
            for chunk_pair in kw.chunks(2) {
                let (name_v, val_v) = (chunk_pair[0], chunk_pair[1]);
                let is_key = name_v.is_heap() && matches!(self.heap.get(name_v), HeapObj::Str(s) if s == "key");
                let is_reverse = name_v.is_heap() && matches!(self.heap.get(name_v), HeapObj::Str(s) if s == "reverse");
                if is_key { sort_key = Some(val_v); }
                else if is_reverse { sort_reverse = self.truthy(val_v); }
                else { leftover.push(name_v); leftover.push(val_v); }
            }
            leftover_storage = leftover;
            &leftover_storage
        } else { kw };

        if !kw_remaining.is_empty() {
            return Err(cold_type("native function takes no keyword arguments"));
        }
        let argc = positional.len() as u16;

        // Pre-validate fixed arity to keep the stack clean on error.
        let expected: Option<u16> = match id {
            Input | Receive => Some(0),
            Len | Abs | Str | Int | Float | Bool | Type | Chr | Ord
            | Sorted | Enumerate | List | Tuple | Bin | Oct | Hex
            | Repr | Reversed | Callable | Id | Hash | Next | Sleep
            | Iter => Some(1),
            Divmod | IsInstance | IsSubclass | HasAttr | Map | Filter | DelAttr => Some(2),
            SetAttr => Some(3),
            WithTimeout => Some(2),
            Cancel => Some(1),
            BytesFromHex => Some(1),
            IntFromBytes => Some(2),
            IntToBytes => Some(3),
            Globals | Locals | Super => Some(0),
            Property => None, // 1 or 2 args, validated in `call_property`.
            Bytes => None, // 0/1/2-arg: bytes() | bytes(n|iter) | bytes(str, "utf-8")
            Slice => None, // 1/2/3-arg
            Gather => None, // variadic
            FrozenSet => None, // 0/1-arg
            Vars => Some(1),
            ImportModule => Some(1),
            _ => None,
        };
        if let Some(n) = expected
            && argc != n { return Err(cold_type("wrong number of arguments to builtin")); }

        for &v in positional { self.push(v); }

        match id {
            // Variadic
            Print => {
                // CallPrint is statement-shaped (no trailing Pop); when reached via Call the parser emits Pop, so push None to keep the stack balanced.
                self.call_print(argc, chunk, slots)?;
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
            Format => self.call_format(argc, chunk, slots),
            // 0/1/2-arg
            Input => self.call_input(),
            Len => self.call_len(chunk, slots),
            Abs => self.call_abs(),
            Str => self.call_str(chunk, slots),
            Int => self.call_int(argc),
            Float => self.call_float(),
            Bool => self.call_bool(chunk, slots),
            Type => self.call_type(),
            Chr => self.call_chr(),
            Ord => self.call_ord(),
            Sorted => self.call_sorted_with_key(sort_key, sort_reverse, chunk, slots),
            Enumerate => self.call_enumerate(),
            List => self.call_list(chunk, slots),
            Tuple => self.call_tuple(chunk, slots),
            Bin => self.call_bin(),
            Oct => self.call_oct(),
            Hex => self.call_hex(),
            Repr => self.call_repr(chunk, slots),
            Reversed => self.call_reversed(),
            Callable => self.call_callable(),
            Id => self.call_id(),
            Hash => self.call_hash(chunk, slots),
            Divmod => self.call_divmod(),
            IsInstance => self.call_isinstance(),
            IsSubclass => self.call_issubclass(),
            HasAttr => self.call_hasattr(),
            Next => self.call_next(),
            Run => self.call_run(argc),
            Sleep => self.call_sleep(),
            Frame => self.call_frame(),
            Receive => self.call_receive(),
            Map => self.call_map(chunk, slots),
            Filter => self.call_filter(chunk, slots),
            Iter => self.call_iter(),
            Bytes => self.call_bytes(argc),
            Slice => self.call_slice(argc),
            Vars => self.call_vars(),
            SetAttr => self.call_setattr(),
            DelAttr => self.call_delattr(),
            ImportModule => self.call_import_module(),
            Gather => self.call_gather(argc),
            WithTimeout => self.call_with_timeout(),
            Cancel => self.call_cancel(),
            BytesFromHex => self.call_bytes_fromhex(),
            IntFromBytes => self.call_int_from_bytes(),
            IntToBytes => self.call_int_to_bytes(),
            FrozenSet => self.call_frozenset(argc),
            Globals => self.call_globals(chunk, slots),
            Locals => self.call_locals(chunk, slots),
            Super => self.call_super(),
            Property => self.call_property(argc),
        }
    }
}
