use alloc::{string::String, vec::Vec, vec, rc::Rc};
use core::cell::RefCell;
use crate::modules::fx::{FxHashMap as HashMap, FxHashSet as HashSet};

/* Per-execution resource caps: max recursion depth, op budget, heap quota. */
pub struct Limits { pub calls: usize, pub ops: usize, pub heap: usize }

impl Limits {
    pub fn none() -> Self { Self { calls: 1_000, ops: usize::MAX, heap: 10_000_000 } }
    pub fn sandbox() -> Self { Self { calls: 256, ops: 100_000_000, heap: 100_000 } }
}

/* Native function callable from EdgePython via `from <pkg> import <name>`.
   Resolved at compile time by the host's Resolver, stored in SSAChunk's
   extern_table, and dispatched by `CallExtern`.

   `func` is `Arc<dyn Fn>` rather than a plain `fn` pointer so loaders that
   wrap external binaries (.wasm via wasmtime, .so via libloading) can capture
   a stateful instance handle in the closure — a `fn` pointer alone can't
   carry context. Pure-Rust hosts that just want to register existing `fn`
   pointers use `from_fn`, which adds a single Arc allocation at registration
   time and zero runtime overhead per call.

   `pure = true` lets the VM memoize the result and skip impurity propagation
   that would taint enclosing functions. */
pub type ExternCallable =
    alloc::sync::Arc<dyn Fn(&mut HeapPool, &[Val]) -> Result<Val, VmErr> + Send + Sync>;

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
    /* Build an ExternFn from a plain `fn` pointer — common case for hand-written
       Rust natives that don't need to capture state. */
    pub fn from_fn(
        name: impl Into<String>,
        func: fn(&mut HeapPool, &[Val]) -> Result<Val, VmErr>,
        pure: bool,
    ) -> Self {
        Self { name: name.into(), func: alloc::sync::Arc::new(func), pure }
    }
}

/* NaN-boxed 8-byte value: int (47-bit), float, bool, None, undef, or heap idx.
   Tags live in the QNAN bit pattern; payload bits decide the variant. */
const QNAN: u64 = 0x7FFC_0000_0000_0000;
const SIGN: u64 = 0x8000_0000_0000_0000;
const TAG_UNDEF: u64 = QNAN;        // payload all zero — distinct from None/True/False/Heap
const TAG_NONE: u64 = QNAN | 1;
const TAG_TRUE: u64 = QNAN | 2;
const TAG_FALSE: u64 = QNAN | 3;
const TAG_INT: u64 = QNAN | SIGN;
const TAG_HEAP: u64 = QNAN | 4;

#[derive(Clone, Copy, Debug)]
pub struct Val(pub(crate) u64);

impl PartialEq for Val {
    #[inline] fn eq(&self, o: &Self) -> bool { self.0 == o.0 }
}
impl Eq for Val {}

impl core::hash::Hash for Val {
    #[inline]
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl Val {
    /* Canonical NaN stored outside the tag space so is_float() stays true. */
    const CANON_NAN: u64 = 0x7FF8_0000_0000_0000;
    #[inline(always)] pub fn float(f: f64) -> Self {
        let bits = f.to_bits();
        if (bits & QNAN) == QNAN { Self(Self::CANON_NAN) } else { Self(bits) }
    }
    #[inline(always)]
    pub fn is_numeric(&self) -> bool {
        self.is_int() || self.is_float()
    }
    pub const INT_MAX: i64 =  0x0000_7FFF_FFFF_FFFF;
    pub const INT_MIN: i64 = -0x0000_8000_0000_0000;
    #[inline(always)] pub fn int(i: i64) -> Self {
        Self(TAG_INT | (i as u64 & 0x0000_FFFF_FFFF_FFFF))
    }
    #[inline(always)] pub fn int_checked(i: i64) -> Option<Self> {
        if !(Self::INT_MIN..=Self::INT_MAX).contains(&i) { None } else { Some(Self::int(i)) }
    }
    #[inline(always)] pub fn none() -> Self { Self(TAG_NONE) }
    #[inline(always)] pub fn bool(b: bool) -> Self { Self(if b { TAG_TRUE } else { TAG_FALSE }) }
    #[inline(always)] pub fn heap(idx: u32) -> Self { Self(TAG_HEAP | ((idx as u64) << 4)) }
    /* Unbound-local sentinel, distinct from none(). Lets slot storage be
       Vec<Val> instead of Vec<Option<Val>>; LoadName raises NameError
       via a single u64 compare. */
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

    #[inline(always)] pub fn as_float(&self) -> f64  { f64::from_bits(self.0) }
    /* Public accessors for wire-format marshalling (FFI / WASM loader / SDK).
       The 64-bit payload is the canonical NaN-boxed representation; treating
       it as an opaque transport word lets non-host code round-trip values
       without depending on the private field. */
    #[inline(always)] pub fn raw(&self) -> u64 { self.0 }
    #[inline(always)] pub fn from_raw(u: u64) -> Self { Self(u) }
    #[inline(always)] pub fn as_int(&self) -> i64  {
        let raw = (self.0 & 0x0000_FFFF_FFFF_FFFF) as i64;
        (raw << 16) >> 16
    }
    #[inline(always)] pub fn as_bool(&self) -> bool { self.0 == TAG_TRUE }
    #[inline(always)] pub fn as_heap(&self) -> u32 { ((self.0 >> 4) & 0x0FFF_FFFF) as u32 }
}


