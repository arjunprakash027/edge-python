/* Output streaming: each print() calls the host-imported `js_print` instead of
   buffering, so the Worker fires a postMessage per line as WASM executes.
   Future DOM pool: same import pattern — WASM writes commands to linear memory,
   host reads them on each signal; no serialization, one transferable per frame. */
#[cfg(target_arch = "wasm32")]
mod runtime {
    use lol_alloc::LeakingPageAllocator;
    use crate::modules::{lexer::lex, parser::{Parser, Diagnostic}, vm::{VM, Limits, VmErr}};
    use alloc::string::String;
    use crate::s;

    #[link(wasm_import_module = "env")]
    unsafe extern "C" {
        fn js_print(ptr: *const u8, len: usize);
    }

    fn stream_print(s: &str) {
        unsafe { js_print(s.as_ptr(), s.len()); }
    }

    #[global_allocator]
    static A: LeakingPageAllocator = LeakingPageAllocator;

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

    const SZ: usize = 1 << 20;
    static mut SRC: [u8; SZ] = [0; SZ];
    static mut OUT: [u8; SZ] = [0; SZ];
    static mut INP: [u8; SZ] = [0; SZ];
    static mut INP_LEN: usize = 0;

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn src_ptr() -> *mut u8 {
        core::ptr::addr_of_mut!(SRC) as *mut u8
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn out_ptr() -> *const u8 {
        core::ptr::addr_of!(OUT) as *const u8
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn run(len: usize) -> usize {
        let len = len.min(SZ);
        let src = match core::str::from_utf8(unsafe {
            core::slice::from_raw_parts(core::ptr::addr_of!(SRC) as *const u8, len)
        }) {
            Ok(s) => s,
            Err(e) => return unsafe {
                write_out(&s!("input rejected: invalid utf-8 at byte ", int e.valid_up_to()))
            },
        };

        let (tokens, lex_errs) = lex(src);
        let mut p = Parser::new(src, tokens.into_iter());
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
            match vm.run() {
                Ok(_) => String::new(),
                Err(e) => match &e {
                    VmErr::Type(m) => s!("TypeError: ", str m),
                    VmErr::Value(m) => s!("ValueError: ", str m),
                    VmErr::Runtime(m) => s!("RuntimeError: ", str m),
                    VmErr::Name(n) => s!("NameError: '", str n, "'"),
                    VmErr::Raised(r) => s!("Exception: ", str r),
                    other => other.as_str().into(),
                }
            }
        };

        unsafe { write_out(&out) }
    }

    unsafe fn write_out(s: &str) -> usize {
        let b = s.as_bytes();
        let n = b.len().min(SZ);
        unsafe {
            core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(OUT) as *mut u8, n)
                .copy_from_slice(&b[..n]);
        }
        n
    }
}

#[cfg(all(test, feature = "wasm-tests"))]
mod tests {
    use crate::modules::{lexer::lexer, parser::Parser, vm::VM};

    #[derive(serde::Deserialize)]
    struct Case {
        src: String,
        output: Vec<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        input: Vec<String>,
    }

    #[test]
    fn vm_cases() {
        let cases: Vec<Case> = serde_json::from_str(
            include_str!("../tests/cases/vm.json")
        ).expect("invalid JSON");

        for case in cases {
            let (chunk, errs) = Parser::new(&case.src, lexer(&case.src)).parse();
            assert!(errs.is_empty(), "parse error on {:?}: {:?}", case.src, errs.iter().map(|e| &e.msg).collect::<Vec<_>>());

            let mut vm = VM::new(&chunk);
            vm.strict_input = true;

            vm.input_buffer = case.input.clone();
            let expects_input_error = case.input.is_empty()
                && (case.src.contains("input(") || case.src.contains("input ("));

            match vm.run() {
                Ok(_) => {
                    assert!(
                        !expects_input_error,
                        "expected input() to error under WASM strict mode for: {:?}", case.src
                    );
                    assert_eq!(vm.output, case.output, "output mismatch on: {:?}", case.src);
                }
                Err(e) => match &case.error {
                    Some(expected) => assert!(
                        e.to_string().contains(expected.as_str()),
                        "wrong error on {:?}: got '{}', expected '{}'", case.src, e, expected
                    ),
                    None if expects_input_error => assert!(
                        e.to_string().contains("input"),
                        "expected input RuntimeError under WASM strict mode for: {:?}, got: {}",
                        case.src, e
                    ),
                    None => panic!("VM error on {:?}: {}", case.src, e),
                }
            }
        }
    }
}