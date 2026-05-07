//! Test-only WASM loader.
//!
//! Edge Python ships as a WebAssembly module; the host runtime is
//! responsible for loading any `.wasm` modules a script imports (browser
//! shim does this via `WebAssembly.instantiateStreaming`, WASI hosts via
//! their runtime's import API). The library itself doesn't bundle a WASM
//! engine.
//!
//! For tests we still need to validate the SDK's WASM ABI end-to-end, so
//! this file mirrors the contract any production host implements: take
//! `.wasm` bytes, instantiate via `wasmtime`, and produce one
//! [`NativeBinding`] per exported function. `wasmtime` is a
//! `[dev-dependencies]` entry — never compiled into a release artifact.

use std::sync::{Arc, Mutex};

use compiler_lib::modules::packages::NativeBinding;
use compiler_lib::modules::vm::types::{HeapPool, Val, VmErr};

/// Load every i64-typed exported function from `bytes` as a [`NativeBinding`].
///
/// The Edge Python wire format is a single `u64` per Val (NaN-boxed). We pass
/// each `Val::raw()` straight through as a `wasmtime::Val::I64`, and decode
/// the result the same way — so floats, bools, and ints round-trip without
/// special-casing. The SDK's `edge_export!` macro performs the type-specific
/// decode/encode on the WASM side.
///
/// Each binding's `func` closure captures a shared `Arc<Mutex<...>>` handle
/// to the wasmtime `Store + Instance`, so calls from the EdgePython VM
/// dispatch into the same module instance and module-internal heap state
/// persists across invocations.
///
/// All exports default to `pure: false` — we can't introspect WASM purity.
/// Hosts that know a module is pure can override this after loading.
pub fn load_wasm_bindings(bytes: &[u8]) -> Result<Vec<NativeBinding>, String> {
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, bytes)
        .map_err(|e| format!("wasm parse: {}", e))?;
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = wasmtime::Instance::new(&mut store, &module, &[])
        .map_err(|e| format!("wasm instantiate: {}", e))?;

    /* Capture each export's arity so the closure can build the right-shaped
       arg slice without re-querying wasmtime per call. Skip non-function
       exports (memory, globals — they don't dispatch as natives). */
    let exports: Vec<(String, usize)> = module.exports()
        .filter_map(|e| match e.ty() {
            wasmtime::ExternType::Func(ft) => Some((e.name().to_string(), ft.params().len())),
            _ => None,
        })
        .collect();

    let state = Arc::new(Mutex::new((store, instance)));
    let mut bindings = Vec::with_capacity(exports.len());
    for (name, arity) in exports {
        let state = Arc::clone(&state);
        let n = name.clone();
        let closure = move |_heap: &mut HeapPool, args: &[Val]| -> Result<Val, VmErr> {
            if args.len() != arity {
                return Err(VmErr::Type("wasm export argument count mismatch"));
            }
            let mut guard = state.lock()
                .map_err(|_| VmErr::Runtime("wasm state lock poisoned"))?;
            let (store, instance) = &mut *guard;
            let func = instance.get_func(&mut *store, &n)
                .ok_or(VmErr::Runtime("wasm export disappeared"))?;
            let wasm_args: Vec<wasmtime::Val> = args.iter()
                .map(|v| wasmtime::Val::I64(v.raw() as i64))
                .collect();
            let mut results = vec![wasmtime::Val::I64(0)];
            func.call(&mut *store, &wasm_args, &mut results)
                .map_err(|_| VmErr::Runtime("wasm call failed"))?;
            match results[0] {
                wasmtime::Val::I64(i) => Ok(Val::from_raw(i as u64)),
                _ => Err(VmErr::Runtime("wasm returned non-i64")),
            }
        };
        bindings.push(NativeBinding {
            name,
            func: Arc::new(closure),
            pure: false,
        });
    }
    Ok(bindings)
}
