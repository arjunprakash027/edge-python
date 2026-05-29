use alloc::{rc::Rc, string::String, vec::Vec};
use core::cell::RefCell;
use crate::util::fx::{FxHashMap as HashMap, FxHashSet as HashSet};

pub mod coro;
pub mod eq;
pub mod err;
pub mod math;
pub mod scheduler;

pub use coro::*;
pub use eq::*;
pub use err::*;
pub use math::*;
pub use scheduler::*;

/* Per-execution caps: recursion depth, op budget, heap quota. */
pub struct Limits { pub calls: usize, pub ops: usize, pub heap: usize }

impl Limits {
    pub fn none() -> Self { Self { calls: 1_000, ops: usize::MAX, heap: 10_000_000 } }
    pub fn sandbox() -> Self { Self { calls: 256, ops: 100_000_000, heap: 100_000 } }
}

/* Plain fn-pointer alias for `ExternFn::from_fn`; the `Arc<dyn Fn ...>` form lives in `ExternCallable`. */
pub type ExternFnPlain = fn(&mut HeapPool, &[Val], Option<Val>) -> Result<Val, VmErr>;

/* Host-provided callable, resolved at compile time and dispatched by `CallExtern`. `Arc<dyn Fn>` lets loaders capture stateful handles; `pure` enables memoization. Third arg is the kwargs slot, `None` for plain positional calls, `Some(dict_val)` when the caller used `name=value` syntax. */
pub type ExternCallable =
    alloc::sync::Arc<dyn Fn(&mut HeapPool, &[Val], Option<Val>) -> Result<Val, VmErr> + Send + Sync>;

#[derive(Clone)]
pub struct ExternFn {
    pub name: String,
    pub func: ExternCallable,
    pub pure: bool,
}

impl core::fmt::Debug for ExternFn {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ExternFn").field("name", &self.name).field("pure", &self.pure).finish()
    }
}

impl ExternFn {
    /* Build from a plain `fn` pointer (stateless Rust natives). */
    pub fn from_fn(name: impl Into<String>, func: ExternFnPlain, pure: bool) -> Self {
        Self { name: name.into(), func: alloc::sync::Arc::new(func), pure }
    }
}

/* NaN-boxed 8-byte value (47-bit int, float, bool, None, undef, heap idx); layout in `abi::nan_box`. */
use crate::abi::nan_box::{
    QNAN, SIGN, TAG_UNDEF, TAG_NONE, TAG_TRUE, TAG_FALSE, TAG_INT, TAG_HEAP,
    INT_PAYLOAD_MASK,
};

#[derive(Clone, Copy, Debug)]
pub struct Val(pub(crate) u64);

impl PartialEq for Val {
    #[inline] fn eq(&self, o: &Self) -> bool {
        if self.0 == o.0 { return true; }
        // Mirror Hash: 1 == 1.0 must dedupe in dict/set keys.
        if self.is_int() && o.is_float() { return (self.as_int() as f64) == o.as_float(); }
        if self.is_float() && o.is_int() { return self.as_float() == (o.as_int() as f64); }
        false
    }
}
impl Eq for Val {}

impl core::hash::Hash for Val {
    #[inline]
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        // Funnel int/float through f64 bits so 1 and 1.0 share a key (47-bit ints fit losslessly).
        if self.is_int() { (self.as_int() as f64).to_bits().hash(state); }
        else if self.is_float() { self.as_float().to_bits().hash(state); }
        else { self.0.hash(state); }
    }
}

