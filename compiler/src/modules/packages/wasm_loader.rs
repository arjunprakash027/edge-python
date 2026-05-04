//! Reference WASM module loader. Native-only (excluded from wasm32 builds).
//!
//! Takes the bytes of a `.wasm` module that follows the Edge Python ABI
//! (i64-typed exports with the wire format from `edge-sdk`), instantiates it
//! with wasmtime, and returns one [`NativeBinding`] per exported function.
//!
//! Used by:
//!   * the `edge` CLI binary, when `from "./mod.wasm" import name` resolves
//!     to a local `.wasm` file
//!   * the test runner, to validate the SDK + ABI end-to-end
//!
//! Production hosts that need a different runtime (wasmer, wasmi) copy this
//! file as a starting point — the public surface is small (one function),
//! and the loader contract is defined entirely by [`NativeBinding`] in
//! [`super`].

use alloc::sync::Arc;
use std::sync::Mutex;

use super::NativeBinding;
use crate::modules::vm::types::{HeapPool, Val, VmErr};

/// Load every i64-typed exported function from `bytes` as a `NativeBinding`.
///
/// Each binding's `func` closure captures a shared (`Arc<Mutex<...>>`) handle
/// to the wasmtime `Store + Instance`, so calls from the EdgePython VM
/// dispatch into the same WASM module instance across invocations. Heap state
/// inside the module persists between calls.
///
/// All exports default to `pure: false` — we can't introspect WASM purity.
/// Hosts that know a module is pure can override this after loading.
pub fn load_wasm_bindings(bytes: &[u8]) -> Result<alloc::vec::Vec<NativeBinding>, alloc::string::String> {
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, bytes)
        .map_err(|e| alloc::format!("wasm parse: {}", e))?;
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = wasmtime::Instance::new(&mut store, &module, &[])
        .map_err(|e| alloc::format!("wasm instantiate: {}", e))?;

    // Snapshot export names up front so per-binding closures don't re-walk.
    let exports: alloc::vec::Vec<alloc::string::String> = module.exports()
        .filter_map(|e| match e.ty() {
            wasmtime::ExternType::Func(_) => Some(alloc::string::String::from(e.name())),
            _ => None,
        })
        .collect();

    let state = Arc::new(Mutex::new((store, instance)));
    let mut bindings = alloc::vec::Vec::with_capacity(exports.len());
    for name in exports {
        let state = Arc::clone(&state);
        let n = name.clone();
        let closure = move |_heap: &mut HeapPool, args: &[Val]| -> Result<Val, VmErr> {
            let mut guard = state.lock()
                .map_err(|_| VmErr::Runtime("wasm state lock poisoned"))?;
            let (store, instance) = &mut *guard;
            let func = instance.get_func(&mut *store, &n)
                .ok_or(VmErr::Runtime("wasm export disappeared"))?;
            let wasm_args: alloc::vec::Vec<wasmtime::Val> = args.iter()
                .map(|v| if v.is_int() {
                    wasmtime::Val::I64(v.as_int())
                } else {
                    wasmtime::Val::I64(0)
                })
                .collect();
            let mut results = alloc::vec![wasmtime::Val::I64(0)];
            func.call(&mut *store, &wasm_args, &mut results)
                .map_err(|_| VmErr::Runtime("wasm call failed"))?;
            match results[0] {
                wasmtime::Val::I64(i) => Ok(Val::int(i)),
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
