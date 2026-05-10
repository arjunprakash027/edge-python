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
Bump-pointer allocator. Default `LeakingPageAllocator` calls memory.grow(1) per alloc — ~0.2 ms on HVCI/VBS hosts (e.g., Snapdragon X on V8).
A ~3,000-alloc perceptron run pays ~600 ms; bumping cuts it to ~50 grows.
*/
#[global_allocator]
static A: AssumeSingleThreaded<LeakingAllocator> = unsafe { AssumeSingleThreaded::new(LeakingAllocator::new()) };

/* Best-effort panic-to-stash: the host's edge_take_error then sees a typed
   message instead of an opaque WASM trap. If the format allocation itself
   re-enters this handler we fall through to unreachable(); the host trap
   behaviour is unchanged from the previous bare implementation. */
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

/* Single struct holding every piece of mutable runtime state the WASM
   bridge owns. Lives in one `static mut` so every accessor goes through
   `with_runtime`, which is the sole `unsafe` point — instead of six
   independent statics each requiring its own ad-hoc unsafe. The
   `current_vm` pointer is set/cleared by `VmGuard` so a panic or early
   return inside `run()` cannot leave a stale pointer behind for a
   later host_edge_op to dereference. */
pub(super) struct WasmRuntime {
    pub src: [u8; SZ],
    pub out: [u8; SZ],
    pub inp: [u8; SZ],
    pub inp_len: usize,
    pub registry: Vec<(String, ModuleEntry)>,
    pub manifests: Vec<(String, Manifest)>,
    pub handles: HandleTable,
    pub error_stash: ErrorStash,
    /* Live during `run()` for re-entrant `host_edge_op` dispatch. Set
       and cleared exclusively through `VmGuard`; the lifetime cast to
       `'static` is a storage-only convention — the pointer is only
       dereferenced inside the `run()` scope that constructed it. */
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

/* Sole entry point for accessing the WASM runtime state. The closure
   form forces every borrow to be scoped: callers that need to invoke
   the VM (which can re-enter `host_edge_op` and therefore `with_runtime`
   itself) MUST copy what they need out and exit this scope first.

   SAFETY: WASM is single-threaded and there is no preemption, so the
   `&mut WasmRuntime` produced here cannot alias another live borrow as
   long as no helper called inside `f` re-enters `with_runtime`. The
   discipline is enforced by routing VM dispatch through `with_vm`,
   which copies `current_vm` out and drops the runtime borrow before
   handing control to the VM. */
pub(super) fn with_runtime<R>(f: impl FnOnce(&mut WasmRuntime) -> R) -> R {
    unsafe { f(&mut *core::ptr::addr_of_mut!(RUNTIME)) }
}

pub(super) fn put_val(v: Val) -> u32 { with_runtime(|rt| rt.handles.put(v.0)) }
pub(super) fn get_val(h: u32) -> Option<Val> { with_runtime(|rt| rt.handles.get(h).map(Val)) }

/* RAII publisher for the live VM pointer. Construction stashes the
   pointer; Drop clears it. Holding the guard for the full body of
   `run()` means a panic, early return, or `?` propagation cannot leave
   a stale pointer behind — every subsequent `host_edge_op` would
   otherwise read freed stack memory. */
pub(super) struct VmGuard;

impl VmGuard {
    pub(super) fn new(vm: &mut VM<'_>) -> Self {
        // Storage cast to 'static: the pointer is only dereferenced
        // inside the same `run()` frame that holds the guard, so the
        // VM is alive for the entire window the pointer is observable.
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
    // Copy the pointer out and exit `with_runtime` before invoking `f`,
    // because the VM dispatch invoked inside `f` will re-enter
    // `with_runtime` and aliasing the outer borrow would be UB.
    let ptr = with_runtime(|rt| rt.current_vm)?;
    Some(f(unsafe { &mut *ptr.as_ptr() }))
}

/* Construct a `&[u8]` from an FFI `(ptr, len)` pair, returning an empty
   slice when `ptr` is null or `len == 0`. Removes the unconditional
   `from_raw_parts` calls that would UB on null inputs. */
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

/* dispatch_* prologue: resolve `recv_h` and run `f` against the live VM. Fails on stale handle or call outside `run()`. */
pub(super) fn with_recv<F>(invalid_recv_msg: &'static str, recv_h: u32, f: F) -> Result<Val, VmErr>
where F: FnOnce(&mut VM<'static>, Val) -> Result<Val, VmErr>
{
    let recv = get_val(recv_h).ok_or(VmErr::Runtime(invalid_recv_msg))?;
    with_vm(|vm| f(vm, recv)).ok_or(VmErr::Runtime("edge_op called outside run()"))?
}