impl Val {
    /* Canonical NaN kept outside the tag space so is_float() stays true. */
    const CANON_NAN: u64 = 0x7FF8_0000_0000_0000;
    #[inline(always)] pub fn float(f: f64) -> Self {
        let bits = f.to_bits();
        if (bits & QNAN) == QNAN { Self(Self::CANON_NAN) } else { Self(bits) }
    }
    #[inline(always)]
    pub fn is_numeric(&self) -> bool {
        self.is_int() || self.is_float()
    }
    pub const INT_MAX: i64 = 0x0000_7FFF_FFFF_FFFF;
    pub const INT_MIN: i64 = -0x0000_8000_0000_0000;
    #[inline(always)] pub fn int(i: i64) -> Self {
        Self(TAG_INT | (i as u64 & INT_PAYLOAD_MASK))
    }
    #[inline(always)] pub fn int_checked(i: i64) -> Option<Self> {
        if !(Self::INT_MIN..=Self::INT_MAX).contains(&i) { None } else { Some(Self::int(i)) }
    }
    #[inline(always)] pub fn none() -> Self { Self(TAG_NONE) }
    #[inline(always)] pub fn bool(b: bool) -> Self { Self(if b { TAG_TRUE } else { TAG_FALSE }) }
    #[inline(always)] pub fn heap(idx: u32) -> Self { Self(TAG_HEAP | ((idx as u64) << 4)) }
    /* Unbound-local sentinel; lets slots stay Vec<Val> and LoadName check via one u64 compare. */
    #[inline(always)] pub fn undef() -> Self { Self(TAG_UNDEF) }

    #[inline(always)] pub fn is_float(&self) -> bool { (self.0 & QNAN) != QNAN }
    #[inline(always)] pub fn is_int(&self) -> bool { (self.0 & (QNAN | SIGN)) == TAG_INT }
    #[inline(always)] pub fn is_none(&self) -> bool { self.0 == TAG_NONE }
    #[inline(always)] pub fn is_true(&self) -> bool { self.0 == TAG_TRUE }
    #[inline(always)] pub fn is_false(&self) -> bool { self.0 == TAG_FALSE }
    #[inline(always)] pub fn is_bool(&self) -> bool { self.0 == TAG_TRUE || self.0 == TAG_FALSE }
    #[inline(always)] pub fn is_undef(&self) -> bool { self.0 == TAG_UNDEF }
    #[inline(always)] pub fn is_heap(&self) -> bool {
        (self.0 & QNAN) == QNAN && (self.0 & SIGN) == 0 && (self.0 & 0xF) >= 4
    }

    #[inline(always)] pub fn as_float(&self) -> f64 { f64::from_bits(self.0) }
    /* Wire-format accessors (FFI / WASM loader / SDK). */
    #[inline(always)] pub fn raw(&self) -> u64 { self.0 }
    /// # Safety
    ///
    /// `u` must come from `Val::raw()` on a live heap slot in the same VM.
    #[inline(always)] pub unsafe fn from_raw(u: u64) -> Self { Self(u) }
    #[inline(always)] pub fn as_int(&self) -> i64 {
        let raw = (self.0 & INT_PAYLOAD_MASK) as i64;
        (raw << 16) >> 16
    }
    #[inline(always)] pub fn as_bool(&self) -> bool { self.0 == TAG_TRUE }
    #[inline(always)] pub fn as_heap(&self) -> u32 { ((self.0 >> 4) & 0x0FFF_FFFF) as u32 }
}


