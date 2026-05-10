use crate::modules::lexer::lex;
use crate::modules::parser::{Parser, Diagnostic};
use crate::modules::vm::{VM, Limits};
use alloc::{boxed::Box, string::{String, ToString}};
use crate::s;

use super::{ModuleEntry, SZ, VmGuard, safe_bytes, stream_print, with_runtime, write_out};
use super::resolver::WasmHostResolver;

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

/* Frees a `wasm_alloc` buffer. Host must pass the exact `size` it requested — a mismatched length rebuilds the wrong Box layout. Null or zero is a no-op. */
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasm_free(ptr: *mut u8, size: u32) {
    if ptr.is_null() || size == 0 { return; }
    unsafe {
        let slice = core::slice::from_raw_parts_mut(ptr, size as usize);
        let _ = Box::from_raw(slice as *mut [u8]);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn register_code_module(
    spec_ptr: *const u8, spec_len: u32,
    src_ptr: *const u8, src_len: u32,
) {
    let spec = core::str::from_utf8(unsafe { safe_bytes(spec_ptr, spec_len) })
        .unwrap_or("").to_string();
    let src = core::str::from_utf8(unsafe { safe_bytes(src_ptr, src_len) })
        .unwrap_or("").to_string();
    with_runtime(|rt| rt.registry.push((spec, ModuleEntry::Code(src))));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn register_native_module(
    spec_ptr: *const u8, spec_len: u32,
    names_ptr: *const u8, names_len: u32,
    base_id: u32,
) {
    use alloc::vec::Vec;
    let spec = core::str::from_utf8(unsafe { safe_bytes(spec_ptr, spec_len) })
        .unwrap_or("").to_string();
    let names_str = core::str::from_utf8(unsafe { safe_bytes(names_ptr, names_len) })
        .unwrap_or("");
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
            Err(e) => e.render_traceback(
                &src, vm.error_pos(), None,
                vm.call_stack_frames(), vm.function_names_ref(),
            ),
        }
    };

    unsafe { write_out(&out) }
}
