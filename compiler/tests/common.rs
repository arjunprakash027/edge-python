/* Test infrastructure for the `packages` system.

   Provides:
     * `TestResolver` — a `Resolver` that holds a `HashMap<String, Resolved>`
       built up before each test. Equivalent to a stripped-down host (no fetch,
       no FS — just an in-memory module map).
     * `test_native(name)` — returns a `NativeBinding` for a fixture function,
       so JSON test cases can declare `{ "native": ["add", "square"] }` and the
       runner wires up the corresponding function pointers.
     * `wasm_example_bytes(name)` — reads (and lazily builds) a wasm32 example
       crate from `../edge-sdk/`. Tests reference fixtures by example name.
     * Fixture functions covering the axes worth testing: pure vs impure,
       fixed-arity, allocates on heap, returns handle (int), errors.

   The reference WASM loader (`load_wasm_bindings`) lives in `tests/loaders.rs`
   — `wasmtime` is a dev-only dep, so the production library never bundles a
   WASM engine. Re-exported here so test files can pull it through `common`.

   This module is `tests/`-only: it never compiles into the production binary. */

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use compiler_lib::modules::packages::{NativeBinding, Resolved, Resolver};
use compiler_lib::modules::vm::types::{HeapObj, HeapPool, Val, VmErr};

// Re-export the test loader so tests can `use crate::common::load_wasm_bindings`.
pub use crate::loaders::load_wasm_bindings;

// ─── TestResolver ────────────────────────────────────────────────────────────

/* Shared state for a `TestResolver` and any of its `child()` sub-resolvers.
   Mirrors the production CLI: one root packages.json (here, `aliases`) and
   module map shared across all transitive resolutions, plus an in-flight
   set for cycle detection. */
#[derive(Default)]
struct TestResolverState {
    modules: HashMap<String, Resolved>,
    aliases: HashMap<String, String>,
    in_flight: HashSet<String>,
    /* Pre-staged raw bytes per spec, consumed by `fetch_bytes`. Lets
       integrity-verification tests prove that the parser hashes the bytes
       it would parse, by feeding a known buffer and asserting the resulting
       diagnostic. Specs that don't appear here surface a plain "not
       supported" error from the default Resolver impl. */
    bytes: HashMap<String, Vec<u8>>,
}

pub struct TestResolver {
    state: Rc<RefCell<TestResolverState>>,
    in_flight_marker: Option<String>,
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
       `#sha256-...` fragment. Tests pair this with `with_native` /
       `with_code` so the parser's hash check sees exactly the bytes that
       would have produced the resolved module. */
    pub fn with_bytes(self, spec: &str, bytes: Vec<u8>) -> Self {
        self.state.borrow_mut().bytes.insert(spec.to_string(), bytes);
        self
    }

    /* Add a packages.json-style alias: bare-name imports map to a target spec
       declared only in the root resolver. Subordinate (child) resolvers see
       the same alias map, so a transitively-imported module can resolve a
       bare name through the entry script's packages.json without declaring
       its own. */
    pub fn with_alias(self, name: &str, target: &str) -> Self {
        self.state.borrow_mut().aliases.insert(name.to_string(), target.to_string());
        self
    }

    /* Resolve a spec to its canonical key (alias-applied) used for both the
       module map lookup and cycle detection. */
    fn canonical(&self, spec: &str) -> String {
        let s = self.state.borrow();
        s.aliases.get(spec).cloned().unwrap_or_else(|| spec.to_string())
    }
}

impl Resolver for TestResolver {
    fn resolve(&mut self, spec: &str) -> Result<Resolved, String> {
        let key = self.canonical(spec);
        if self.state.borrow().in_flight.contains(&key) {
            return Err(format!("circular import: '{}'", spec));
        }
        match self.state.borrow().modules.get(&key) {
            // Clone so the same module can be re-imported (e.g.,
            // `from m import f; from m import f as g`). Test fixtures are
            // small; cloning is cheap.
            Some(r) => Ok(r.clone()),
            None => Err(format!("module '{}' not found in TestResolver", spec)),
        }
    }

    /* Sub-resolver for transitive imports: shares the entry resolver's full
       state (modules + aliases + in_flight), so a deeper module can resolve
       a bare name declared only in the root configuration. The returned
       resolver records its spec in in_flight; Drop removes it when the
       splicer's parse step finishes. Mirrors the entry-point packages.json
       semantics described in `documentation/reference/imports.md`. */
    fn child(&self, spec: &str) -> Box<dyn Resolver> {
        let canon = self.canonical(spec);
        self.state.borrow_mut().in_flight.insert(canon.clone());
        Box::new(TestResolver {
            state: Rc::clone(&self.state),
            in_flight_marker: Some(canon),
        })
    }

    /* Surface pre-staged bytes for integrity verification, or fall through
       to the default Err if the test didn't seed any. The parser will hash
       whatever we return, so a test that wants to assert "good hash, loads
       cleanly" feeds the bytes that produce the matching SHA-256. */
    fn fetch_bytes(&mut self, spec: &str) -> Result<Vec<u8>, String> {
        let key = self.canonical(spec);
        match self.state.borrow().bytes.get(&key) {
            Some(b) => Ok(b.clone()),
            None => Err(format!(
                "module '{}' integrity verification not supported by this resolver", spec)),
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

/* Read (and lazily build) a wasm32 example from `../edge-sdk/`. The first
   call shells out to `cargo build --target wasm32-unknown-unknown --example
   <name>`; subsequent calls find the cached `.wasm` and skip the build.

   This is the bridge that makes the docs-canonical example file in
   `edge-sdk/examples/` the same artifact the tests load — single source of
   truth: if the SDK or the example breaks, the test fails. */
pub fn wasm_example_bytes(name: &str) -> Vec<u8> {
    /* Workspace target dir is shared at the repo root, so from `compiler/`
       (where `cargo test` runs) the artifact lands at `../target/...`. We
       build with `-p edge-sdk` instead of `cd`-ing into the SDK so cargo
       resolves the right member without depending on cwd. */
    let path = format!(
        "../target/wasm32-unknown-unknown/release/examples/{}.wasm",
        name,
    );
    if !std::path::Path::new(&path).exists() {
        let status = std::process::Command::new("cargo")
            .args([
                "build", "--release",
                "--target", "wasm32-unknown-unknown",
                "--example", name,
                "-p", "edge-sdk",
            ])
            .status()
            .expect("failed to spawn cargo to build wasm fixture");
        assert!(status.success(), "wasm fixture build failed for example '{}'", name);
    }
    std::fs::read(&path)
        .unwrap_or_else(|e| panic!("wasm fixture '{}' missing after build: {}", name, e))
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
