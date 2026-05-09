/* WASM bridge — orchestration layer.
 *
 * Wires the Edge Python parser/VM to the JS shim and to the universal handle
 * ABI. The sealed-contract spec (host imports, guest export shape, op codes,
 * tags, error kinds, primitive codec, handle table layout) lives in
 * `crate::abi`. Look there to extend the contract — DO NOT add wire-level
 * constants or behavior here. */

use lol_alloc::{AssumeSingleThreaded, LeakingAllocator};
use crate::abi::{ErrorStash, HandleTable};
use crate::modules::vm::VM;
use crate::modules::vm::types::{Val, VmErr};
use crate::modules::packages::Manifest;
use alloc::{string::String, vec::Vec};

mod abi_bridge;
mod errors;
mod exports;
mod resolver;

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    pub(super) fn js_print(ptr: *const u8, len: usize);

    /* Invoked when the Edge Python VM dispatches a `CallExtern` for a
       function registered via `register_native_module`. The host fills
       `argv` with `argc` handles (already registered in the host's handle
       table; valid for the call's duration) and the guest writes the
       return handle into `out`. Returns 0/1 status. */
    pub(super) fn js_call_native(
        id: u32,
        argv_ptr: *const u32, argc: u32,
        out: *mut u32,
    ) -> i32;

    /* Returns the host-cached bytes for `spec`. When `hash_ptr` is
       non-null, it points to 32 bytes — the expected sha-256 of the
       returned content. */
    pub(super) fn js_fetch_bytes(
        spec_ptr: *const u8, spec_len: u32,
        hash_ptr: *const u8,
        out_len: *mut u32,
    ) -> *mut u8;
}

pub(super) fn stream_print(s: &str) {
    unsafe { js_print(s.as_ptr(), s.len()); }
}

/* Bump-pointer allocator: places multiple allocations per WebAssembly
   page instead of one page per alloc. The default `LeakingPageAllocator`
   requests `memory.grow(1)` for every Vec/String, which on hosts that
   gate page commits through a hypervisor (Snapdragon X Elite Copilot+
   PCs run V8 with HVCI/VBS active) costs ~0.2 ms per call. A perceptron
   training run produces ~3,000 small allocs ⇒ ~600 ms of grow overhead.
   Bumping inside pages cuts that to ~50 grows total. */
#[global_allocator]
static A: AssumeSingleThreaded<LeakingAllocator> =
    unsafe { AssumeSingleThreaded::new(LeakingAllocator::new()) };

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

pub(super) const SZ: usize = 1 << 20;
#[allow(non_upper_case_globals)]
pub(super) static mut SRC: [u8; SZ] = [0; SZ];
#[allow(non_upper_case_globals)]
pub(super) static mut OUT: [u8; SZ] = [0; SZ];
#[allow(non_upper_case_globals)]
pub(super) static mut INP: [u8; SZ] = [0; SZ];
pub(super) static mut INP_LEN: usize = 0;

pub(super) enum ModuleEntry {
    Code(String),
    Native(Vec<(String, u32)>),
}

static mut REGISTRY: Option<Vec<(String, ModuleEntry)>> = None;
static mut MANIFESTS: Option<Vec<(String, Manifest)>> = None;

pub(super) unsafe fn registry() -> &'static mut Vec<(String, ModuleEntry)> {
    unsafe {
        let p = core::ptr::addr_of_mut!(REGISTRY);
        if (*p).is_none() { *p = Some(Vec::new()); }
        (*p).as_mut().unwrap()
    }
}

pub(super) unsafe fn manifests() -> &'static mut Vec<(String, Manifest)> {
    unsafe {
        let p = core::ptr::addr_of_mut!(MANIFESTS);
        if (*p).is_none() { *p = Some(Vec::new()); }
        (*p).as_mut().unwrap()
    }
}

static mut HANDLES: Option<HandleTable> = None;
static mut ERROR_STASH: Option<ErrorStash> = None;

pub(super) fn handles() -> &'static mut HandleTable {
    unsafe {
        let p = core::ptr::addr_of_mut!(HANDLES);
        if (*p).is_none() { *p = Some(HandleTable::new()); }
        (*p).as_mut().unwrap()
    }
}

pub(super) fn error_stash() -> &'static mut ErrorStash {
    unsafe {
        let p = core::ptr::addr_of_mut!(ERROR_STASH);
        if (*p).is_none() { *p = Some(ErrorStash::new()); }
        (*p).as_mut().unwrap()
    }
}

pub(super) fn put_val(v: Val) -> u32 { handles().put(v.0) }
pub(super) fn get_val(h: u32) -> Option<Val> { handles().get(h).map(Val) }

/* Set during `run()` so that `host_edge_op` (called re-entrantly from
   a guest's edge_op) can dispatch methods through the VM's heap and
   method table. Cleared at end of run. */
pub(super) static mut CURRENT_VM: *mut VM<'static> = core::ptr::null_mut();

pub(super) fn with_vm<R>(f: impl FnOnce(&mut VM<'static>) -> R) -> Option<R> {
    unsafe {
        if CURRENT_VM.is_null() { None }
        else { Some(f(&mut *CURRENT_VM)) }
    }
}

pub(super) unsafe fn write_out(s: &str) -> usize {
    let b = s.as_bytes();
    let n = b.len().min(SZ);
    unsafe {
        core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(OUT) as *mut u8, n)
            .copy_from_slice(&b[..n]);
    }
    n
}

/* Shared prologue for every `dispatch_*`: resolve `recv_h` against the
   handle table and run `f` against the live VM. Two failure modes: the
   handle is stale/invalid (`invalid_recv_msg`) or the host called us
   outside `run()` ("edge_op called outside run()"). */
pub(super) fn with_recv<F>(invalid_recv_msg: &'static str, recv_h: u32, f: F) -> Result<Val, VmErr>
where F: FnOnce(&mut VM<'static>, Val) -> Result<Val, VmErr>
{
    let recv = get_val(recv_h).ok_or(VmErr::Runtime(invalid_recv_msg))?;
    with_vm(|vm| f(vm, recv))
        .ok_or(VmErr::Runtime("edge_op called outside run()"))?
}
