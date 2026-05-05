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
pub fn load_wasm_bindings(bytes: &[u8]) -> Result<alloc::vec::Vec<NativeBinding>, alloc::string::String> {
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, bytes)
        .map_err(|e| alloc::format!("wasm parse: {}", e))?;
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = wasmtime::Instance::new(&mut store, &module, &[])
        .map_err(|e| alloc::format!("wasm instantiate: {}", e))?;

    /* Capture each export's arity so the closure can build the right-shaped
       arg slice without re-querying wasmtime per call. Skip non-function
       exports (memory, globals — they don't dispatch as natives). */
    let exports: alloc::vec::Vec<(alloc::string::String, usize)> = module.exports()
        .filter_map(|e| match e.ty() {
            wasmtime::ExternType::Func(ft) => {
                Some((alloc::string::String::from(e.name()), ft.params().len()))
            }
            _ => None,
        })
        .collect();

    let state = Arc::new(Mutex::new((store, instance)));
    let mut bindings = alloc::vec::Vec::with_capacity(exports.len());
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
            let wasm_args: alloc::vec::Vec<wasmtime::Val> = args.iter()
                .map(|v| wasmtime::Val::I64(v.raw() as i64))
                .collect();
            let mut results = alloc::vec![wasmtime::Val::I64(0)];
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