/* Heap-allocated value variants in HeapPool's arena, indexed via Val::heap. */
#[derive(Clone, Debug)]
pub enum HeapObj {
    Str(String),
    Bytes(Vec<u8>),
    List(Rc<RefCell<Vec<Val>>>),
    Dict(Rc<RefCell<DictMap>>),
    Set(Rc<RefCell<HashSet<Val>>>),
    /* Immutable, hashable counterpart of Set; built via `frozenset(iter)`. */
    FrozenSet(Rc<HashSet<Val>>),
    Tuple(Vec<Val>),
    Func(usize, Vec<Val>, Vec<(usize, Val)>),
    Range(i64, i64, i64),
    Slice(Val, Val, Val),
    // True `...` singleton, distinct from any string.
    Ellipsis,
    Type(String),
    // `NotImplemented` singleton; dunder return sentinel that triggers the reflected operator fallback.
    NotImplemented,
    /* Wide-int slow path (i128); `int_to_val` canonicalises so 47-bit values stay inline. */
    LongInt(i128),
    /* Exception instance: type name + ctor args (exposed via `.args`). */
    ExcInstance(String, Vec<Val>),
    BoundMethod(Val, BuiltinMethodId),
    NativeFn(NativeFnId),
    // `bases` lists direct parents in declared order; `resolve_attr` DFS-walks them on miss.
    Class(String, Vec<Val>, Vec<(String, Val)>),
    Instance(Val, Rc<RefCell<DictMap>>),
    // `(recv, func, class)`; `class` is where `func` was found so the called frame knows what `super()` should skip past.
    BoundUserMethod(Val, Val, Val),
    // `super()` proxy: attribute access walks the bases of `cls` (skipping `cls` itself); methods bind to `recv`.
    Super(Val, Val),
    // `(getter, setter)`; `setter == none()` for getter-only properties, written via `@property` / `@x.setter`.
    Property(Val, Val),
    // Intermediate produced by `prop.setter`: callable that takes a function and returns a new `Property` with the setter attached.
    PropertySetter(Val),
    // Trailing `Vec<SyncFrame>` stacks suspended sync sub-calls (innermost-last); resume walks inside-out, each return lands on next frame's Call site. `BodyRef` discriminates user-fn coros from the implicit module-body coro. Final `Vec<ExceptionFrame>` carries try/except across yields.
    Coroutine(usize, Vec<Val>, Vec<Val>, BodyRef, Vec<IterFrame>, Vec<SyncFrame>, Vec<ExceptionFrame>),
    /* Produced by `import m`; attr access via LoadAttr, calls fuse through CallMethod. */
    Module(String, Vec<(String, Val)>),
    /* A native binding lifted to a first-class callable. */
    Extern(ExternFn),
}

pub use crate::modules::vm::handlers::methods::BuiltinMethodId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NativeFnId {
    Print, Len, Abs, Str, Int, Float, Bool, Type, Chr, Ord,
    Range, Round, Min, Max, Sum, Sorted, Enumerate, Zip,
    List, Tuple, Dict, Set, IsInstance, IsSubclass, Input, All, Any,
    Bin, Oct, Hex, Divmod, Pow, Repr, Reversed, Callable, Id,
    Hash, Format, GetAttr, HasAttr, SetAttr, DelAttr, Next, Run, Sleep,
    Receive, Map, Filter, Iter, Bytes, ImportModule, Slice, Vars,
    Gather, WithTimeout, Cancel,
    BytesFromHex, IntFromBytes, IntToBytes, FrozenSet,
    Globals, Locals,
    Super,
    Property,
    Frame,
}

impl NativeFnId {
    /* Name table indexed by `self as usize`; order MUST match the enum above. */
    pub fn name(self) -> &'static str {
        const NAMES: &[&str] = &[
            "print", "len", "abs", "str", "int", "float", "bool", "type", "chr", "ord",
            "range", "round", "min", "max", "sum", "sorted", "enumerate", "zip",
            "list", "tuple", "dict", "set", "isinstance", "issubclass", "input", "all", "any",
            "bin", "oct", "hex", "divmod", "pow", "repr", "reversed", "callable", "id",
            "hash", "format", "getattr", "hasattr", "setattr", "delattr",
            "next", "run", "sleep",
            "receive", "map", "filter", "iter", "bytes", "import_module",
            "slice", "vars",
            "gather", "with_timeout", "cancel",
            "bytes_fromhex", "int_from_bytes", "int_to_bytes", "frozenset",
            "globals", "locals",
            "super",
            "property",
            "frame",
        ];
        NAMES[self as usize]
    }
}

/* Insertion-ordered dict: Vec for ordering, HashMap as index for O(1) get. */
#[derive(Clone, Debug)]
pub struct DictMap {
    pub entries: Vec<(Val, Val)>,
    index: HashMap<Val, usize>,
}

