use crate::modules::lexer::lex;
use crate::modules::parser::{Parser, Diagnostic};
use crate::modules::vm::{VM, Limits};
use alloc::{boxed::Box, string::{String, ToString}};
use crate::s;

use super::{INP, INP_LEN, ModuleEntry, OUT, SRC, SZ, error_stash, handles, registry, manifests, write_out, stream_print};
use super::resolver::WasmHostResolver;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn src_ptr() -> *mut u8 {
    core::ptr::addr_of_mut!(SRC) as *mut u8
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn out_ptr() -> *const u8 {
    core::ptr::addr_of!(OUT) as *const u8
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasm_alloc(size: u32) -> *mut u8 {
    let v = alloc::vec![0u8; size as usize];
    Box::into_raw(v.into_boxed_slice()) as *mut u8
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn register_code_module(
    spec_ptr: *const u8, spec_len: u32,
    src_ptr: *const u8, src_len: u32,
) {
    let spec = core::str::from_utf8(unsafe {
        core::slice::from_raw_parts(spec_ptr, spec_len as usize)
    }).unwrap_or("").to_string();
    let src = core::str::from_utf8(unsafe {
        core::slice::from_raw_parts(src_ptr, src_len as usize)
    }).unwrap_or("").to_string();
    unsafe { registry().push((spec, ModuleEntry::Code(src))); }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn register_native_module(
    spec_ptr: *const u8, spec_len: u32,
    names_ptr: *const u8, names_len: u32,
    base_id: u32,
) {
    use alloc::{string::ToString, vec::Vec};
    let spec = core::str::from_utf8(unsafe {
        core::slice::from_raw_parts(spec_ptr, spec_len as usize)
    }).unwrap_or("").to_string();
    let names_str = core::str::from_utf8(unsafe {
        core::slice::from_raw_parts(names_ptr, names_len as usize)
    }).unwrap_or("");
    let funcs: Vec<(String, u32)> = names_str.split('\n')
        .filter(|n| !n.is_empty())
        .enumerate()
        .map(|(i, name)| (name.to_string(), base_id + i as u32))
        .collect();
    unsafe { registry().push((spec, ModuleEntry::Native(funcs))); }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn reset_modules() {
    unsafe {
        registry().clear();
        manifests().clear();
    }
    handles().clear();
    error_stash().clear();
}

/* Reads up to SZ bytes from the host-owned SRC buffer and validates UTF-8.
   `len` is capped so the slice never extends past the buffer; callers decide
   how to surface a UTF-8 failure (silent vs. user-facing error). */
unsafe fn read_src(len: usize) -> Result<&'static str, core::str::Utf8Error> {
    let len = len.min(SZ);
    let bytes = unsafe {
        core::slice::from_raw_parts(core::ptr::addr_of!(SRC) as *const u8, len)
    };
    core::str::from_utf8(bytes)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn extract_imports(len: usize) -> usize {
    let src = match unsafe { read_src(len) } {
        Ok(s) => s,
        Err(_) => return unsafe { write_out("") },
    };
    let specs = crate::modules::packages::scan_string_imports(src);
    let joined = specs.join("\n");
    unsafe { write_out(&joined) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn run(len: usize) -> usize {
    let src = match unsafe { read_src(len) } {
        Ok(s) => s,
        Err(e) => return unsafe {
            write_out(&s!("input rejected: invalid utf-8 at byte ", int e.valid_up_to()))
        },
    };

    let (tokens, lex_errs) = lex(src);
    let resolver = Box::new(WasmHostResolver { dir: String::new() });
    let mut p = Parser::with_resolver(src, tokens.into_iter(), resolver);
    for e in lex_errs {
        p.errors.push(Diagnostic { start: e.start, end: e.end, msg: e.msg.into() });
    }
    let (mut chunk, errs) = p.parse();

    let out: String = if !errs.is_empty() {
        let mut s = String::new();
        for (i, e) in errs.iter().enumerate() {
            if i > 0 { s.push('\n'); }
            s.push_str(&e.render(src, None));
        }
        s
    } else {
        crate::modules::vm::optimizer::constant_fold(&mut chunk);
        let mut vm = VM::with_limits(&chunk, Limits::sandbox());
        vm.print_hook = Some(stream_print);
        vm.strict_input = true;
        let inp_len = unsafe { INP_LEN };
        if inp_len > 0 {
            let inp = unsafe { core::str::from_utf8_unchecked(
                core::slice::from_raw_parts(core::ptr::addr_of!(INP) as *const u8, inp_len)
            )};
            vm.input_buffer = inp.split('\n').map(alloc::string::String::from).collect();
            unsafe { INP_LEN = 0; }
        }

        // Publish VM for re-entrant host_edge_op; cleared on scope exit.
        let vm_ptr: *mut VM<'static> = (&mut vm as *mut VM<'_>).cast();
        unsafe { super::CURRENT_VM = vm_ptr; }
        let result = vm.run();
        unsafe { super::CURRENT_VM = core::ptr::null_mut(); }

        match result {
            Ok(_) => String::new(),
            Err(e) => e.render_traceback(
                src, vm.error_pos(), None,
                vm.call_stack_frames(), vm.function_names_ref(),
            ),
        }
    };

    unsafe { write_out(&out) }
}