/* Heap-allocated value variants. Stored in HeapPool's arena; addressed
   by index via the Val::heap tag. */
#[derive(Clone, Debug)]
pub enum HeapObj {
    Str(String),
    Bytes(Vec<u8>),
    List(Rc<RefCell<Vec<Val>>>),
    Dict(Rc<RefCell<DictMap>>),
    Set(Rc<RefCell<HashSet<Val>>>),
    Tuple(Vec<Val>),
    Func(usize, Vec<Val>, Vec<(usize, Val)>),
    Range(i64, i64, i64),
    Slice(Val, Val, Val),
    Type(String),
    BoundMethod(Val, BuiltinMethodId),
    NativeFn(NativeFnId),
    Class(String, Vec<(String, Val)>),
    Instance(Val, Rc<RefCell<DictMap>>),
    BoundUserMethod(Val, Val),
    Coroutine(usize, Vec<Val>, Vec<Val>, usize, Vec<IterFrame>),
    /* `import m` materialises this. Attribute access (`m.x`) goes through
       LoadAttr; calls (`m.x(...)`) fuse via CallMethod. The attrs vector
       carries one entry per exported name, in declaration order. */
    Module(String, Vec<(String, Val)>),
    /* A native binding lifted to a first-class callable. Created by
       LoadExtern when a Module needs to expose an extern as an attr; also
       returned directly when LoadAttr resolves to an extern. The dispatch
       path mirrors CallExtern but reads the function pointer from the heap
       object instead of the chunk's extern_table. */
    Extern(ExternFn),
}

pub use crate::modules::vm::handlers::methods::BuiltinMethodId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NativeFnId {
    Print, Len, Abs, Str, Int, Float, Bool, Type, Chr, Ord,
    Range, Round, Min, Max, Sum, Sorted, Enumerate, Zip,
    List, Tuple, Dict, Set, IsInstance, Input, All, Any,
    Bin, Oct, Hex, Divmod, Pow, Repr, Reversed, Callable, Id,
    Hash, Format, Ascii, GetAttr, HasAttr, Next, Run, Sleep,
    Receive, Map, Filter, Iter, Bytes, ImportModule,
}

