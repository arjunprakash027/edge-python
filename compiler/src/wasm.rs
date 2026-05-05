/* Output streaming: each print() calls the host-imported `js_print` instead of
   buffering, so the Worker fires a postMessage per line as WASM executes.
   Future DOM pool: same import pattern — WASM writes commands to linear memory,
   host reads them on each signal; no serialization, one transferable per frame.

   Module imports go through the same host-import pattern. JS pre-fetches every
   spec the script imports, then calls `register_code_module` (for .py source)
   or `register_native_module` (for .wasm exports). When the parser asks the
   `WasmHostResolver` for a module, it returns the pre-staged entry. For native
   bindings, each function call routes through the `js_call_native` host import
   so JS can dispatch into the right WebAssembly instance — Edge Python's
   bytecode never has to know whether a binding is .wasm-backed, JS-implemented,
   or anything else. */
#[cfg(target_arch = "wasm32")]
mod runtime {
    use lol_alloc::LeakingPageAllocator;
    use crate::modules::{lexer::lex, parser::{Parser, Diagnostic}, vm::{VM, Limits}};
    use crate::modules::vm::types::{HeapPool, Val, VmErr};
    use crate::modules::packages::{NativeBinding, Resolved, Resolver};
    use alloc::{boxed::Box, string::{String, ToString}, sync::Arc, vec::Vec};
    use crate::s;

    #[link(wasm_import_module = "env")]
    unsafe extern "C" {
        fn js_print(ptr: *const u8, len: usize);
        /* Dispatches a CallExtern for a native binding registered by JS.
           `id` is the integer assigned at register time; `args_ptr/len`
           describe a packed u64 array (one Val per slot, NaN-boxed wire
           format). Returns a u64 that's a Val bit-cast. */
        fn js_call_native(id: u32, args_ptr: *const u64, args_len: u32) -> i64;
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

    /* Pre-staged module registry. JS populates this via `register_code_module`
       and `register_native_module` before calling `run()`. The Resolver consults
       it during compilation; modules not present yield a parse-time error. */
    enum ModuleEntry {
        Code(String),
        Native(Vec<(String, u32)>),
    }

    static mut REGISTRY: Option<Vec<(String, ModuleEntry)>> = None;

    unsafe fn registry() -> &'static mut Vec<(String, ModuleEntry)> {
        unsafe {
            let p = core::ptr::addr_of_mut!(REGISTRY);
            if (*p).is_none() { *p = Some(Vec::new()); }
            (*p).as_mut().unwrap()
        }
    }

    struct WasmHostResolver;

    impl Resolver for WasmHostResolver {
        fn resolve(&mut self, spec: &str) -> Result<Resolved, String> {
            let reg = unsafe { registry() };
            let entry = reg.iter().find(|(s, _)| s == spec)
                .ok_or_else(|| s!(
                    "module '", str spec,
                    "' not registered (host did not pre-fetch / register before run())"
                ))?;
            match &entry.1 {
                ModuleEntry::Code(src) => Ok(Resolved::Code(src.clone())),
                ModuleEntry::Native(funcs) => {
                    let bindings: Vec<NativeBinding> = funcs.iter().map(|(name, id)| {
                        let id = *id;
                        let closure = move |_: &mut HeapPool, args: &[Val]| -> Result<Val, VmErr> {
                            let raw: Vec<u64> = args.iter().map(|v| v.0).collect();
                            let result_bits = unsafe {
                                js_call_native(id, raw.as_ptr(), raw.len() as u32)
                            };
                            Ok(Val(result_bits as u64))
                        };
                        NativeBinding {
                            name: name.clone(),
                            func: Arc::new(closure),
                            pure: false,
                        }
                    }).collect();
                    Ok(Resolved::Native(bindings))
                }
            }
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn src_ptr() -> *mut u8 {
        core::ptr::addr_of_mut!(SRC) as *mut u8
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn out_ptr() -> *const u8 {
        core::ptr::addr_of!(OUT) as *const u8
    }

    /* General-purpose linear-memory allocator for JS to write variable-sized
       data into (module specs, source code, names lists). The returned pointer
       lives until the module is unloaded (LeakingPageAllocator never frees,
       which is fine for the ephemeral run-then-reset lifecycle). */
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn wasm_alloc(size: u32) -> *mut u8 {
        let v = alloc::vec![0u8; size as usize];
        Box::into_raw(v.into_boxed_slice()) as *mut u8
    }

    /* Register a `.py` code module under the given spec. JS calls this once
       per code module after fetching its source. Spec must match what the
       parser will look up via `from "<spec>" import ...`. */
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

    /* Register a native (.wasm-backed or JS-backed) module under the given
       spec. `names` is newline-separated; each name gets a unique callback id
       starting at `base_id` and incrementing. JS keeps a parallel table that
       maps id → actual callable, so when EdgePython invokes js_call_native(id),
       JS routes to the right function. */
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn register_native_module(
        spec_ptr: *const u8, spec_len: u32,
        names_ptr: *const u8, names_len: u32,
        base_id: u32,
    ) {
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

    /* Clear the registry between runs so leftover state from a previous
       compile doesn't leak into the next one. */
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn reset_modules() {
        unsafe { registry().clear(); }
    }

    /* Pre-scan for the JS host. Walks the source for quoted module specs in
       `from "..." import` statements and writes them newline-separated into
       OUT. The host parses the result, fetches each URL/path in parallel,
       then calls register_*_module() for each one before invoking run().

       Bare-name imports (`from json import x`) aren't returned — those resolve
       via the host's import map, which is JS-side state outside the parser. */
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn extract_imports(len: usize) -> usize {
        let len = len.min(SZ);
        let src = match core::str::from_utf8(unsafe {
            core::slice::from_raw_parts(core::ptr::addr_of!(SRC) as *const u8, len)
        }) {
            Ok(s) => s,
            Err(_) => return unsafe { write_out("") },
        };
        let specs = crate::modules::packages::scan_string_imports(src);
        let joined = specs.join("\n");
        unsafe { write_out(&joined) }
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
        let mut p = Parser::with_resolver(src, tokens.into_iter(), Box::new(WasmHostResolver));
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
                Err(e) => e.render_at(src, vm.error_pos(), None),
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
    use crate::modules::{lexer::lex, parser::Parser, vm::VM};

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
            let (tokens, lex_errs) = lex(&case.src);
            assert!(lex_errs.is_empty(), "lex error on {:?}: {:?}", case.src, lex_errs.iter().map(|e| e.msg).collect::<Vec<_>>());
            let (chunk, errs) = Parser::new(&case.src, tokens.into_iter()).parse();
            if !errs.is_empty() {
                match &case.error {
                    Some(expected) => {
                        assert!(
                            errs.iter().any(|e| e.msg.contains(expected.as_str())),
                            "wrong parse error on {:?}: got {:?}, expected '{}'",
                            case.src,
                            errs.iter().map(|e| &e.msg).collect::<Vec<_>>(),
                            expected
                        );
                        continue;
                    }
                    None => panic!("parse error on {:?}: {:?}", case.src, errs.iter().map(|e| &e.msg).collect::<Vec<_>>()),
                }
            }

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
