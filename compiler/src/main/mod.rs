/*
WASM bridge — orchestration only. Wires the Edge Python parser/VM to the host through the universal handle ABI.
Wire-level contract lives in `crate::abi` — extend it there, never here.
*/

use lol_alloc::{AssumeSingleThreaded, LeakingAllocator};
use crate::abi::{ErrorStash, HandleTable};
use crate::modules::vm::VM;
use crate::modules::vm::types::{Val, VmErr};
use crate::modules::packages::Manifest;
use alloc::{string::String, vec::Vec};
use core::ptr::NonNull;

mod abi_bridge;
mod errors;
mod exports;
mod resolver;

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    pub(super) fn host_print(ptr: *const u8, len: usize);

    /* CallExtern dispatch for register_native_module. Host owns argv; guest writes return into out. */
    pub(super) fn host_call_native(id: u32, argv_ptr: *const u32, argc: u32, out: *mut u32) -> i32;

    /* Host-cached bytes for `spec`. Non-null `hash_ptr` is a 32-byte expected sha-256. */
    pub(super) fn host_fetch_bytes(spec_ptr: *const u8, spec_len: u32, hash_ptr: *const u8, out_len: *mut u32) -> *mut u8;
}

pub(super) fn stream_print(s: &str) {
    unsafe { host_print(s.as_ptr(), s.len()); }
}

/*
Bump-pointer allocator. Default `LeakingPageAllocator` calls memory.grow(1) per alloc — around 0.2 ms on HVCI/VBS hosts (e.g., Snapdragon X on V8).
A 3,000-alloc perceptron run hit 600 ms; bumping cuts it to about 50 grows.
*/
#[global_allocator]
static A: AssumeSingleThreaded<LeakingAllocator> = unsafe { AssumeSingleThreaded::new(LeakingAllocator::new()) };

/* Best-effort panic-to-stash so the host gets a typed message instead of an opaque trap. Re-entry during the format alloc falls through to unreachable() — same trap as before. */
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let msg = alloc::format!("internal panic: {}", info.message());
    with_runtime(|rt| {
        rt.error_stash.set(crate::abi::ErrorKind::Runtime as u32, msg);
    });
    core::arch::wasm32::unreachable()
}

pub(super) const SZ: usize = 1 << 20;

pub(super) enum ModuleEntry {
    Code(String),
    Native(Vec<(String, u32)>),
}

/* All mutable WASM-bridge state in one struct so every accessor funnels through `with_runtime` — the sole `unsafe` point — instead of N independent statics. */
pub(super) struct WasmRuntime {
    pub src: [u8; SZ],
    pub out: [u8; SZ],
    pub inp: [u8; SZ],
    pub inp_len: usize,
    pub registry: Vec<(String, ModuleEntry)>,
    pub manifests: Vec<(String, Manifest)>,
    pub handles: HandleTable,
    pub error_stash: ErrorStash,
    /* Set/cleared exclusively by `VmGuard`. The `'static` is storage-only — the pointer is dereferenced only inside the `run()` scope that built it. */
    pub current_vm: Option<NonNull<VM<'static>>>,
}

impl WasmRuntime {
    const fn new() -> Self {
        Self {
            src: [0; SZ],
            out: [0; SZ],
            inp: [0; SZ],
            inp_len: 0,
            registry: Vec::new(),
            manifests: Vec::new(),
            handles: HandleTable::new(),
            error_stash: ErrorStash::new(),
            current_vm: None,
        }
    }
}

static mut RUNTIME: WasmRuntime = WasmRuntime::new();

// SAFETY: single-threaded WASM; re-entrant callers route through `with_vm` to drop the borrow first.
pub(super) fn with_runtime<R>(f: impl FnOnce(&mut WasmRuntime) -> R) -> R {
    unsafe { f(&mut *core::ptr::addr_of_mut!(RUNTIME)) }
}

pub(super) fn put_val(v: Val) -> u32 { with_runtime(|rt| rt.handles.put(v.0)) }
pub(super) fn get_val(h: u32) -> Option<Val> { with_runtime(|rt| rt.handles.get(h).map(Val)) }

/* RAII publisher for the live VM pointer. Holding the guard across `run()` ensures a panic or early return cannot leave a stale pointer for later `host_edge_op` calls. */
pub(super) struct VmGuard;

impl VmGuard {
    pub(super) fn new(vm: &mut VM<'_>) -> Self {
        // 'static is storage-only — deref only inside the `run()` frame holding the guard.
        let ptr: NonNull<VM<'static>> = NonNull::from(vm).cast();
        with_runtime(|rt| rt.current_vm = Some(ptr));
        Self
    }
}

impl Drop for VmGuard {
    fn drop(&mut self) {
        with_runtime(|rt| rt.current_vm = None);
    }
}

pub(super) fn with_vm<R>(f: impl FnOnce(&mut VM<'static>) -> R) -> Option<R> {
    // Drop the runtime borrow before `f` — VM dispatch re-enters `with_runtime`.
    let ptr = with_runtime(|rt| rt.current_vm)?;
    Some(f(unsafe { &mut *ptr.as_ptr() }))
}

/* Builds a `&[u8]` from an FFI `(ptr, len)`, empty on null or zero length — `from_raw_parts` would UB on either. */
pub(super) unsafe fn safe_bytes<'a>(ptr: *const u8, len: u32) -> &'a [u8] {
    if ptr.is_null() || len == 0 { return &[]; }
    unsafe { core::slice::from_raw_parts(ptr, len as usize) }
}

/* Same for `&[u32]` argv arrays. */
pub(super) unsafe fn safe_handles<'a>(ptr: *const u32, len: u32) -> &'a [u32] {
    if ptr.is_null() || len == 0 { return &[]; }
    unsafe { core::slice::from_raw_parts(ptr, len as usize) }
}

pub(super) unsafe fn write_out(s: &str) -> usize {
    let b = s.as_bytes();
    let n = b.len().min(SZ);
    with_runtime(|rt| rt.out[..n].copy_from_slice(&b[..n]));
    n
}

/* `dispatch_*` prologue: resolve `recv_h` and run `f` against the live VM. Fails on stale handle or call outside `run()`. */
pub(super) fn with_recv<F>(invalid_recv_msg: &'static str, recv_h: u32, f: F) -> Result<Val, VmErr>
where F: FnOnce(&mut VM<'static>, Val) -> Result<Val, VmErr>
{ let recv = get_val(recv_h).ok_or(VmErr::Runtime(invalid_recv_msg))?; with_vm(|vm| f(vm, recv)).ok_or(VmErr::Runtime("edge_op called outside run()"))? }