impl NativeFnId {
    /* Static name table indexed by `self as usize`. The order MUST match
       the enum declaration above; #[repr(u8)] keeps discriminants stable. */
    pub fn name(self) -> &'static str {
        const NAMES: &[&str] = &[
            "print", "len", "abs", "str", "int", "float", "bool", "type", "chr", "ord",
            "range", "round", "min", "max", "sum", "sorted", "enumerate", "zip",
            "list", "tuple", "dict", "set", "isinstance", "input", "all", "any",
            "bin", "oct", "hex", "divmod", "pow", "repr", "reversed", "callable", "id",
            "hash", "format", "ascii", "getattr", "hasattr", "next", "run", "sleep",
            "receive", "map", "filter", "iter", "bytes", "import_module",
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
        let val  = self.entries[idx].1;

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

/* Visit every `Val` field reachable from `obj` exactly once. Single source
   of truth for the GC's traversal schema — adding a new HeapObj variant only
   requires adding an arm here, not editing 6+ scattered match cascades. */
pub(crate) fn for_each_val(obj: &HeapObj, mut f: impl FnMut(Val)) {
    match obj {
        HeapObj::Tuple(items)         => for &v in items { f(v); },
        HeapObj::Slice(a, b, c)       => { f(*a); f(*b); f(*c); }
        HeapObj::List(rc)             => for &v in rc.borrow().iter() { f(v); },
        HeapObj::Dict(rc)             => for (k, v) in rc.borrow().iter() { f(k); f(v); },
        HeapObj::Set(rc)              => for &v in rc.borrow().iter() { f(v); },
        HeapObj::BoundMethod(recv, _) => f(*recv),
        HeapObj::Class(_, methods)    => for (_, v) in methods { f(*v); },
        HeapObj::BoundUserMethod(r, fu) => { f(*r); f(*fu); }
        HeapObj::Instance(cls, attrs) => {
            f(*cls);
            for (k, v) in attrs.borrow().iter() { f(k); f(v); }
        }
        HeapObj::Coroutine(_, slots, stack, _, iters) => {
            for &v in slots { f(v); }
            for &v in stack { f(v); }
            for fr in iters { match fr {
                IterFrame::Seq { items, .. } => for &v in items { f(v); },
                IterFrame::Coroutine(v) => f(*v),
                IterFrame::Range { .. } => {}
            }}
        }
        HeapObj::Func(_, defaults, captures) => {
            for &v in defaults { f(v); }
            for &(_, v) in captures { f(v); }
        }
        HeapObj::Module(_, attrs) => for (_, v) in attrs { f(*v); },
        // Variants without Val payloads (Str, Bytes, Type, NativeFn,
        // Range, Extern) — terminal, nothing to trace.
        HeapObj::Str(_) | HeapObj::Bytes(_)
        | HeapObj::Type(_) | HeapObj::NativeFn(_) | HeapObj::Range(..) | HeapObj::Extern(_) => {}
    }
}

/* Arena allocator with mark-sweep GC and string interning (≤128 bytes). */
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
    /* Interns short bytes literals so that two `b"key"` allocations
       collapse to the same Val. Required because Val's Hash uses raw
       bits — without interning, a dict's `d[b"key"]` lookup hashes a
       different slot than the one that was inserted. Mirrors `strings`. */
    bytes_intern: HashMap<Vec<u8>, u32>,
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
            _ => {}
        }

        self.live += 1;
        self.alloc_count += 1;
        Ok(Val::heap(idx))
    }

    pub fn mark(&mut self, v: Val) {
        if !v.is_heap() { return; }
        let mut worklist = vec![v.as_heap()];
        while let Some(idx) = worklist.pop() {
            let idx = idx as usize;
            if self.slots[idx].marked { continue; }
            self.slots[idx].marked = true;
            if let Some(obj) = &self.slots[idx].obj {
                for_each_val(obj, |val| {
                    if val.is_heap() { worklist.push(val.as_heap()); }
                });
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
                Some(_) => {
                    slot.obj = None;
                    self.free_list.push(idx as u32);
                    self.live -= 1;
                }
            }
        }

        self.gc_threshold = (self.live * 2).max(512);
        self.alloc_count  = 0;

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
    

    /* Stable per-type tag used by the inline cache to specialise binops.
       Returns 0 for unknown / freed values. */
    #[inline(always)]
    pub fn val_tag(&self, v: Val) -> u8 {
        if v.is_int() { 1 } else if v.is_float() { 2 } else if v.is_bool() { 3 }
        else if v.is_none() { 4 } else if v.is_heap() {
            match self.slots[v.as_heap() as usize].obj.as_ref() {
                Some(HeapObj::Str(_)) => 5,
                Some(HeapObj::List(_)) => 6,
                Some(HeapObj::Dict(_)) => 7,
                Some(HeapObj::Set(_)) => 8,
                Some(HeapObj::Tuple(_)) => 9,
                Some(HeapObj::Func(_, _, _)) => 10,
                Some(HeapObj::Range(..)) => 11,
                Some(HeapObj::Slice(..)) => 12,
                Some(HeapObj::Type(_)) => 13,
                Some(HeapObj::BoundMethod(_, _)) => 15,
                Some(HeapObj::NativeFn(_)) => 16,
                Some(HeapObj::BoundUserMethod(..)) => 17,
                Some(HeapObj::Class(..)) => 18,
                Some(HeapObj::Instance(..)) => 18,
                Some(HeapObj::Coroutine(..)) => 19,
                Some(HeapObj::Module(..)) => 20,
                Some(HeapObj::Extern(_)) => 21,
                Some(HeapObj::Bytes(_)) => 22,
                None => 0,
            }
        } else { 0 }
    }
}

// Equality on Val/heap objects.

pub(super) fn eq_seq(a: &[Val], b: &[Val], eq: impl Fn(Val,Val)->bool) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x,y)| eq(*x,*y))
}
pub(super) fn eq_dict(a: &DictMap, b: &DictMap, eq: impl Fn(Val,Val)->bool) -> bool {
    a.len() == b.len() && a.iter().all(|(k,v)| b.get(&k).is_some_and(|&v2| eq(v,v2)))
}

