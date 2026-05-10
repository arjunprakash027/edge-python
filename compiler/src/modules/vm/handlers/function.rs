use crate::s;
use super::*;
use super::super::ParamKind;

use crate::alloc::string::ToString;

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
            OpCode::CallExtern   => self.call_extern(operand, chunk),
            _ => Err(cold_runtime("non-function opcode in handle_function")),
        }
    }

    pub(crate) fn exec_bound_method(
        &mut self, recv: Val,
        id: super::methods::BuiltinMethodId,
        pos: &[Val], kw: &[Val],
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
        let param_names: crate::util::fx::FxHashSet<String> = params.iter().map(|p| s!(str p.trim_start_matches(['*', '~']), "_0")).collect();
        let mut captures: Vec<(usize, Val)> = Vec::new();
        // Capture closure values once per canonical (coalesced) slot, skipping
        // names already bound as formal parameters. The body.names list is
        // typically <30, so a linear scan over chunk.names is competitive
        // with a HashMap and avoids a per-call monomorphization.
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

        // Top-level defs in the entry chunk go into `globals` so the
        // call-site free-load fallback in `exec_call` resolves forward
        // references — `def is_even` defined before `def is_odd` in
        // the same module captures nothing useful at MakeFunction time,
        // but at CALL time is_odd is in globals and the lookup succeeds.
        // Module-level defs are NOT registered here: they live in the
        // module's bindings (looked up via `fn_module[fi]` at call time)
        // so cross-module helpers with the same name stay isolated.
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

    /* Top-level orchestrator for the `Call` opcode. Pops the callee + its
       arguments off the stack, then routes through the appropriate
       sub-helper based on what the callee actually is. The user-defined
       `Func` path is the only one that builds a fresh `fn_slots` frame and
       runs the body inline; every other kind (BoundMethod, NativeFn,
       Extern, exception Type, Class instantiation, BoundUserMethod,
       Coroutine resume) short-circuits in `try_dispatch_non_func_callable`. */
    pub(crate) fn exec_call(&mut self, operand: u16, chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        let (positional, kw_flat, _num_pos, num_kw) = self.parse_call_args(operand)?;

        if self.depth >= self.max_calls { return Err(cold_depth()); }

        let callee = self.pop()?;
        if !callee.is_heap() { return Err(cold_type("object is not callable")); }

        if self.try_dispatch_non_func_callable(callee, &positional, &kw_flat, num_kw, chunk, slots)? {
            return Ok(());
        }

        let fi = match self.heap.get(callee) {
            HeapObj::Func(i, _, _) => *i,
            _ => return Err(cold_type("object is not callable")),
        };

        // Pure-call memoisation: skip the whole body if a prior call with
        // the same args already produced a result. Disabled when an outer
        // frame is impure (would memoise a stale view of the world) or
        // when kwargs are in play (cache key only spans positional args).
        let outer_impure = self.observed_impure.last().copied().unwrap_or(false);
        if num_kw == 0 && !outer_impure
            && let Some(cached) = self.templates.lookup(fi, &positional, &self.heap) {
                self.push(cached);
                return Ok(());
        }

        self.depth += 1;
        let (_params, body, _, name_idx_ref) = self.functions[fi];
        let name_idx = *name_idx_ref;
        let mut fn_slots = self.slot_templates[fi].clone();

        self.bind_function_args(fi, callee, &positional, &kw_flat, &mut fn_slots)?;

        if self.needs_caller_slots[fi] {
            self.apply_caller_slot_propagation(fi, callee, chunk, slots, &mut fn_slots);
        }

        self.bind_self_reference(fi, name_idx, callee, chunk, &mut fn_slots);

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
        let (callee_impure, exec_result) = self.run_body_with_frame(fi, body, chunk, &mut fn_slots, slots);
        self.depth -= 1;

        self.back_propagate_nonlocals(fi, body, callee, chunk, slots, &fn_slots);

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

    /* Decode the call's `operand` (low 8 bits = positional count, high 8
       bits = keyword pairs), apply pending star-spread deltas, then pop
       `num_pos + 2 * num_kw` items off the stack into separate
       positional/kw buffers. The kw vector is alternating name/value pairs
       as the parser emits it. */
    fn parse_call_args(&mut self, operand: u16) -> Result<(Vec<Val>, Vec<Val>, usize, usize), VmErr> {
        let raw = operand as usize;

        let base_pos = (raw & 0xFF)        as i32;
        let base_kw  = ((raw >> 8) & 0xFF) as i32;
        let num_pos = (base_pos + self.pending_pos_delta).max(0) as usize;
        let num_kw  = (base_kw  + self.pending_kw_delta ).max(0) as usize;
        self.pending_pos_delta = 0;
        self.pending_kw_delta  = 0;

        let total_items = num_pos + 2 * num_kw;
        let mut stack_items: Vec<Val> = (0..total_items)
            .map(|_| self.pop())
            .collect::<Result<_, _>>()?;
        stack_items.reverse();

        let kw_flat: Vec<Val> = stack_items.split_off(num_pos);
        Ok((stack_items, kw_flat, num_pos, num_kw))
    }

    /* Handle every callee kind that ISN'T a user-defined `Func`. Returns
       Ok(true) when the callee was dispatched here; Ok(false) means the
       caller should continue with the user-Func code path. The early
       returns mirror the original exec_call layout: each kind clones what
       it needs out of the heap borrow before invoking helpers that need
       `&mut self`. */
    fn try_dispatch_non_func_callable(
        &mut self, callee: Val,
        positional: &[Val], kw_flat: &[Val], num_kw: usize,
        chunk: &SSAChunk, slots: &mut [Val],
    ) -> Result<bool, VmErr> {
        if let HeapObj::BoundMethod(recv, id) = self.heap.get(callee) {
            let recv = *recv;
            let id = *id;
            self.exec_bound_method(recv, id, positional, kw_flat)?;
            return Ok(true);
        }

        if let HeapObj::NativeFn(id) = self.heap.get(callee) {
            let id = *id;
            self.dispatch_native(id, positional, kw_flat, chunk, slots)?;
            return Ok(true);
        }

        if let HeapObj::Extern(extern_fn) = self.heap.get(callee) {
            if !kw_flat.is_empty() {
                return Err(cold_type("extern function takes no keyword arguments"));
            }
            let func = extern_fn.func.clone();
            let pure = extern_fn.pure;
            if !pure { self.mark_impure(); }
            let result = func(&mut self.heap, positional)?;
            self.push(result);
            return Ok(true);
        }

        // Calling a builtin Type: build an ExcInstance carrying the type name
        // and args. Used for `raise ValueError("msg")` and friends; `e.args`
        // exposes the args tuple. Conversion-style types (int/float/...) are
        // routed through specialised opcodes by the parser, so this path is
        // overwhelmingly hit by exception construction in practice.
        if let HeapObj::Type(name) = self.heap.get(callee) {
            let name = name.clone();
            if !kw_flat.is_empty() {
                return Err(cold_type("exception class takes no keyword arguments"));
            }
            let exc = self.heap.alloc(HeapObj::ExcInstance(name, positional.to_vec()))?;
            self.push(exc);
            return Ok(true);
        }

        // Calling a class: create an instance and run __init__ if defined.
        if let HeapObj::Class(_, methods) = self.heap.get(callee) {
            // The recursive `exec_call(argc, ...)` below only encodes
            // positional count, so kwargs would silently disappear before
            // reaching `__init__`. Surface that explicitly.
            if !kw_flat.is_empty() {
                return Err(cold_type("class constructor takes no keyword arguments"));
            }
            let methods = methods.clone();
            let instance = self.heap.alloc(HeapObj::Instance(callee, Rc::new(RefCell::new(DictMap::new()))))?;
            if let Some((_, init_fn)) = methods.iter().find(|(n, _)| n == "__init__") {
                let init_fn = *init_fn;
                self.push(init_fn);
                let mut args = vec![instance];
                args.extend_from_slice(positional);
                for a in &args { self.push(*a); }
                let argc = args.len() as u16;
                self.exec_call(argc, chunk, slots)?;
                // Discard __init__'s return value.
                self.pop()?;
            }
            self.push(instance);
            return Ok(true);
        }

        // Bound user method: prepend `self` to the arg list and re-dispatch.
        if let HeapObj::BoundUserMethod(recv, func) = self.heap.get(callee) {
            let (recv, func) = (*recv, *func);
            self.push(func);
            self.push(recv);
            for a in positional { self.push(*a); }
            let argc = (positional.len() + 1) as u16;
            let encoded = ((num_kw as u16) << 8) | argc;
            self.exec_call(encoded, chunk, slots)?;
            return Ok(true);
        }

        // Resume a suspended coroutine; the inner yield must NOT propagate
        // to the surrounding function call.
        if let HeapObj::Coroutine(..) = self.heap.get(callee) {
            let result = self.resume_coroutine(callee)?;
            if self.yielded { self.yielded = false; }
            self.push(result);
            return Ok(true);
        }

        Ok(false)
    }

    /* Bind formal parameters from the popped positional/kw buffers, then
       fill remaining un-bound slots with any defaults and closure captures
       attached to the callee Func. Operates on the freshly-cloned
       `fn_slots` for the callee body. */
    fn bind_function_args(
        &mut self, fi: usize, callee: Val,
        positional: &[Val], kw_flat: &[Val],
        fn_slots: &mut [Val],
    ) -> Result<(), VmErr> {
        // Param binding via pre-computed param_slots. Cloned because the
        // DoubleStar/Star arms call `self.heap.alloc`, which needs `&mut
        // self` and would conflict with an in-flight `&self.param_slots`
        // iterator borrow.
        let pslots = self.param_slots[fi].clone();
        let mut pos_idx = 0usize;
        for (kind, slot) in pslots {
            match kind {
                ParamKind::DoubleStar => {
                    let dm = DictMap::from_pairs(kw_flat.chunks_exact(2).map(|p| (p[0], p[1])).collect());
                    let dict_val = self.heap.alloc(HeapObj::Dict(Rc::new(RefCell::new(dm))))?;
                    if slot < fn_slots.len() { fn_slots[slot] = dict_val; }
                }
                ParamKind::Star => {
                    let rest: Vec<Val> = positional[pos_idx..].to_vec();
                    pos_idx = positional.len();
                    let list_val = self.heap.alloc(HeapObj::List(Rc::new(RefCell::new(rest))))?;
                    if slot < fn_slots.len() { fn_slots[slot] = list_val; }
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
                let key = match self.heap.get(pair[0]) {
                    HeapObj::Str(s) => s.clone(),
                    _ => return Err(cold_runtime("malformed kwarg on stack")),
                };
                // Param prefixes (`*`, `**`, `~`) decorate the declared name —
                // strip them to compare with the keyword arg key.
                if params.iter().any(|p| p.trim_start_matches(['*', '~']) == key.as_str()) {
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

        Ok(())
    }

    /* Propagate caller slots into matching body slots, then fall back on
       bare-name + module-attr + globals lookups for free-load slots that
       didn't find a name match. Two regimes, selected by whether the
       caller is the callee's lexical parent:

         same scope (caller_fi == callee.parent_fi)
           Late-binding: overwrite freely so a lambda inside `def f`
           reading an outer-scope var sees the current value, not the
           snapshot taken at MakeFunction time.

         different scope
           Closure semantics: skip slots filled by captures so a closure
           created elsewhere keeps its captured values when invoked. Fixes
           stacked decorators where each layer's `w` captures its own
           `f` — without the guard the outer caller's `f` overwrote the
           inner's captured `f` and the closure recursed forever.

       is_param_slot remains the hard guard for formal parameters bound
       by the call. */
    fn apply_caller_slot_propagation(
        &self, fi: usize, callee: Val,
        chunk: &SSAChunk, slots: &[Val], fn_slots: &mut [Val],
    ) {
        let body_map = &self.body_maps[fi];
        let param_bm = &self.is_param_slot[fi];
        let caller_fi = self.body_to_fi.get(&(chunk as *const _)).copied();
        let callee_parent_fi = self.function_parents.get(fi).and_then(|x| *x);
        // "Same scope" means the callee was defined in the caller's
        // OWN scope — late-binding via caller slots is then correct
        // (it's mutual recursion of sibling defs). Crossing a module
        // boundary breaks that assumption: a function imported from
        // module M shouldn't have its captured free vars rebound by
        // the importer's slots, even if both happen to be top-level
        // (parent_fi == None). Comparing fn_module on both sides
        // restores per-module isolation that the old splice path
        // achieved via name mangling.
        let caller_module = caller_fi.and_then(|cf| self.fn_module.get(cf).cloned().flatten());
        let callee_module = self.fn_module.get(fi).cloned().flatten();
        let same_scope = caller_fi == callee_parent_fi
            && caller_module == callee_module;
        let captured_set: crate::util::fx::FxHashSet<usize> = if same_scope {
            crate::util::fx::FxHashSet::default()
        } else if let HeapObj::Func(_, _, captures) = self.heap.get(callee) {
            captures.iter().map(|(s, _)| *s).collect()
        } else {
            crate::util::fx::FxHashSet::default()
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

        // Bare-name fallback for free-load slots: when the body's reference
        // records `<base>_0` (the version current at body-compile time) but
        // the caller now stores `<base>` under a higher SSA version, exact-
        // name match misses. Find the caller's most-recent slot for the
        // bare name and propagate. Required for mutual recursion across
        // top-level defs in a code module — the splicer ends up storing
        // sibling defs as `_1+` while each body still records `_0`. Skips
        // capture-protected slots so closures keep their captured values.
        let free_loads = &self.body_free_loads[fi];
        for (bare, bs) in free_loads {
            if captured_set.contains(bs) { continue; }
            let mut latest_ver: i64 = -1;
            let mut latest_v: Val = Val::undef();
            for (si, sname) in chunk.names.iter().enumerate() {
                if let Some(p) = sname.rfind('_')
                    && &sname[..p] == bare.as_str()
                    && let Ok(v) = sname[p+1..].parse::<i64>()
                    && si < slots.len()
                    && !slots[si].is_undef()
                    && v > latest_ver
                {
                    latest_ver = v;
                    latest_v = slots[si];
                }
            }
            if !latest_v.is_undef() {
                fn_slots[*bs] = latest_v;
                continue;
            }
            // Module-bindings fallback: if the callee was defined in
            // an imported module, look up `bare` in that module's
            // attrs first. Cross-module name collisions stay isolated
            // — `a.helper` and `b.helper` resolve to their own
            // module's helper instead of clobbering each other in the
            // shared globals table.
            if let Some(Some(spec)) = self.fn_module.get(fi).cloned()
                && let Some(mod_val) = self.module_table.get(&spec).copied()
                && mod_val.is_heap()
                && let HeapObj::Module(_, attrs) = self.heap.get(mod_val)
                && let Some((_, v)) = attrs.iter().find(|(n, _)| n == bare.as_str())
            {
                fn_slots[*bs] = *v;
                continue;
            }
            // Globals fallback: catches forward-ref module-level mutual
            // recursion in the entry chunk (where module_table doesn't
            // apply because the entry isn't a "module"). Top-level defs
            // in entry register themselves in globals at MakeFunction
            // time for this lookup.
            if let Some(&v) = self.globals.get(bare.as_str()) {
                fn_slots[*bs] = v;
            }
        }
    }

    /* Bind the function's own name slot to `callee` so recursive calls
       resolve without a global lookup. No-op for anonymous lambdas
       (`name_idx == u16::MAX`) or when the slot was already filled by an
       earlier phase (params, captures, caller-slot propagation). */
    fn bind_self_reference(
        &self, fi: usize, name_idx: u16, callee: Val,
        chunk: &SSAChunk, fn_slots: &mut [Val],
    ) {
        if name_idx == u16::MAX { return; }
        let Some(raw_name) = chunk.names.get(name_idx as usize) else { return; };
        let base = ssa_strip(raw_name);
        let versioned = s!(str base, "_0");
        let body_map = &self.body_maps[fi];
        if let Some(&slot) = body_map.get(versioned.as_str())
            && fn_slots[slot].is_undef()
        {
            fn_slots[slot] = callee;
        }
    }

    /* Run the callee body with a snapshot of the caller's slots pinned in
       `live_slots` (so the GC can mark them) and a CallFrame on
       `call_stack` (so the traceback renderer can walk the chain). On
       success the frame is popped; on error we leave it in place — the
       error catch in dispatch is responsible for clearing the chain on
       swallowed exceptions. Returns `(callee_impure, exec_result)` so the
       caller can propagate impurity and inspect the body's outcome. */
    fn run_body_with_frame(
        &mut self, fi: usize, body: &SSAChunk, chunk: &SSAChunk,
        fn_slots: &mut [Val], slots: &[Val],
    ) -> (bool, Result<Val, VmErr>) {
        // mark() short-circuits on non-heap values, so the whole slice is fine.
        let snap = self.live_slots.len();
        self.live_slots.extend_from_slice(slots);

        // The frame snapshots the caller's chunk source/path so render
        // works without holding a borrow on the live chunk pointers.
        let call_byte_pos = self.pending_call_byte_pos.take().unwrap_or(0);
        self.call_stack.push(super::super::types::CallFrame {
            fi,
            call_byte_pos,
            caller_source: chunk.source.clone(),
            caller_path: chunk.path.clone(),
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

    /* Back-propagate `nonlocal` writes from the callee's `fn_slots` to
       the caller's matching slots, and sync any closure-capture entries
       attached to the callee Func so a subsequent invocation sees the
       new value. No-op for bodies that don't declare `nonlocal`. */
    fn back_propagate_nonlocals(
        &mut self, fi: usize, body: &SSAChunk, callee: Val,
        chunk: &SSAChunk, slots: &mut [Val], fn_slots: &[Val],
    ) {
        let nl_table = &self.nonlocal_tables[fi];
        if nl_table.is_empty() { return; }
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


    /* Dispatch a `CallExtern` opcode: pop `argc` positional args, look up the
       extern function pointer in the chunk's extern_table, invoke it with
       direct heap access, and push the result. Operand encoding mirrors the
       parser's emit at literals.rs::call: high 8 bits = extern_idx, low 8
       bits = argc.

       Purity: impure externs taint the enclosing user function via
       `mark_impure`, mirroring the runtime tracking that enables template
       memoization to skip non-cacheable bodies. Pure externs leave the
       impurity flag untouched, so a user `def` whose only side-effects are
       calls to pure externs remains memoizable. */
    pub(crate) fn call_extern(&mut self, operand: u16, chunk: &SSAChunk) -> Result<(), VmErr> {
        let extern_idx = (operand >> 8) as usize;
        let argc       = (operand & 0xFF) as usize;
        let extern_fn  = chunk.extern_table.get(extern_idx)
            .ok_or(cold_runtime("CallExtern: extern index out of bounds"))?;
        let func = extern_fn.func.clone();   // Arc clone — refcount bump only
        let pure = extern_fn.pure;
        let args = self.pop_n(argc)?;
        if !pure { self.mark_impure(); }
        let result = func(&mut self.heap, &args)?;
        self.push(result);
        Ok(())
    }

    pub(crate) fn dispatch_native(
        &mut self, id: super::super::types::NativeFnId,
        positional: &[Val], kw: &[Val],
        chunk: &SSAChunk, slots: &mut [Val],
    ) -> Result<(), VmErr> {
        use super::super::types::NativeFnId::*;

        // sorted() is the one builtin that accepts keyword arguments
        // (`sorted(xs, key=fn)`). Pull the key out of kw_flat (slice of
        // alternating name/value) before the generic "no kwargs" check.
        let mut sort_key: Option<Val> = None;
        let leftover_storage: Vec<Val>;
        let kw_remaining: &[Val] = if id == Sorted {
            let mut leftover: Vec<Val> = Vec::new();
            for chunk_pair in kw.chunks(2) {
                let (name_v, val_v) = (chunk_pair[0], chunk_pair[1]);
                let is_key = name_v.is_heap()
                    && matches!(self.heap.get(name_v), HeapObj::Str(s) if s == "key");
                if is_key {
                    sort_key = Some(val_v);
                } else {
                    leftover.push(name_v);
                    leftover.push(val_v);
                }
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
            Divmod | IsInstance | HasAttr | Map | Filter | DelAttr => Some(2),
            SetAttr => Some(3),
            WithTimeout => Some(2),
            Cancel => Some(1),
            BytesFromHex => Some(1),
            IntFromBytes => Some(2),
            IntToBytes => Some(3),
            Globals | Locals => Some(0),
            Bytes => None,  // 0/1/2-arg: bytes() | bytes(n|iter) | bytes(str, "utf-8")
            Slice => None,  // 1/2/3-arg
            Gather => None, // variadic
            FrozenSet => None, // 0/1-arg
            Vars => Some(1),
            ImportModule => Some(1),
            _ => None,
        };
        if let Some(n) = expected
            && argc != n {
                return Err(cold_type("wrong number of arguments to builtin"));
        }

        for &v in positional { self.push(v); }

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
            Sorted => self.call_sorted_with_key(sort_key, chunk, slots),
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
            Divmod => self.call_divmod(),
            IsInstance => self.call_isinstance(),
            HasAttr => self.call_hasattr(),
            Next => self.call_next(),
            Run => self.call_run(argc),
            Sleep => self.call_sleep(),
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
        }
    }
}