impl DictMap {
    pub fn with_capacity(cap: usize) -> Self {
        Self { entries: Vec::with_capacity(cap), index: HashMap::with_capacity_and_hasher(cap, Default::default()) }
    }

    pub fn get(&self, key: &Val) -> Option<&Val> {
        self.index.get(key).map(|&i| &self.entries[i].1)
    }

    pub fn contains_key(&self, key: &Val) -> bool {
        self.index.contains_key(key)
    }

    pub fn insert(&mut self, key: Val, value: Val) {
        if let Some(&i) = self.index.get(&key) {
            self.entries[i].1 = value;
        } else {
            let i = self.entries.len();
            self.entries.push((key, value));
            self.index.insert(key, i);
        }
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    pub fn iter(&self) -> impl Iterator<Item = (Val, Val)> + '_ {
        self.entries.iter().map(|&(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = Val> + '_ {
        self.entries.iter().map(|&(k, _)| k)
    }

    pub fn from_pairs(pairs: Vec<(Val, Val)>) -> Self {
        let mut dm = Self::with_capacity(pairs.len());
        for (k, v) in pairs { dm.insert(k, v); }
        dm
    }
}

impl Default for DictMap {
    fn default() -> Self { Self::new() }
}

impl DictMap {
    pub fn new() -> Self { Self { entries: Vec::new(), index: HashMap::default() } }

    pub fn remove(&mut self, key: &Val) -> Option<Val> {
        let &idx = self.index.get(key)?;
        let val = self.entries[idx].1;

        self.index.remove(key);

        self.entries.remove(idx);

        for (i, (k, _)) in self.entries[idx..].iter().enumerate() {
            if let Some(entry) = self.index.get_mut(k) {
                *entry = idx + i;
            }
        }

        Some(val)
    }
}

/* Visits every reachable `Val` once; single source of truth for GC traversal. */
pub(crate) fn for_each_val(obj: &HeapObj, mut f: impl FnMut(Val)) {
    match obj {
        HeapObj::Tuple(items) => for &v in items { f(v); },
        HeapObj::Slice(a, b, c) => { f(*a); f(*b); f(*c); }
        HeapObj::List(rc) => for &v in rc.borrow().iter() { f(v); },
        HeapObj::Dict(rc) => for (k, v) in rc.borrow().iter() { f(k); f(v); },
        HeapObj::Set(rc) => for &v in rc.borrow().iter() { f(v); },
        HeapObj::FrozenSet(rc) => for &v in rc.iter() { f(v); },
        HeapObj::BoundMethod(recv, _) => f(*recv),
        HeapObj::Class(_, bases, methods) => {
            for &v in bases { f(v); }
            for (_, v) in methods { f(*v); }
        }
        HeapObj::BoundUserMethod(r, fu, cls) => { f(*r); f(*fu); f(*cls); }
        HeapObj::Super(cls, recv) => { f(*cls); f(*recv); }
        HeapObj::Property(g, s) => { f(*g); f(*s); }
        HeapObj::PropertySetter(p) => f(*p),
        HeapObj::Instance(cls, attrs) => {
            f(*cls);
            for (k, v) in attrs.borrow().iter() { f(k); f(v); }
        }
        HeapObj::Coroutine(_, slots, stack, _, iters, sub_frames, _) => {
            for &v in slots { f(v); }
            for &v in stack { f(v); }
            for fr in iters { match fr {
                IterFrame::Seq { items, .. } => for &v in items { f(v); },
                IterFrame::Coroutine(v) => f(*v),
                IterFrame::UserDefined(v) => f(*v),
                IterFrame::Range { .. } => {}
            }}
            for sf in sub_frames {
                for &v in &sf.slots { f(v); }
                for &v in &sf.stack_delta { f(v); }
                for fr in &sf.iter_delta { match fr {
                    IterFrame::Seq { items, .. } => for &v in items { f(v); },
                    IterFrame::Coroutine(v) => f(*v),
                    IterFrame::UserDefined(v) => f(*v),
                    IterFrame::Range { .. } => {}
                }}
            }
        }
        HeapObj::Func(_, defaults, captures) => {
            for &v in defaults { f(v); }
            for &(_, v) in captures { f(v); }
        }
        HeapObj::Module(_, attrs) => for (_, v) in attrs { f(*v); },
        HeapObj::ExcInstance(_, args) => for &v in args { f(v); },
        // Variants without Val payloads, terminal, nothing to trace.
        HeapObj::Str(_) | HeapObj::Bytes(_) | HeapObj::LongInt(_)
        | HeapObj::Type(_) | HeapObj::NativeFn(_) | HeapObj::Range(..)
        | HeapObj::Extern(_) | HeapObj::Ellipsis | HeapObj::NotImplemented => {}
    }
}

/* Arena allocator with mark-sweep GC and string interning (<=128 bytes). */
struct HeapSlot {
    obj: Option<HeapObj>,
    marked: bool,
}

pub struct HeapPool {
    slots: Vec<HeapSlot>,
    free_list: Vec<u32>,
    live: usize,
    pub gc_threshold: usize,
    alloc_count: usize,
    limit: usize,
    strings: HashMap<String, u32>,
    /* Interns short bytes literals so equal `b"..."` share a Val (Hash uses raw bits). */
    bytes_intern: HashMap<Vec<u8>, u32>,
    /* Interns LongInt by value so equal i128s share a Val and stay hash/eq consistent. */
    longints: HashMap<i128, u32>,
    /* Interns Type objects by name so `type(x) is set` and `type(None) is type(None)` hold. */
    types: HashMap<String, u32>,
    // Cached Ellipsis slot index so `... is ...` is True (singleton parity).
    ellipsis_idx: Option<u32>,
    // Same singleton invariant as `ellipsis_idx`, but for `NotImplemented`.
    notimpl_idx: Option<u32>,
    /* Reused across mark() calls; cleared not freed, so GC never re-allocates under pressure. */
    mark_worklist: Vec<u32>,
}

impl HeapPool {
    pub fn new(limit: usize) -> Self {
        Self {
            slots: Vec::new(),
            free_list: Vec::new(),
            live: 0,
            gc_threshold: 512,
            alloc_count: 0,
            limit,
            strings: HashMap::default(),
            bytes_intern: HashMap::default(),
            longints: HashMap::default(),
            types: HashMap::default(),
            ellipsis_idx: None,
            notimpl_idx: None,
            mark_worklist: Vec::with_capacity(64),
        }
    }

    pub fn alloc(&mut self, obj: HeapObj) -> Result<Val, VmErr> {
        if let HeapObj::Str(ref s) = obj
            && s.len() <= 128
            && let Some(&idx) = self.strings.get(s) {
                return Ok(Val::heap(idx));
        }
        if let HeapObj::Bytes(ref b) = obj
            && b.len() <= 128
            && let Some(&idx) = self.bytes_intern.get(b) {
                return Ok(Val::heap(idx));
        }
        if let HeapObj::LongInt(i) = obj
            && let Some(&idx) = self.longints.get(&i) {
                return Ok(Val::heap(idx));
        }
        if let HeapObj::Type(ref name) = obj
            && let Some(&idx) = self.types.get(name) {
                return Ok(Val::heap(idx));
        }
        // Ellipsis is a true singleton, every `...` literal returns the same Val.
        if matches!(obj, HeapObj::Ellipsis)
            && let Some(idx) = self.ellipsis_idx {
                return Ok(Val::heap(idx));
        }
        // `NotImplemented` follows the same singleton rule so `is` and dunder checks agree.
        if matches!(obj, HeapObj::NotImplemented)
            && let Some(idx) = self.notimpl_idx {
                return Ok(Val::heap(idx));
        }
        if self.live >= self.limit { return Err(cold_heap()); }
        if self.slots.len() >= (1 << 28) { return Err(VmErr::Heap); }

        let idx = if let Some(i) = self.free_list.pop() {
            self.slots[i as usize] = HeapSlot { obj: Some(obj), marked: false };
            i
        } else {
            let i = self.slots.len() as u32;
            self.slots.push(HeapSlot { obj: Some(obj), marked: false });
            i
        };

        match self.slots[idx as usize].obj.as_ref().unwrap() {
            HeapObj::Str(s) if s.len() <= 128 => { self.strings.insert(s.clone(), idx); }
            HeapObj::Bytes(b) if b.len() <= 128 => { self.bytes_intern.insert(b.clone(), idx); }
            HeapObj::LongInt(i) => { self.longints.insert(*i, idx); }
            HeapObj::Type(name) => { self.types.insert(name.clone(), idx); }
            HeapObj::Ellipsis => { self.ellipsis_idx = Some(idx); }
            HeapObj::NotImplemented => { self.notimpl_idx = Some(idx); }
            _ => {}
        }

        self.live += 1;
        self.alloc_count += 1;
        Ok(Val::heap(idx))
    }

    pub fn mark(&mut self, v: Val) {
        if !v.is_heap() { return; }
        /* Split borrow: closure needs &mut mark_worklist while we read slots. */
        let HeapPool { slots, mark_worklist, .. } = self;
        mark_worklist.push(v.as_heap());
        while let Some(idx) = mark_worklist.pop() {
            let idx = idx as usize;
            if slots[idx].marked { continue; }
            slots[idx].marked = true;
            if let Some(obj) = &slots[idx].obj {
                for_each_val(obj, |val| { if val.is_heap() { mark_worklist.push(val.as_heap()); } });
            }
        }
    }

    pub fn sweep(&mut self) {
        for idx in 0..self.slots.len() {
            let slot = &mut self.slots[idx];
            match &slot.obj {
                None => {}
                Some(_) if slot.marked => { slot.marked = false; }
                Some(HeapObj::Str(s)) => {
                    self.strings.remove(s);
                    slot.obj = None;
                    self.free_list.push(idx as u32);
                    self.live -= 1;
                }
                Some(HeapObj::Bytes(b)) => {
                    self.bytes_intern.remove(b);
                    slot.obj = None;
                    self.free_list.push(idx as u32);
                    self.live -= 1;
                }
                Some(HeapObj::LongInt(i)) => {
                    self.longints.remove(i);
                    slot.obj = None;
                    self.free_list.push(idx as u32);
                    self.live -= 1;
                }
                Some(HeapObj::Type(name)) => {
                    self.types.remove(name);
                    slot.obj = None;
                    self.free_list.push(idx as u32);
                    self.live -= 1;
                }
                Some(HeapObj::Ellipsis) => {
                    // Cached singleton index becomes stale when its slot is freed.
                    if self.ellipsis_idx == Some(idx as u32) { self.ellipsis_idx = None; }
                    slot.obj = None;
                    self.free_list.push(idx as u32);
                    self.live -= 1;
                }
                Some(HeapObj::NotImplemented) => {
                    // Singleton index becomes stale when its slot is freed.
                    if self.notimpl_idx == Some(idx as u32) { self.notimpl_idx = None; }
                    slot.obj = None;
                    self.free_list.push(idx as u32);
                    self.live -= 1;
                }
                Some(_) => {
                    slot.obj = None;
                    self.free_list.push(idx as u32);
                    self.live -= 1;
                }
            }
        }

        self.gc_threshold = (self.live * 2).max(512);
        self.alloc_count = 0;

        // Cap free list at 512K slots; sort to prefer low indices and reduce fragmentation.
        if self.free_list.len() > 524_288 {
            self.free_list.sort_unstable();
            self.free_list.truncate(524_288);
        }
    }

    pub fn needs_gc(&self) -> bool {
        let alloc_limit = (self.live / 4).max(4096);
        self.live >= self.gc_threshold || self.alloc_count >= alloc_limit
    }

    pub fn usage(&self) -> usize { self.live }

    #[inline(always)] pub fn get(&self, v: Val) -> &HeapObj {
        self.slots[v.as_heap() as usize].obj
            .as_ref()
            .expect("garbage collector invariant violated: live Val references a freed heap slot")
    }
    #[inline(always)] pub fn get_mut(&mut self, v: Val) -> &mut HeapObj {
        self.slots[v.as_heap() as usize].obj
            .as_mut()
            .expect("garbage collector invariant violated: live Val references a freed heap slot (mut)")
    }


