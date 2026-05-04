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

   The actual WASM loader lives in `compiler_lib::modules::packages::
   load_wasm_bindings` (production code). Tests use the same loader the CLI
   uses — single source of truth.

   This module is `tests/`-only: it never compiles into the production binary. */

#![allow(dead_code)]

use std::collections::HashMap;

use compiler_lib::modules::packages::{NativeBinding, Resolved, Resolver};
use compiler_lib::modules::vm::types::{HeapObj, HeapPool, Val, VmErr};

// Re-export the production loader so tests can `use crate::common::load_wasm_bindings`.
pub use compiler_lib::modules::packages::load_wasm_bindings;

// ─── TestResolver ────────────────────────────────────────────────────────────

pub struct TestResolver {
    modules: HashMap<String, Resolved>,
}

impl TestResolver {
    pub fn new() -> Self { Self { modules: HashMap::new() } }

    pub fn with_native(mut self, spec: &str, bindings: Vec<NativeBinding>) -> Self {
        self.modules.insert(spec.to_string(), Resolved::Native(bindings));
        self
    }

    pub fn with_code(mut self, spec: &str, src: &str) -> Self {
        self.modules.insert(spec.to_string(), Resolved::Code(src.to_string()));
        self
    }
}

impl Resolver for TestResolver {
    fn resolve(&mut self, spec: &str) -> Result<Resolved, String> {
        match self.modules.get(spec) {
            // Clone so the same module can be re-imported (e.g.,
            // `from m import f; from m import f as g`). Test fixtures are
            // small; cloning is cheap.
            Some(r) => Ok(clone_resolved(r)),
            None => Err(format!("module '{}' not found in TestResolver", spec)),
        }
    }
}

fn clone_resolved(r: &Resolved) -> Resolved {
    match r {
        Resolved::Code(s) => Resolved::Code(s.clone()),
        Resolved::Native(bs) => Resolved::Native(bs.iter().map(|b| NativeBinding {
            name: b.name.clone(),
            func: b.func.clone(),
            pure: b.pure,
        }).collect()),
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

/* Read (and lazily build) a wasm32 example from `../edge-sdk/`. The first
   call shells out to `cargo build --target wasm32-unknown-unknown --example
   <name>`; subsequent calls find the cached `.wasm` and skip the build.

   This is the bridge that makes the docs-canonical example file in
   `edge-sdk/examples/` the same artifact the tests load — single source of
   truth: if the SDK or the example breaks, the test fails. */
pub fn wasm_example_bytes(name: &str) -> Vec<u8> {
    let path = format!(
        "../edge-sdk/target/wasm32-unknown-unknown/release/examples/{}.wasm",
        name,
    );
    if !std::path::Path::new(&path).exists() {
        let status = std::process::Command::new("cargo")
            .args([
                "build", "--release",
                "--target", "wasm32-unknown-unknown",
                "--example", name,
            ])
            .current_dir("../edge-sdk")
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
        _ => return None,
    };
    Some(NativeBinding::from_fn(name, func, pure))
}
