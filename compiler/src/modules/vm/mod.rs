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

/* Side-channel state passed between opcodes in one dispatch frame; grouped for auditability. */
pub(crate) struct Pending {
    /* Star/double-star spread bumps the next Call's argument count. */
    pub pos_delta: i32,
    pub kw_delta: i32,
    /* Current Call's byte offset; consumed by the traceback renderer. */
    pub call_byte_pos: Option<u32>,
    /* Wakeup deadline set by `sleep()` and consumed by the scheduler. */
    pub sleep_until_ns: Option<u64>,
    /* Lifted ExcInstance from `raise X(...)` so `except X as e` binds the real instance. */
    pub exc_val: Option<Val>,
}

impl Pending {
    const fn new() -> Self {
        Self {
            pos_delta: 0,
            kw_delta: 0,
            call_byte_pos: None,
            sleep_until_ns: None,
            exc_val: None,
        }
    }
}

/* `bare_name -> [(version, slot), ...]` for one chunk's `chunk.names`. */
pub(crate) type NameVersionIndex = crate::util::fx::FxHashMap<String, Vec<(i64, usize)>>;

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
    // (chunk_ptr, global fn ids); linear scan over a tiny list avoids HashMap monomorphization.
    pub(crate) fn_index: Vec<(*const SSAChunk, Vec<u32>)>,
    // function_parents: lexical enclosing fi (None at module level); body_to_fi: chunk->fi.
    pub(crate) function_parents: Vec<Option<usize>>,
    pub(crate) body_to_fi: HashMap<*const SSAChunk, usize>,
    pub(crate) body_maps: Vec<HashMap<String, usize>>,
    pub(crate) param_slots: Vec<Vec<(ParamKind, usize)>>,
    pub(crate) slot_templates: Vec<Vec<Val>>,
    pub(crate) nonlocal_tables: Vec<Vec<(usize, usize)>>,
    pub(crate) needs_caller_slots: Vec<bool>,
    /* Bitmap: slot bound to a formal parameter; protected from caller-slot propagation. */
    pub(crate) is_param_slot: Vec<Vec<bool>>,
    /* Free-variable body slots (bare_name, slot); used for caller-chunk base-name fallback. */
    pub(crate) body_free_loads: Vec<Vec<(String, usize)>>,
    pub(crate) is_async: Vec<bool>,
    pub(crate) default_slots: Vec<Vec<(usize, Val)>>,
    /* Pre-resolved `<name>_0` body slot for self-reference binding; None for lambdas. */
    pub(crate) self_ref_slot: Vec<Option<usize>>,
    pub(crate) opcode_caches: HashMap<*const SSAChunk, OpcodeCache>,
    /* Per-chunk `bare -> [(version, slot)]` index for the free-load fallback. */
    pub(crate) chunk_name_versions: HashMap<*const SSAChunk, NameVersionIndex>,
    /* Const-pool ptrs for caches currently checked out by live exec() frames. */
    pub(crate) active_const_pools: Vec<*const [Val]>,
    /* Cached `ops == usize::MAX` so the hot path skips the budget decrement. */
    pub(crate) sandbox_off: bool,
    pub(crate) with_stack: Vec<Val>,
    pub(crate) pending: Pending,
    pub(crate) yielded: bool,
    pub(crate) resume_ip: usize,
    pub output: Vec<String>,
    pub print_hook: Option<fn(&str)>,
    pub input_buffer: Vec<String>,
    pub event_queue: Vec<Val>,
    pub strict_input: bool,
    /* Byte offset of the deepest propagating error in the last run(). */
    pub(crate) error_byte_pos: Option<u32>,
    /* spec -> Module Val, populated by `init_modules`; read by LoadModule / import_module(). */
    pub(crate) module_table: HashMap<String, Val>,
    /* `fi -> module spec`; scopes the free-load fallback to the fn's own module. */
    pub(crate) fn_module: Vec<Option<String>>,
    /* Function names parallel to `functions`; consumed by traceback render. Empty = lambda. */
    pub(crate) function_names: Vec<String>,
    /* Active call frames (innermost at end); drained by the traceback renderer on error. */
    pub(crate) call_stack: Vec<CallFrame>,
    /* Cooperative scheduler for `run` / `gather` / `with_timeout`; one handle per coroutine. */
    pub(crate) scheduler: Vec<CoroutineHandle>,
    /* Host-installed wall-clock (ns). */
    pub(crate) time_hook: Option<fn() -> u64>,
    /* Fallback monotonic counter when `time_hook` is None; reset each `run()`. */
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
            pending: Pending::new(),
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
            self_ref_slot: Vec::new(),
            opcode_caches: HashMap::default(),
            chunk_name_versions: HashMap::default(),
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
                // `~` prefix marks kw-only parameters (after a lone `*`).
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
                // Require an explicit `_<digits>` suffix; bare Nonlocal-operand slots aren't canonical.
                let canon = body.names.iter().enumerate()
                    .find(|(_, n)| crate::modules::parser::SsaName::parse(n).map(|s| s.bare) == Some(base.as_str()))
                    .map(|(i, _)| body.alias_groups.get(i).and_then(|g| g.first().copied()).unwrap_or(i as u16) as usize)?;
                Some((canon, canon))
            }).collect()
        }).collect();

        // True iff the body references names not in params/builtins/captures.
        vm.needs_caller_slots = (0..vm.functions.len()).map(|fi| {
            let (params, body, _, _) = vm.functions[fi];
            let param_names: crate::util::fx::FxHashSet<&str> = params.iter().map(|p| p.trim_start_matches(['*', '~'])).collect();
            body.names.iter().any(|n| {
                let base = crate::modules::parser::ssa_strip(n);
                !param_names.contains(base) && !vm.globals.contains_key(n)
            })
        }).collect();

        // Bitmap of param-bound slots; avoids per-call BTreeSet allocation.
        vm.is_param_slot = (0..vm.functions.len()).map(|fi| {
            let (_, body, _, _) = vm.functions[fi];
            let n_slots = body.names.len();
            let mut bm = alloc::vec![false; n_slots];
            for &(_, slot) in &vm.param_slots[fi] { if slot < n_slots { bm[slot] = true; } }
            bm
        }).collect();

        // Canonical, non-param, never-written slots — built once at VM init.
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
                let canon = body.alias_groups.get(slot).and_then(|g| g.first().copied()).unwrap_or(slot as u16) as usize;
                if canon != slot { return None; }
                if param_bm.get(slot).copied().unwrap_or(false) { return None; }
                if written.contains(&slot) { return None; }
                let parsed = crate::modules::parser::SsaName::parse(name)?;
                Some((parsed.bare.to_string(), slot))
            }).collect()
        }).collect();

        // Self-reference slot, resolved once to avoid per-call `<base>_0` allocation.
        vm.self_ref_slot = (0..vm.functions.len()).map(|fi| {
            let bare = vm.function_names.get(fi)?;
            if bare.is_empty() { return None; }
            let key = s!(str bare, "_0");
            vm.body_maps[fi].get(key.as_str()).copied()
        }).collect();

        // Default-slot table: (slot, placeholder) entries the call path overwrites.
        vm.default_slots = (0..vm.functions.len()).map(|fi| {
            let (params, _, n_defaults, _) = vm.functions[fi];
            let n_defaults = *n_defaults as usize;
            if n_defaults == 0 { return Vec::new(); }
            let pslots = &vm.param_slots[fi];
            let n_params = params.len();
            let offset = n_params.saturating_sub(n_defaults);
            (0..n_defaults).filter_map(|di| { pslots.get(offset + di).map(|&(_, slot)| (slot, Val::none())) }).collect()
        }).collect();
        for &name in BUILTIN_TYPES {
            if let Ok(type_obj) = vm.heap.alloc(HeapObj::Type(name.to_string())) {
                vm.globals.insert(name.to_string(), type_obj);
                vm.globals.insert(s!(str name, "_0"), type_obj);
            }
        }
        // Entry chunk's `__name__` is "__main__"; inserted before slot_templates is built.
        if let Ok(main_name) = vm.heap.alloc(HeapObj::Str("__main__".to_string())) {
            vm.globals.insert("__name__".to_string(), main_name);
            vm.globals.insert("__name___0".to_string(), main_name);
        }
        // Builtins as first-class NativeFn values so they can be rebound/passed around.
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
        // Slot templates built after all globals are registered.
        vm.slot_templates = vm.functions.iter().map(|(_, body, _, _)| {
            vm.fill_builtins(&body.names)
        }).collect();
        vm
    }
}
