/* Test infrastructure for the `packages` system.

   Provides:
     * `TestResolver` — a `Resolver` that holds a `HashMap<String, Resolved>`
       of fixture modules plus a `HashMap<dir, Manifest>` of nested
       packages.json manifests. Walk-up resolution mirrors the WASM bridge
       so the same fixture format exercises both.
     * `test_native(name)` — returns a `NativeBinding` for a fixture
       function, so JSON test cases can declare `{ "native": ["add"] }` and
       the runner wires up the function pointers.
     * Fixture functions covering pure / impure, fixed-arity, allocates on
       heap, returns handle (int), errors.

   `tests/`-only — never compiles into the production binary. */

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use compiler_lib::modules::fx::FxHashMap;
use compiler_lib::modules::packages::{
    NativeBinding, Resolved, Resolver,
    Manifest, walk_up_dirs, dir_of, join_relative,
};
use compiler_lib::modules::vm::types::{HeapObj, HeapPool, Val, VmErr};

// ─── TestResolver ────────────────────────────────────────────────────────────

/* Shared state across a TestResolver and its child sub-resolvers. Mirrors
   the WASM bridge: one process-static manifest cache + module map shared
   across all transitive resolutions, plus an in-flight set for cycle
   detection on the canonical (post-alias) spec. */
#[derive(Default)]
struct TestResolverState {
    modules: HashMap<String, Resolved>,
    /* Manifests keyed by directory ("" for the root, "lib/" for a sub-pkg,
       "https://cdn.foo/kit/" for a remote sub-pkg). The walk-up resolver
       looks for a manifest at each parent dir of the importer's location. */
    manifests: HashMap<String, Manifest>,
    in_flight: HashSet<String>,
    /* Pre-staged raw bytes per spec, consumed by `fetch_bytes`. Drives
       integrity-verification tests by feeding the parser the same buffer it
       would have hashed in production. Specs absent here surface a "not
       supported" error from the default Resolver impl. */
    bytes: HashMap<String, Vec<u8>>,
}

pub struct TestResolver {
    state: Rc<RefCell<TestResolverState>>,
    in_flight_marker: Option<String>,
    /* Directory of the module that this resolver instance was scoped for
       (set by `child(spec)`). Bare-name imports walk up from here looking
       for a manifest. Empty string for the entry-script resolver. */
    dir: String,
}

impl Drop for TestResolver {
    fn drop(&mut self) {
        if let Some(canon) = self.in_flight_marker.take() {
            self.state.borrow_mut().in_flight.remove(&canon);
        }
    }
}

impl TestResolver {
    pub fn new() -> Self {
        Self {
            state: Rc::new(RefCell::new(TestResolverState::default())),
            in_flight_marker: None,
            dir: String::new(),
        }
    }

    pub fn with_native(self, spec: &str, bindings: Vec<NativeBinding>) -> Self {
        self.state.borrow_mut().modules.insert(spec.to_string(), Resolved::Native(bindings));
        self
    }

    pub fn with_code(self, spec: &str, src: &str) -> Self {
        self.state.borrow_mut().modules.insert(spec.to_string(), Resolved::Code(src.to_string()));
        self
    }

    /* Feed the bytes the parser will hash for `spec` when verifying a
       `#sha256-...` fragment. */
    pub fn with_bytes(self, spec: &str, bytes: Vec<u8>) -> Self {
        self.state.borrow_mut().bytes.insert(spec.to_string(), bytes);
        self
    }

    /* Add an alias to the root manifest (dir = ""). Equivalent to writing
       `{"imports": { "<name>": "<target>" }}` in a sibling packages.json.
       Idempotent and additive: multiple calls accumulate into one root
       manifest, matching the original test API. */
    pub fn with_alias(self, name: &str, target: &str) -> Self {
        {
            let mut s = self.state.borrow_mut();
            let m = s.manifests.entry(String::new())
                .or_insert_with(|| Manifest {
                    imports: FxHashMap::default(),
                    extends: None,
                });
            m.imports.insert(name.to_string(), target.to_string());
        }
        self
    }

