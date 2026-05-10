pub mod types;
mod cache;
mod ops;
mod builtins;
pub(crate) mod handlers;
pub mod optimizer;

mod dispatch;
mod gc;
mod helpers;
mod init;

use crate::s;
use crate::modules::parser::{SSAChunk, BUILTIN_TYPES};
use crate::util::fx::FxHashMap as HashMap;

pub use types::{Val, HeapObj, HeapPool, VmErr, Limits};

use types::*;
use cache::{OpcodeCache, Templates};
use alloc::{string::{String, ToString}, vec::Vec};

/* Saved stack/iter/with depths for unwinding to a try arm's handler. */
pub(crate) struct ExceptionFrame {
    pub handler_ip: usize,
    pub stack_depth: usize,
    pub iter_depth: usize,
    pub with_depth: usize,
}

#[derive(Clone, Copy)]
pub(crate) enum ParamKind { Normal, Star, DoubleStar, KwOnly }

pub struct VM<'a> {
    pub(crate) stack: Vec<Val>,
    pub(crate) heap: HeapPool,
    pub(crate) iter_stack: Vec<IterFrame>,
    pub(crate) yields: Vec<Val>,
    pub(crate) chunk: &'a SSAChunk,
    pub(crate) globals: HashMap<String, Val>,
    pub(crate) live_slots: Vec<Val>,
    pub(crate) templates: Templates,
    pub(crate) budget: usize,
    pub(crate) depth: usize,
    pub(crate) max_calls: usize,
    pub(crate) observed_impure: Vec<bool>,
    pub(crate) exception_stack: Vec<ExceptionFrame>,
    pub(crate) functions: Vec<&'a (Vec<String>, SSAChunk, u16, u16)>,
    // (chunk_ptr, [global_fn_id; chunk.functions.len()]). Linear scan; one
    // entry per chunk (typically <20). Avoids HashMap monomorphization for
    // a tiny pointer-keyed map.
    pub(crate) fn_index: Vec<(*const SSAChunk, Vec<u32>)>,
    // function_parents[fi] = the fi of the def that lexically encloses `fi`,
    // or None for module-level. body_to_fi resolves a body chunk pointer to
    // its owning fi (for caller identification). See build_function_table.
    pub(crate) function_parents: Vec<Option<usize>>,
    pub(crate) body_to_fi: HashMap<*const SSAChunk, usize>,
    pub(crate) body_maps: Vec<HashMap<String, usize>>,
    pub(crate) param_slots: Vec<Vec<(ParamKind, usize)>>,
    pub(crate) slot_templates: Vec<Vec<Val>>,
    pub(crate) nonlocal_tables: Vec<Vec<(usize, usize)>>,
    pub(crate) needs_caller_slots: Vec<bool>,
    /* `is_param_slot[fi][slot]` — true when slot is bound to a formal
       parameter and must NOT be overwritten by caller-slot propagation.
       Replaces a per-call BTreeSet<usize> allocation in exec_call. */
    pub(crate) is_param_slot: Vec<Vec<bool>>,
    /* Body slots holding free-variable references (canonical, version-0,
       not a parameter). Each entry is `(bare_name, body_slot)`. exec_call
       falls back to base-name lookup against the caller's chunk for these
       slots so that names whose SSA version differs between body and caller
       still late-bind correctly. */
    pub(crate) body_free_loads: Vec<Vec<(String, usize)>>,
    pub(crate) is_async: Vec<bool>,
    pub(crate) default_slots: Vec<Vec<(usize, Val)>>,
    pub(crate) opcode_caches: HashMap<*const SSAChunk, OpcodeCache>,
    /* Const pool slice ptrs for caches currently owned by a live exec()
       frame (removed from `opcode_caches` for the duration of the call). */
    pub(crate) active_const_pools: Vec<*const [Val]>,
    /* Cached `Limits::ops == usize::MAX` so the hot dispatch path skips
       the budget decrement on every backward jump. */
    pub(crate) sandbox_off: bool,
    pub(crate) with_stack: Vec<Val>,
    pub(crate) pending_pos_delta: i32,
    pub(crate) pending_kw_delta: i32,
    pub(crate) yielded: bool,
    pub(crate) resume_ip: usize,
    pub output: Vec<String>,
    pub print_hook: Option<fn(&str)>,
    pub input_buffer: Vec<String>,
    pub event_queue: Vec<Val>,
    pub strict_input: bool,
    /* Source byte offset of the deepest frame that raised a propagating
       error in the most recent run(). */
    pub(crate) error_byte_pos: Option<u32>,
    /* Byte offset of the Call instruction currently being dispatched. */
    pub(crate) pending_call_byte_pos: Option<u32>,
    /* When `sleep(s)` yields, it stores the wakeup time here so the
       scheduler can move the handle to Sleeping(until_ns). */
    pub(crate) pending_sleep_until_ns: Option<u64>,
    /* When a `raise X("msg")` lifts an `ExcInstance`, we stash the Val here
       so the matching `except X as e` handler can bind the actual instance. */
    pub(crate) pending_exc_val: Option<Val>,
    /* spec -> Module Val map populated by `init_modules` before user
       bytecode runs. Both `OpCode::LoadModule` and the `import_module()`
       builtin look up here. */
    pub(crate) module_table: HashMap<String, Val>,
    /* `fi -> module spec`. Tracks which module a function lives in so
       the call-site free-load fallback resolves bare-name references
       against the function's OWN module's bindings instead of the
       global namespace. */
    pub(crate) fn_module: Vec<Option<String>>,
    /* Function-name registry parallel to `functions`. Populated when
       `MakeFunction` runs, consulted by the multi-frame traceback renderer
       to fill the `<fname>` placeholder in `note: called from <fname>()`
       lines. Empty string entries are anonymous lambdas. */
    pub(crate) function_names: Vec<String>,
    /* Active call frames pushed/popped by exec_call on every user-function
       entry and exit. Drained on error to feed the traceback renderer; the
       innermost (most recent) call is at the end of the Vec. */
    pub(crate) call_stack: Vec<CallFrame>,
    /* Cooperative scheduler for `run` / `gather` / `with_timeout`.
       Empty between top-level invocations of `run`; each handle holds
       a HeapObj::Coroutine Val plus its lifecycle state. */
    pub(crate) scheduler: Vec<CoroutineHandle>,
    /* Host-installed wall-clock provider in nanoseconds. WASM hosts
       wire this to `Date.now() * 1e6`; native hosts can use
       `std::time::Instant`. */
    pub(crate) time_hook: Option<fn() -> u64>,
    /* Internal monotonic counter used when `time_hook` is None: each
       `sleep(s)` call moves it forward by `s * 1e9`, so coroutines still
       wake in order even without a real clock. Resets at every `run()`. */
    pub(crate) virtual_clock_ns: u64,
}