pub fn eq_vals_with_heap(a: Val, b: Val, heap: &HeapPool) -> bool {
    if !a.is_heap() || !b.is_heap() {
        if a.is_int() && b.is_int() { return a.as_int() == b.as_int(); }
        if a.is_float() && b.is_float() { return a.as_float() == b.as_float(); }
        if a.is_int() && b.is_float() { return (a.as_int() as f64) == b.as_float(); }
        if a.is_float() && b.is_int() { return a.as_float() == (b.as_int() as f64); }
        return a.0 == b.0;
    }

    match (heap.get(a), heap.get(b)) {
        (HeapObj::Str(x), HeapObj::Str(y)) => x == y,
        (HeapObj::Bytes(x), HeapObj::Bytes(y)) => x == y,
        (HeapObj::Tuple(x), HeapObj::Tuple(y)) => eq_seq(x, y, |a,b| eq_vals_with_heap(a, b, heap)),
        (HeapObj::List(x), HeapObj::List(y)) => eq_seq(&x.borrow(), &y.borrow(), |a,b| eq_vals_with_heap(a, b, heap)),
        (HeapObj::Set(x), HeapObj::Set(y)) => *x.borrow() == *y.borrow(),
        (HeapObj::Dict(x), HeapObj::Dict(y)) => eq_dict(&x.borrow(), &y.borrow(), |a,b| eq_vals_with_heap(a, b, heap)),
        // Cross-type comparisons fall through to false. Notably `bytes == str`
        // is False in Python, even when the bytes are valid UTF-8 of the str.
        _ => false,
    }
}

/* Runtime errors. Static-string variants avoid alloc on the hot error path;
   *Msg / Name / Attribute / Raised variants carry dynamic text so the user
   sees the actual offending name or object type instead of a generic
   "attribute not found". */
pub enum VmErr {
    CallDepth, Heap, Budget, ZeroDiv, Overflow,
    Name(String),
    Type(&'static str),
    TypeMsg(String),
    Value(&'static str),
    Runtime(&'static str),
    Attribute(String),
    Raised(String),
}

impl VmErr {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CallDepth => "RecursionError: max depth",
            Self::Heap => "MemoryError: heap limit",
            Self::Budget => "RuntimeError: budget exceeded",
            Self::ZeroDiv => "ZeroDivisionError: division by zero",
            Self::Overflow => "OverflowError: integer too large for 47-bit Val",
            Self::Type(s) => s,
            Self::Value(s) => s,
            Self::Runtime(s) => s,
            Self::TypeMsg(_) => "TypeError",
            Self::Attribute(_) => "AttributeError",
            Self::Name(_) => "NameError",
            Self::Raised(_) => "Exception",
        }
    }

    pub fn render(&self) -> alloc::string::String {
        use crate::s;
        match self {
            Self::Name(n) => s!("NameError: name '", str n, "' is not defined"),
            Self::Raised(m) => s!("Exception: ", str m),
            Self::Type(m) => s!("TypeError: ", str m),
            Self::TypeMsg(m) => s!("TypeError: ", str m),
            Self::Value(m) => s!("ValueError: ", str m),
            Self::Runtime(m) => s!("RuntimeError: ", str m),
            Self::Attribute(m) => s!("AttributeError: ", str m),
            other => alloc::string::String::from(other.as_str()),
        }
    }

    /* Same message as render(), but anchored at a source byte offset so the
       parser's Diagnostic renderer adds the rustc-style line/caret preview.
       Falls back to plain render() when no position is known (pre-parse env
       errors or VM-setup faults before the first instruction). */
    pub fn render_at(&self, src: &str, byte_pos: Option<usize>, path: Option<&str>) -> alloc::string::String {
        let Some(pos) = byte_pos else { return self.render(); };
        crate::modules::parser::Diagnostic { start: pos, end: pos, msg: self.render() }
            .render(src, path)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl core::fmt::Display for VmErr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.render())
    }
}

/* Iterator state for ForIter. Consumed one item at a time. */
#[derive(Clone, Debug)]
pub enum IterFrame {
    Seq { items: Vec<Val>, idx: usize },
    Range { cur: i64, end: i64, step: i64 },
    Coroutine(Val),
}