    /* Stable per-type tag for inline-cache binop specialisation; 0 for unknown/freed. */
    #[inline(always)]
    pub fn val_tag(&self, v: Val) -> u8 {
        if v.is_int() { 1 } else if v.is_float() { 2 } else if v.is_bool() { 3 }
        else if v.is_none() { 4 } else if v.is_heap() {
            match self.slots[v.as_heap() as usize].obj.as_ref() {
                Some(HeapObj::Str(_)) => 5,
                Some(HeapObj::List(_)) => 6,
                Some(HeapObj::Dict(_)) => 7,
                Some(HeapObj::Set(_)) => 8,
                Some(HeapObj::FrozenSet(_)) => 25,
                Some(HeapObj::Tuple(_)) => 9,
                Some(HeapObj::Func(_, _, _)) => 10,
                Some(HeapObj::Range(..)) => 11,
                Some(HeapObj::Slice(..)) => 12,
                Some(HeapObj::Type(_)) => 13,
                Some(HeapObj::LongInt(_)) => 14,
                Some(HeapObj::BoundMethod(_, _)) => 15,
                Some(HeapObj::NativeFn(_)) => 16,
                Some(HeapObj::BoundUserMethod(..)) => 17,
                Some(HeapObj::Class(..)) => 18,
                Some(HeapObj::Instance(..)) => 18,
                Some(HeapObj::Coroutine(..)) => 19,
                Some(HeapObj::Module(..)) => 20,
                Some(HeapObj::Extern(_)) => 21,
                Some(HeapObj::Bytes(_)) => 22,
                Some(HeapObj::ExcInstance(..)) => 24,
                Some(HeapObj::Ellipsis) => 26,
                Some(HeapObj::NotImplemented) => 27,
                Some(HeapObj::Super(..)) => 28,
                Some(HeapObj::Property(..)) => 29,
                Some(HeapObj::PropertySetter(..)) => 30,
                None => 0,
            }
        } else { 0 }
    }