impl<'a> VM<'a> {
    pub fn new(chunk: &'a SSAChunk) -> Self { Self::with_limits(chunk, Limits::none()) }

    pub fn with_limits(chunk: &'a SSAChunk, limits: Limits) -> Self {
        let sandbox_off = limits.ops == usize::MAX;
        let mut vm = Self {
            stack: Vec::with_capacity(256),
            iter_stack: Vec::with_capacity(16),
            yields: Vec::new(),
            chunk,
            heap: HeapPool::new(limits.heap),
            globals: HashMap::default(),
            live_slots: Vec::new(),
            templates: Templates::new(),
            budget: limits.ops,
            depth: 0,
            max_calls: limits.calls,
            with_stack: Vec::new(),
            pending_pos_delta: 0,
            pending_kw_delta: 0,
            yielded: false,
            resume_ip: 0,
            strict_input: false,
            output: Vec::new(),
            print_hook: None,
            input_buffer: Vec::new(),
            event_queue: Vec::new(),
            observed_impure: Vec::new(),
            exception_stack: Vec::new(),
            error_byte_pos: None,
            pending_call_byte_pos: None,
            pending_sleep_until_ns: None,
            pending_exc_val: None,
            module_table: HashMap::default(),
            fn_module: Vec::new(),
            function_names: Vec::new(),
            call_stack: Vec::new(),
            scheduler: Vec::new(),
            time_hook: None,
            virtual_clock_ns: 0,
            functions: Vec::new(),
            fn_index: Vec::new(),
            function_parents: Vec::new(),
            body_to_fi: HashMap::default(),
            body_maps: Vec::new(),
            param_slots: Vec::new(),
            slot_templates: Vec::new(),
            nonlocal_tables: Vec::new(),
            needs_caller_slots: Vec::new(),
            is_param_slot: Vec::new(),
            body_free_loads: Vec::new(),
            is_async: Vec::new(),
            default_slots: Vec::new(),
            opcode_caches: HashMap::default(),
            active_const_pools: Vec::new(),
            sandbox_off,
        };
        vm.build_function_table(chunk, None, None);
        vm.body_maps = vm.functions.iter().map(|(_, body, _, _)| {
            body.names.iter().enumerate().map(|(i, n)| (n.clone(), i)).collect()
        }).collect();
        vm.param_slots = (0..vm.functions.len()).map(|fi| {
            let (params, _, _, _) = vm.functions[fi];
            let bm = &vm.body_maps[fi];
            params.iter().map(|p| {
                // Prefix `~` marks parameters declared after a lone `*` separator.
                let (kind, bare) = if let Some(stripped) = p.strip_prefix("**") {
                    (ParamKind::DoubleStar, stripped)
                } else if let Some(stripped) = p.strip_prefix('*') {
                    (ParamKind::Star, stripped)
                } else if let Some(stripped) = p.strip_prefix('~') {
                    (ParamKind::KwOnly, stripped)
                } else {
                    (ParamKind::Normal, p.as_str())
                };
                let slot = bm.get(&s!(str bare, "_0")).copied().unwrap_or(usize::MAX);
                (kind, slot)
            }).collect()
        }).collect();

        // Pre-compute nonlocal resolution: (canonical_body_slot, canonical_body_slot).
        vm.nonlocal_tables = vm.functions.iter().map(|(_, body, _, _)| {
            body.nonlocals.iter().filter_map(|base| {
                // Must skip names that lack a `_<digits>` SSA suffix entirely:
                // body.names also holds the bare `Nonlocal` opcode operand,
                // and that slot isn't the variable's canonical SSA root. So
                // we explicitly require the suffix-bearing form here, not
                // ssa_strip's "fall through to bare on missing suffix" shape.
                let canon = body.names.iter().enumerate()
                    .find(|(_, n)| n.rfind('_').map(|p| &n[..p]) == Some(base.as_str()))
                    .map(|(i, _)| body.alias_groups.get(i).and_then(|g| g.first().copied()).unwrap_or(i as u16) as usize)?;
                Some((canon, canon))
            }).collect()
        }).collect();

        // True iff the body references names not in params/builtins/captures.
        vm.needs_caller_slots = (0..vm.functions.len()).map(|fi| {
            let (params, body, _, _) = vm.functions[fi];
            let param_names: crate::util::fx::FxHashSet<&str> = params.iter()
                .map(|p| p.trim_start_matches(['*', '~'])).collect();
            body.names.iter().any(|n| {
                let base = crate::modules::parser::ssa_strip(n);
                !param_names.contains(base) && !vm.globals.contains_key(n)
            })
        }).collect();

        // Bitmap of slots bound to formal parameters — used to skip caller-slot
        // propagation without allocating a BTreeSet per call.
        vm.is_param_slot = (0..vm.functions.len()).map(|fi| {
            let (_, body, _, _) = vm.functions[fi];
            let n_slots = body.names.len();
            let mut bm = alloc::vec![false; n_slots];
            for &(_, slot) in &vm.param_slots[fi] {
                if slot < n_slots { bm[slot] = true; }
            }
            bm
        }).collect();

        // Body free-load slots: canonical, non-parameter names that the body
        // never writes to. Built once at VM init.
        vm.body_free_loads = (0..vm.functions.len()).map(|fi| {
            let (_, body, _, _) = vm.functions[fi];
            let param_bm = &vm.is_param_slot[fi];
            let mut written: crate::util::fx::FxHashSet<usize> = crate::util::fx::FxHashSet::default();
            for ins in &body.instructions {
                if matches!(ins.opcode, crate::modules::parser::OpCode::StoreName | crate::modules::parser::OpCode::Phi) {
                    written.insert(ins.operand as usize);
                }
            }
            body.names.iter().enumerate().filter_map(|(slot, name)| {
                let canon = body.alias_groups.get(slot)
                    .and_then(|g| g.first().copied())
                    .unwrap_or(slot as u16) as usize;
                if canon != slot { return None; }
                if param_bm.get(slot).copied().unwrap_or(false) { return None; }
                if written.contains(&slot) { return None; }
                let p = name.rfind('_')?;
                name[p+1..].parse::<u32>().ok()?;
                Some((name[..p].to_string(), slot))
            }).collect()
        }).collect();

        // Default-slot table: (slot, placeholder) entries the call path overwrites.
        vm.default_slots = (0..vm.functions.len()).map(|fi| {
            let (params, _, n_defaults, _) = vm.functions[fi];
            let n_defaults = *n_defaults as usize;
            if n_defaults == 0 { return Vec::new(); }
            let pslots = &vm.param_slots[fi];
            let n_params = params.len();
            let offset = n_params.saturating_sub(n_defaults);
            (0..n_defaults).filter_map(|di| {
                pslots.get(offset + di).map(|&(_, slot)| (slot, Val::none()))
            }).collect()
        }).collect();
        for &name in BUILTIN_TYPES {
            if let Ok(type_obj) = vm.heap.alloc(HeapObj::Type(name.to_string())) {
                vm.globals.insert(name.to_string(), type_obj);
                vm.globals.insert(s!(str name, "_0"), type_obj);
            }
        }
        // Module identity. The entry chunk always runs as "__main__" so the
        // `if __name__ == "__main__":` guard works without special-casing in
        // the parser. Inserted before slot_templates is built below so name
        // references get pre-resolved into slots.
        if let Ok(main_name) = vm.heap.alloc(HeapObj::Str("__main__".to_string())) {
            vm.globals.insert("__name__".to_string(), main_name);
            vm.globals.insert("__name___0".to_string(), main_name);
        }
        // Register builtins as first-class NativeFn values so `print = print`,
        // `f = len; f([1,2])`, etc. work without a separate dispatch path.
        let builtin_fns: &[NativeFnId] = &[
            NativeFnId::Print, NativeFnId::Len, NativeFnId::Abs, NativeFnId::Str,
            NativeFnId::Int, NativeFnId::Float, NativeFnId::Bool, NativeFnId::Type,
            NativeFnId::Chr, NativeFnId::Ord, NativeFnId::Range, NativeFnId::Round,
            NativeFnId::Min, NativeFnId::Max, NativeFnId::Sum, NativeFnId::Sorted,
            NativeFnId::Enumerate, NativeFnId::Zip, NativeFnId::List, NativeFnId::Tuple,
            NativeFnId::Dict, NativeFnId::Set, NativeFnId::IsInstance, NativeFnId::Input,
            NativeFnId::All, NativeFnId::Any, NativeFnId::Bin, NativeFnId::Oct,
            NativeFnId::Hex, NativeFnId::Divmod, NativeFnId::Pow, NativeFnId::Repr,
            NativeFnId::Reversed, NativeFnId::Callable, NativeFnId::Id, NativeFnId::Hash,
            NativeFnId::Format, NativeFnId::GetAttr, NativeFnId::HasAttr,
            NativeFnId::SetAttr, NativeFnId::DelAttr, NativeFnId::Next,
            NativeFnId::Run, NativeFnId::Sleep, NativeFnId::Receive,
            NativeFnId::Map, NativeFnId::Filter, NativeFnId::Iter,
            NativeFnId::Bytes, NativeFnId::ImportModule,
            NativeFnId::Slice, NativeFnId::Vars,
            NativeFnId::Gather, NativeFnId::WithTimeout, NativeFnId::Cancel,
            NativeFnId::BytesFromHex, NativeFnId::IntFromBytes,
            NativeFnId::IntToBytes, NativeFnId::FrozenSet,
            NativeFnId::Globals, NativeFnId::Locals,
        ];
        for &id in builtin_fns {
            if let Ok(v) = vm.heap.alloc(HeapObj::NativeFn(id)) {
                let name = id.name();
                vm.globals.insert(name.to_string(), v);
                vm.globals.insert(s!(str name, "_0"), v);
            }
        }
        // Slot templates need every global already populated — built once
        // after the loop, not per builtin.
        vm.slot_templates = vm.functions.iter().map(|(_, body, _, _)| {
            vm.fill_builtins(&body.names)
        }).collect();
        vm
    }
}
