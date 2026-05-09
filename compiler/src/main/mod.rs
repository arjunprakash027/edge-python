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

/* Live during `run()` for re-entrant `host_edge_op` dispatch from guests. */
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

/* dispatch_* prologue: resolve `recv_h` and run `f` against the live VM. Fails on stale handle or call outside `run()`. */
pub(super) fn with_recv<F>(invalid_recv_msg: &'static str, recv_h: u32, f: F) -> Result<Val, VmErr>
where F: FnOnce(&mut VM<'static>, Val) -> Result<Val, VmErr>
{
    let recv = get_val(recv_h).ok_or(VmErr::Runtime(invalid_recv_msg))?;
    with_vm(|vm| f(vm, recv)).ok_or(VmErr::Runtime("edge_op called outside run()"))?
}
