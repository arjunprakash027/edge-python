/*
Edge Python `json` package. Exports `loads(text) -> value` and `dumps(value) -> text` over the `wasm-pdk` ABI.
*/

#![cfg_attr(target_arch = "wasm32", no_std)]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(special_module_name)]

extern crate alloc;

use alloc::string::String;
use wasm_pdk::*;

/* Free-list allocator over a static `.bss` pool — `memory.grow` is never called, side-stepping the runtime bug where `env.js` host_call_native captures `new DataView(memory.buffer)` before the first `__edge_alloc` and detaches it if memory grows mid-call. `dealloc` reclaims chunks so iterative workloads (JSONL streaming, loops over `loads`/`dumps`) reuse memory instead of leaking. Single-threaded: wasm32-unknown-unknown has no threads, so `UnsafeCell` access is safe without a lock. */
#[cfg(target_arch = "wasm32")]
mod allocator {
    use core::alloc::{GlobalAlloc, Layout};
    use core::cell::UnsafeCell;
    use core::ptr::NonNull;
    use core::sync::atomic::{AtomicBool, Ordering};
    use linked_list_allocator::Heap;

    const POOL_SIZE: usize = 4 * 1024 * 1024;

    #[repr(align(16))]
    struct Pool(UnsafeCell<[u8; POOL_SIZE]>);
    unsafe impl Sync for Pool {}

    static POOL: Pool = Pool(UnsafeCell::new([0; POOL_SIZE]));

    struct HeapCell(UnsafeCell<Heap>);
    unsafe impl Sync for HeapCell {}

    static HEAP: HeapCell = HeapCell(UnsafeCell::new(Heap::empty()));
    static INIT: AtomicBool = AtomicBool::new(false);

    pub struct FreeListAlloc;

    unsafe impl GlobalAlloc for FreeListAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if !INIT.load(Ordering::Relaxed) {
                unsafe { (*HEAP.0.get()).init(POOL.0.get() as *mut u8, POOL_SIZE); }
                INIT.store(true, Ordering::Relaxed);
            }
            unsafe {
                (*HEAP.0.get())
                    .allocate_first_fit(layout)
                    .map_or(core::ptr::null_mut(), |p| p.as_ptr())
            }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { (*HEAP.0.get()).deallocate(NonNull::new_unchecked(ptr), layout); }
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static A: allocator::FreeListAlloc = allocator::FreeListAlloc;

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

pub mod main;

#[plugin_fn]
fn loads(text: String) -> Result<Handle> {
    main::parser::parse(&text)
}

#[plugin_fn]
fn dumps(value: Handle) -> Result<String> {
    main::serializer::serialize(&value)
}