    /* Identity probe for the `NotImplemented` singleton; consumed by the dunder dispatch protocol. */
    #[inline(always)]
    pub fn is_not_implemented(&self, v: Val) -> bool {
        v.is_heap()
            && matches!(self.slots[v.as_heap() as usize].obj.as_ref(), Some(HeapObj::NotImplemented))
    }

    /* `child` is `ancestor` or has it in its transitive bases. Identity on heap idx, classes are interned per-MakeClass and never mutated, so direct equality suffices. */
    pub fn is_subclass(&self, child: Val, ancestor: Val) -> bool {
        if child.0 == ancestor.0 { return true; }
        if !child.is_heap() { return false; }
        let HeapObj::Class(_, bases, _) = self.get(child) else { return false; };
        bases.iter().any(|&b| self.is_subclass(b, ancestor))
    }
}

/* Single-write SSA store after register coalescing. */
#[inline(always)]
pub fn p_store_ssa(slots: &mut [Val], slot: usize, v: Val) {
    slots[slot] = v;
}

/* Widens int/bool/LongInt to i128 for the slow path; None on non-integer operands. */
#[inline]
pub fn as_i128(v: Val, heap: &HeapPool) -> Option<i128> {
    if v.is_int() { Some(v.as_int() as i128) }
    else if v.is_bool() { Some(v.as_bool() as i128) }
    else if v.is_heap() {
        match heap.get(v) {
            HeapObj::LongInt(i) => Some(*i),
            _ => None,
        }
    }
    else { None }
}