    /* Add a nested manifest at `dir` ("" for root, "lib/" for sub, etc.).
       Used by fixtures that want to exercise walk-up: bare-name resolution
       from a module under `dir/foo.py` will hit this manifest first. */
    pub fn with_manifest(self, dir: &str, imports: &[(&str, &str)], extends: Option<&str>) -> Self {
        let mut imp = FxHashMap::default();
        for (k, v) in imports { imp.insert(k.to_string(), v.to_string()); }
        let m = Manifest { imports: imp, extends: extends.map(|s| s.to_string()) };
        self.state.borrow_mut().manifests.insert(dir.to_string(), m);
        self
    }
}

impl Resolver for TestResolver {
    fn resolve(&mut self, spec: &str) -> Result<Resolved, String> {
        if !spec.contains('/') {
            let dir = self.dir.clone();
            return self.resolve_bare(spec, &dir);
        }
        let canonical = if spec.contains("://") || spec.starts_with('/') {
            spec.to_string()
        } else {
            join_relative(&self.dir, spec)
        };
        self.resolve_canonical(&canonical)
    }

    /* Sub-resolver for a transitive import: shares all state (modules,
       manifests, in_flight, bytes) and rescopes `dir` to the imported
       module's location. The Drop impl removes the in-flight marker when
       the splicer is done parsing this module. */
    fn child(&self, spec: &str) -> Box<dyn Resolver> {
        let canon = spec.to_string();
        self.state.borrow_mut().in_flight.insert(canon.clone());
        Box::new(TestResolver {
            state: Rc::clone(&self.state),
            in_flight_marker: Some(canon),
            dir: dir_of(spec).to_string(),
        })
    }

    fn fetch_bytes(&mut self, spec: &str) -> Result<Vec<u8>, String> {
        match self.state.borrow().bytes.get(spec) {
            Some(b) => Ok(b.clone()),
            None => Err(format!(
                "module '{}' integrity verification not supported by this resolver", spec)),
        }
    }
}

impl TestResolver {
    /* Walk up from `start_dir` looking for the nearest manifest that
       declares `name`. Hermetic: first manifest encountered is
       authoritative. `extends` relocates the search to another dir.
       Cycle detection on the extends chain. */
    fn resolve_bare(&mut self, name: &str, start_dir: &str) -> Result<Resolved, String> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut search_dir = start_dir.to_string();
        let mut hops = 0u32;
        loop {
            if hops > 32 {
                return Err(format!("packages.json walk-up exceeded 32 hops resolving '{}'", name));
            }
            hops += 1;
            let mut hit: Option<(String, Option<String>, Option<String>)> = None;
            for dir in walk_up_dirs(&search_dir) {
                let s = self.state.borrow();
                if let Some(m) = s.manifests.get(&dir) {
                    let target = m.imports.get(name).cloned();
                    let ext = m.extends.clone();
                    drop(s);
                    hit = Some((dir, target, ext));
                    break;
                }
            }
            let Some((dir, target, ext)) = hit else {
                return Err(format!(
                    "no packages.json above '{}' declares '{}'", start_dir, name));
            };
            if let Some(target) = target {
                let canonical = join_relative(&dir, &target);
                return self.resolve_canonical(&canonical);
            }
            if let Some(ext) = ext {
                let m_spec = format!("{}packages.json", dir);
                if !visited.insert(m_spec) {
                    return Err("circular extends chain in packages.json".to_string());
                }
                let mut next = join_relative(&dir, &ext);
                if !next.ends_with('/') { next.push('/'); }
                search_dir = next;
                continue;
            }
            return Err(format!(
                "alias '{}' not declared in '{}packages.json'\nhelp: declare it, add \"extends\": \"..\" to inherit, or use a quoted path",
                name, dir));
        }
    }

    fn resolve_canonical(&self, spec: &str) -> Result<Resolved, String> {
        let s = self.state.borrow();
        if s.in_flight.contains(spec) {
            return Err(format!("circular import: '{}'", spec));
        }
        match s.modules.get(spec) {
            // Clone so the same module can be re-imported (e.g.,
            // `from m import f; from m import f as g`). Test fixtures are
            // small; cloning is cheap.
            Some(r) => Ok(r.clone()),
            None => Err(format!("module '{}' not found in TestResolver", spec)),
        }
    }
}

// ─── Fixture functions ───────────────────────────────────────────────────────

/* Pure: a + b. Two int args, returns int. The bread-and-butter pure native:
   tests CallExtern dispatch, arg marshalling, and template memoization. */
