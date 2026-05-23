/*
Edge Python `json` package. Exports `loads(text) -> value` and `dumps(value) -> text` over the `wasm-pdk` ABI.
*/

#![cfg_attr(target_arch = "wasm32", no_std)]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(special_module_name)]

extern crate alloc;

use alloc::string::{String, ToString};
use wasm_pdk::*;

/* Free-list allocator on static `.bss` pool, avoids `memory.grow` detaching `env.js` host_call_native's pre-captured `DataView(memory.buffer)`. `dealloc` reclaims chunks for iterative `loads`/`dumps`; single-threaded wasm32 makes `UnsafeCell` safe. */
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
fn loads(text: String, kw: Kwargs) -> Result<Handle> {
    let ctx = main::parser::LoadCtx {
        object_hook: kw.get_handle("object_hook")?,
        object_pairs_hook: kw.get_handle("object_pairs_hook")?,
        parse_float: kw.get_handle("parse_float")?,
        parse_int: kw.get_handle("parse_int")?,
        parse_constant: kw.get_handle("parse_constant")?,
    };
    main::parser::parse(&text, &ctx)
}

#[plugin_fn]
fn dumps(value: Handle, kw: Kwargs) -> Result<String> {
    let mut opts = main::serializer::Options::default();
    opts.indent = kw.get::<i64>("indent")?;
    opts.sort_keys = kw.get::<bool>("sort_keys")?.unwrap_or(false);
    if let Some(b) = kw.get::<bool>("ensure_ascii")? { opts.ensure_ascii = b; }
    if let Some(b) = kw.get::<bool>("check_circular")? { opts.check_circular = b; }
    if let Some(b) = kw.get::<bool>("allow_nan")? { opts.allow_nan = b; }
    if let Some(b) = kw.get::<bool>("skipkeys")? { opts.skipkeys = b; }
    opts.cls = kw.get_handle("cls")?;
    opts.default = kw.get_handle("default")?;
    // `separators` arrives as a 2-tuple `(item, key)`; index manually since `Kwargs::get` can't decode tuples.
    if let Some(seps) = kw.get_handle("separators")? {
        let zero = encode(Value::Int(0))?;
        let one = encode(Value::Int(1))?;
        let item = seps.get_item(&zero)?;
        let key = seps.get_item(&one)?;
        opts.item_sep = decode_str(&item, "separators[0]")?;
        opts.key_sep = decode_str(&key, "separators[1]")?;
    } else if opts.indent.is_some() {
        // CPython: when `indent` is given and `separators` is not, default key separator becomes `": "`.
        opts.item_sep = ",".to_string();
        opts.key_sep = ": ".to_string();
    }
    main::serializer::serialize(&value, opts)
}

fn decode_str(h: &Handle, what: &str) -> Result<String> {
    match decode(h.raw())? {
        Value::Bytes(b) => String::from_utf8(b).map_err(|e| Error::Value(alloc::format!("{} not UTF-8: {}", what, e))),
        _ => Err(Error::Type(alloc::format!("{} must be str", what))),
    }
}