impl IterFrame {
    pub fn next_item(&mut self) -> Option<Val> {
        match self {
            Self::Coroutine(_) => None,
            Self::Seq { items, idx } => {
                if *idx < items.len() { let v = items[*idx]; *idx += 1; Some(v) } else { None }
            }
            Self::Range { cur, end, step } => {
                let done = if *step > 0 { *cur >= *end } else { *cur <= *end };
                if done { None } else { let v = *cur; *cur += *step; Some(Val::int(v)) }
            }
        }
    }
}

// Pure-Rust f64 math (no libm, works under no_std / WASM).

#[inline]
pub fn fpowi(mut base: f64, exp: i32) -> f64 {
    if exp == 0 { return 1.0; }
    let neg = exp < 0;
    let mut e = (exp as i64).unsigned_abs() as u32;
    let mut r = 1.0;
    while e > 0 { if e & 1 != 0 { r *= base; } base *= base; e >>= 1; }
    if neg { 1.0 / r } else { r }
}

#[inline]
pub fn fround(x: f64) -> f64 {
    let i = x as i64;
    let t = i as f64;
    let d = x - t;
    if d > 0.5 { t + 1.0 }
    else if d < -0.5 { t - 1.0 }
    else if d == 0.5 { if i % 2 == 0 { t } else { t + 1.0 } }
    else if d == -0.5 { if i % 2 == 0 { t } else { t - 1.0 } }
    else { t }
}

pub fn fln(x: f64) -> f64 {
    let bits = f64::to_bits(x);
    let exp = ((bits >> 52) & 0x7FF) as i64 - 1023;
    let m = f64::from_bits((bits & 0x000F_FFFF_FFFF_FFFF) | 0x3FF0_0000_0000_0000);
    let t = (m - 1.0) / (m + 1.0); let t2 = t * t;
    2.0 * t * (1.0 + t2 * (1.0/3.0 + t2 * (1.0/5.0 + t2 * (1.0/7.0 + t2 / 9.0)))) + exp as f64 * core::f64::consts::LN_2
}

pub fn fexp(x: f64) -> f64 {
    if x > 709.0 { return f64::INFINITY; }
    if x < -709.0 { return 0.0; }
    let k = (x * core::f64::consts::LOG2_E) as i64;
    let r = x - k as f64 * core::f64::consts::LN_2;
    let e = 1.0 + r * (1.0 + r * (0.5 + r * (1.0/6.0 + r * (1.0/24.0 + r * (1.0/120.0 + r / 720.0)))));
    f64::from_bits(((k + 1023) as u64) << 52) * e
}

#[inline]
pub fn fpowf(base: f64, exp: f64) -> f64 {
    let ei = exp as i32;
    if (ei as f64) == exp { return fpowi(base, ei); }
    if base <= 0.0 {
        if base == 0.0 { return if exp > 0.0 { 0.0 } else { f64::INFINITY }; }
        return f64::NAN;
    }
    fexp(exp * fln(base))
}

/* Out-of-line error constructors keep the hot dispatch loop linear in
   the icache; #[cold] + #[inline(never)] push them off the fast path. */
#[cold] #[inline(never)] pub fn cold_heap() -> VmErr { VmErr::Heap }
#[cold] #[inline(never)] pub fn cold_budget() -> VmErr { VmErr::Budget }
#[cold] #[inline(never)] pub fn cold_depth() -> VmErr { VmErr::CallDepth }
#[cold] #[inline(never)] pub fn cold_type(m: &'static str) -> VmErr { VmErr::Type(m) }
#[cold] #[inline(never)] pub fn cold_value(m: &'static str) -> VmErr { VmErr::Value(m) }
#[cold] #[inline(never)] pub fn cold_runtime(m: &'static str) -> VmErr { VmErr::Runtime(m) }
#[cold] #[inline(never)] pub fn cold_overflow() -> VmErr { VmErr::Overflow }

/* Single-write SSA store after register coalescing. */
#[inline(always)]
pub fn p_store_ssa(slots: &mut [Val], slot: usize, v: Val) {
    slots[slot] = v;
}

#[inline]
pub fn ffloor(x: f64) -> f64 {
    let i = x as i64 as f64;
    if x < i { i - 1.0 } else { i }
}

#[inline]
pub fn fabs(x: f64) -> f64 {
    f64::from_bits(f64::to_bits(x) & 0x7FFF_FFFF_FFFF_FFFF)
}

#[inline]
pub fn ftrunc(x: f64) -> f64 {
    if x >= 0.0 { ffloor(x) } else { -ffloor(-x) }
}

#[inline]
pub fn fsignum(x: f64) -> f64 {
    if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 }
}

