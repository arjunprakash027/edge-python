use crate::modules::lexer::lex;
use crate::modules::parser::{Parser, Diagnostic, SSAChunk};
use crate::modules::vm::{VM, Limits};
use crate::modules::vm::types::{HeapObj, SchedulerStatus, VmErr};
use alloc::{boxed::Box, string::{String, ToString}};
use core::ptr::NonNull;
use crate::s;

use super::{ModuleEntry, PausedRun, SZ, VmGuard, now_ns_host, safe_bytes, stream_print, with_runtime, write_out};
use super::resolver::WasmHostResolver;

/* Packed `u32` from `run_start` / `run_resume`: top 3 bits = kind, low 29 = out-buffer length. */
const STATUS_KIND_SHIFT: u32 = 29;
const STATUS_PAYLOAD_MASK: u32 = (1 << STATUS_KIND_SHIFT) - 1;
const STATUS_DONE: u32 = 0 << STATUS_KIND_SHIFT;
const STATUS_PENDING_TIMER: u32 = 1 << STATUS_KIND_SHIFT;
const STATUS_PENDING_FRAME: u32 = 2 << STATUS_KIND_SHIFT;
const STATUS_PENDING_EVENT: u32 = 3 << STATUS_KIND_SHIFT;
const STATUS_ERROR: u32 = 4 << STATUS_KIND_SHIFT;
const STATUS_PENDING_HOST_CALL: u32 = 5 << STATUS_KIND_SHIFT;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn src_ptr() -> *mut u8 {
    with_runtime(|rt| rt.src.as_mut_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn out_ptr() -> *const u8 {
    with_runtime(|rt| rt.out.as_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasm_alloc(size: u32) -> *mut u8 {
    let v = alloc::vec![0u8; size as usize];
    Box::into_raw(v.into_boxed_slice()) as *mut u8
}

/* Frees a `wasm_alloc` buffer. Host must pass the exact `size` it requested, a mismatched length rebuilds the wrong Box layout. Null or zero is a no-op. */
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasm_free(ptr: *mut u8, size: u32) {
    if ptr.is_null() || size == 0 { return; }
    unsafe {
        let slice = core::slice::from_raw_parts_mut(ptr, size as usize);
        let _ = Box::from_raw(slice as *mut [u8]);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn register_code_module(spec_ptr: *const u8, spec_len: u32, src_ptr: *const u8, src_len: u32) {
    let spec = core::str::from_utf8(unsafe { safe_bytes(spec_ptr, spec_len) }).unwrap_or("").to_string();
    let src = core::str::from_utf8(unsafe { safe_bytes(src_ptr, src_len) }).unwrap_or("").to_string();
    with_runtime(|rt| rt.registry.push((spec, ModuleEntry::Code(src))));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn register_native_module(spec_ptr: *const u8, spec_len: u32, names_ptr: *const u8, names_len: u32, base_id: u32) {
    use alloc::vec::Vec;
    let spec = core::str::from_utf8(unsafe { safe_bytes(spec_ptr, spec_len) }).unwrap_or("").to_string();
    let names_str = core::str::from_utf8(unsafe { safe_bytes(names_ptr, names_len) }).unwrap_or("");
    let funcs: Vec<(String, u32)> = names_str.split('\n')
        .filter(|n| !n.is_empty())
        .enumerate()
        .map(|(i, name)| (name.to_string(), base_id + i as u32))
        .collect();
    with_runtime(|rt| rt.registry.push((spec, ModuleEntry::Native(funcs))));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn reset_modules() {
    with_runtime(|rt| {
        rt.registry.clear();
        rt.manifests.clear();
        rt.handles.clear();
        rt.error_stash.clear();
        // Paused run references the now-stale module table; drop it for a clean reset.
        rt.paused_run = None;
        // current_vm may have pointed into the dropped paused VM; clear it so a stray host_edge_* sees None instead of dangling.
        rt.current_vm = None;
    });
}

/* Copies up to SZ bytes from the host SRC buffer into an owned `String` so the caller can drop the runtime borrow before parsing. */
fn read_src(len: usize) -> Result<String, core::str::Utf8Error> {
    with_runtime(|rt| {
        let len = len.min(SZ);
        core::str::from_utf8(&rt.src[..len]).map(|s| s.to_string())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn extract_imports(len: usize) -> usize {
    let src = match read_src(len) {
        Ok(s) => s,
        Err(_) => return unsafe { write_out("") },
    };
    let specs = crate::modules::packages::scan_string_imports(&src);
    let joined = specs.join("\n");
    unsafe { write_out(&joined) }
}

/* Drive one segment of execution; on `Pending*` re-stash the VM into the recycled `PausedRun` box. */
fn step_vm(mut vm: VM<'static>, src: &str, prev_paused: Option<Box<PausedRun>>) -> u32 {
    let result = {
        let _guard = VmGuard::new(&mut vm);
        vm.run()
    };
    match result {
        Ok(_) => {
            drop(vm);
            drop(prev_paused);
            STATUS_DONE
        }
        Err(VmErr::HostYield(status)) => {
            let (kind, deadline) = match status {
                SchedulerStatus::PendingTimer(d) => (STATUS_PENDING_TIMER, d),
                SchedulerStatus::PendingFrame => (STATUS_PENDING_FRAME, 0),
                SchedulerStatus::PendingEvent => (STATUS_PENDING_EVENT, 0),
                SchedulerStatus::PendingHostCall => (STATUS_PENDING_HOST_CALL, 0),
                SchedulerStatus::Done => (STATUS_DONE, 0),
            };
            let mut paused = match prev_paused {
                Some(mut b) => {
                    b.vm = Some(vm);
                    b.last_yield_deadline_ns = deadline;
                    b
                }
                None => Box::new(PausedRun {
                    vm: Some(vm),
                    last_yield_deadline_ns: deadline,
                }),
            };
            // Re-publish `current_vm` to the boxed VM so embedder calls like `host_edge_encode` (run between this yield and the next `run_resume`) can still allocate into the heap. The Box's address is stable across the move into rt.
            let vm_ptr = paused.vm.as_mut().map(|v| NonNull::from(v).cast::<VM<'static>>());
            with_runtime(|rt| {
                rt.paused_run = Some(paused);
                rt.current_vm = vm_ptr;
            });
            kind
        }
        Err(e) => {
            let traceback = e.render_traceback(
                src, vm.error_pos(), None,
                vm.call_stack_frames(), vm.function_names_ref(),
            );
            drop(vm);
            drop(prev_paused);
            let n = unsafe { write_out(&traceback) };
            STATUS_ERROR | ((n as u32) & STATUS_PAYLOAD_MASK)
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn run_start(len: usize) -> u32 {
    // Discard any previous paused run; a fresh `run_start` is a hard reset of execution state.
    with_runtime(|rt| { rt.paused_run = None; });

    let src = match read_src(len) {
        Ok(s) => s,
        Err(e) => {
            let msg = s!("input rejected: invalid utf-8 at byte ", int e.valid_up_to());
            let n = unsafe { write_out(&msg) };
            return STATUS_ERROR | ((n as u32) & STATUS_PAYLOAD_MASK);
        }
    };

    let (tokens, lex_errs) = lex(&src);
    let resolver = Box::new(WasmHostResolver { dir: String::new() });
    let mut p = Parser::with_resolver(&src, tokens.into_iter(), resolver);
    for e in lex_errs {
        p.errors.push(Diagnostic { start: e.start, end: e.end, msg: e.msg.into() });
    }
    let (mut chunk, errs) = p.parse();

    if !errs.is_empty() {
        let mut buf = String::new();
        for (i, e) in errs.iter().enumerate() {
            if i > 0 { buf.push('\n'); }
            buf.push_str(&e.render(&src, None));
        }
        let n = unsafe { write_out(&buf) };
        return STATUS_ERROR | ((n as u32) & STATUS_PAYLOAD_MASK);
    }

    crate::modules::vm::optimizer::constant_fold(&mut chunk);

    // Leak chunk so its lifetime survives across `run_resume`; reclaimed on page reload.
    let chunk_static: &'static SSAChunk = Box::leak(Box::new(chunk));
    let mut vm = VM::with_limits(chunk_static, Limits::sandbox());
    vm.print_hook = Some(stream_print);
    vm.set_time_hook(now_ns_host);
    vm.strict_input = true;

    let inp_text = with_runtime(|rt| {
        if rt.inp_len == 0 { return String::new(); }
        let bytes = &rt.inp[..rt.inp_len];
        let inp = core::str::from_utf8(bytes).unwrap_or("").to_string();
        rt.inp_len = 0;
        inp
    });
    if !inp_text.is_empty() {
        vm.input_buffer = inp_text.split('\n').map(alloc::string::String::from).collect();
    }

    step_vm(vm, &src, None)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn run_resume() -> u32 {
    let paused = match with_runtime(|rt| rt.paused_run.take()) {
        Some(p) => p,
        None => {
            let n = unsafe { write_out("RuntimeError: run_resume called with no paused run") };
            return STATUS_ERROR | ((n as u32) & STATUS_PAYLOAD_MASK);
        }
    };
    // Take VM out so `step_vm` owns it; recycle the empty Box for the next stash.
    let mut paused_box = paused;
    let vm = paused_box.vm.take().expect("paused_run with no VM is a runtime bug");
    step_vm(vm, "", Some(paused_box))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn run_push_event(ptr: *const u8, len: u32) -> i32 {
    let bytes = unsafe { safe_bytes(ptr, len) };
    let s = match core::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => return 1,
    };
    with_runtime(|rt| {
        let Some(paused) = rt.paused_run.as_mut() else { return 1; };
        let Some(vm) = paused.vm.as_mut() else { return 1; };
        let val = match vm.heap.alloc(HeapObj::Str(s)) {
            Ok(v) => v,
            Err(_) => return 2,
        };
        vm.inject_event(val);
        0
    })
}

/* Wake a `WaitingHostCall` coro: inject `handle`'s Val into its saved-stack top. 0 ok / 1 stale / 2 no waiter / 3 no paused run. */
#[unsafe(no_mangle)]
pub unsafe extern "C" fn set_host_result(handle: u32) -> i32 {
    let val = match super::get_val(handle) {
        Some(v) => v,
        None => return 1,
    };
    super::with_runtime(|rt| { rt.handles.release(handle); });
    super::with_runtime(|rt| {
        let Some(paused) = rt.paused_run.as_mut() else { return 3; };
        let Some(vm) = paused.vm.as_mut() else { return 3; };
        if vm.inject_host_result(val) { 0 } else { 2 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn last_yield_deadline_ns() -> u64 {
    with_runtime(|rt| rt.paused_run.as_ref().map(|p| p.last_yield_deadline_ns).unwrap_or(0))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn run(len: usize) -> usize {
    let src = match read_src(len) {
        Ok(s) => s,
        Err(e) => return unsafe {
            write_out(&s!("input rejected: invalid utf-8 at byte ", int e.valid_up_to()))
        },
    };

    let (tokens, lex_errs) = lex(&src);
    let resolver = Box::new(WasmHostResolver { dir: String::new() });
    let mut p = Parser::with_resolver(&src, tokens.into_iter(), resolver);
    for e in lex_errs {
        p.errors.push(Diagnostic { start: e.start, end: e.end, msg: e.msg.into() });
    }
    let (mut chunk, errs) = p.parse();

    let out: String = if !errs.is_empty() {
        let mut s = String::new();
        for (i, e) in errs.iter().enumerate() {
            if i > 0 { s.push('\n'); }
            s.push_str(&e.render(&src, None));
        }
        s
    } else {
        crate::modules::vm::optimizer::constant_fold(&mut chunk);
        let mut vm = VM::with_limits(&chunk, Limits::sandbox());
        vm.print_hook = Some(stream_print);
        vm.set_time_hook(now_ns_host);
        vm.strict_input = true;
        // Drain any host-supplied input bytes; `UTF-8` invalid bytes degrade to an empty input rather than UB.
        let inp_text = with_runtime(|rt| {
            if rt.inp_len == 0 { return String::new(); }
            let bytes = &rt.inp[..rt.inp_len];
            let inp = core::str::from_utf8(bytes).unwrap_or("").to_string();
            rt.inp_len = 0;
            inp
        });
        if !inp_text.is_empty() {
            vm.input_buffer = inp_text.split('\n').map(alloc::string::String::from).collect();
        }

        // Publish VM for re-entrant host_edge_op via RAII guard so a panic or early return cannot leave a stale pointer in the runtime.
        let _guard = VmGuard::new(&mut vm);
        let result = vm.run();

        match result {
            Ok(_) => String::new(),
            // Legacy `run` cannot suspend; embedders that need `sleep(n>0)` / `frame()` / `receive()` must drive `run_start` + `run_resume`.
            Err(VmErr::HostYield(_)) => String::from(
                "RuntimeError: scheduler suspended; this build's legacy `run` entry has no resume — drive `run_start` / `run_resume` instead.",
            ),
            Err(e) => e.render_traceback(
                &src, vm.error_pos(), None,
                vm.call_stack_frames(), vm.function_names_ref(),
            ),
        }
    };

    unsafe { write_out(&out) }
}