fn add(_: &mut HeapPool, args: &[Val]) -> Result<Val, VmErr> {
    if args.len() != 2 { return Err(VmErr::Type("add: expected 2 args")); }
    let a = if args[0].is_int() { args[0].as_int() } else { return Err(VmErr::Type("add: arg 0 not int")); };
    let b = if args[1].is_int() { args[1].as_int() } else { return Err(VmErr::Type("add: arg 1 not int")); };
    Ok(Val::int(a + b))
}

/* Pure: x * x. Used to verify nested calls (square(add(1,2))). */
fn square(_: &mut HeapPool, args: &[Val]) -> Result<Val, VmErr> {
    if args.len() != 1 { return Err(VmErr::Type("square: expected 1 arg")); }
    let x = if args[0].is_int() { args[0].as_int() } else { return Err(VmErr::Type("square: arg not int")); };
    Ok(Val::int(x * x))
}

/* Pure but allocates: returns a heap string of length n. Tests heap access
   from extern context (HeapPool::alloc round-trip). */
fn make_str(heap: &mut HeapPool, args: &[Val]) -> Result<Val, VmErr> {
    if args.len() != 1 { return Err(VmErr::Type("make_str: expected 1 arg")); }
    let n = if args[0].is_int() { args[0].as_int() } else { return Err(VmErr::Type("make_str: arg not int")); };
    let s: String = "x".repeat(n.max(0) as usize);
    heap.alloc(HeapObj::Str(s))
}

/* Impure: returns a monotonically increasing int. Tests that calling an
   impure native taints the enclosing function and disables memoization. */
fn counter(_: &mut HeapPool, _args: &[Val]) -> Result<Val, VmErr> {
    use std::sync::atomic::{AtomicI64, Ordering};
    static N: AtomicI64 = AtomicI64::new(0);
    Ok(Val::int(N.fetch_add(1, Ordering::SeqCst)))
}

/* Pure: always returns 42. Useful when the test only cares that an extern
   was called and what value flowed through, without arithmetic noise. */
fn const_42(_: &mut HeapPool, _args: &[Val]) -> Result<Val, VmErr> {
    Ok(Val::int(42))
}

/* Pure: errors out with a fixed message. Tests error propagation from extern
   into the VM dispatch path and out to the runner. */
fn boom(_: &mut HeapPool, _args: &[Val]) -> Result<Val, VmErr> {
    Err(VmErr::Runtime("boom from extern"))
}

/* Pure: f64 → f64. Verifies that Val's float wire format round-trips through
   an extern call without coercion to int. */
fn double_f(_: &mut HeapPool, args: &[Val]) -> Result<Val, VmErr> {
    if args.len() != 1 || !args[0].is_float() {
        return Err(VmErr::Type("double_f: expected one float arg"));
    }
    Ok(Val::float(args[0].as_float() * 2.0))
}

/* Pure: bool → bool. Asserts that bool tags survive the extern dispatch. */
fn negate(_: &mut HeapPool, args: &[Val]) -> Result<Val, VmErr> {
    if args.len() != 1 || !args[0].is_bool() {
        return Err(VmErr::Type("negate: expected one bool arg"));
    }
    Ok(Val::bool(!args[0].as_bool()))
}

/* Pure: bool, int → int. Mixes types to confirm per-arg decode is correct. */
fn pick(_: &mut HeapPool, args: &[Val]) -> Result<Val, VmErr> {
    if args.len() != 3 || !args[0].is_bool() || !args[1].is_int() || !args[2].is_int() {
        return Err(VmErr::Type("pick: expected (bool, int, int)"));
    }
    Ok(if args[0].as_bool() { args[2] } else { args[1] })
}

/* Map a fixture name to its (function pointer, purity) pair. Test JSON
   references natives by name; the runner translates each name into a
   NativeBinding via this lookup. */
type NativeFn = fn(&mut HeapPool, &[Val]) -> Result<Val, VmErr>;

pub fn test_native(name: &str) -> Option<NativeBinding> {
    let (func, pure): (NativeFn, bool) = match name {
        "add"      => (add,      true),
        "square"   => (square,   true),
        "make_str" => (make_str, true),
        "counter"  => (counter,  false),
        "const_42" => (const_42, true),
        "boom"     => (boom,     true),
        "double_f" => (double_f, true),
        "negate"   => (negate,   true),
        "pick"     => (pick,     true),
        _ => return None,
    };
    Some(NativeBinding::from_fn(name, func, pure))
}
