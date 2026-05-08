/* WASM bridge — orchestration layer.
 *
 * This file wires the Edge Python parser/VM to the JS shim and to the
 * universal handle ABI. The sealed-contract spec (host imports, guest
 * export shape, op codes, tags, error kinds, primitive codec, handle
 * table layout) lives in `crate::abi`. Look there to understand or
 * extend the contract — DO NOT add wire-level constants or behavior to
 * this file. The user-facing version of the spec, with a worked
 * Rust + Python example, is at `documentation/reference/wasm-abi.md`.
 *
 * What lives here:
 *   - Three JS imports the bridge consumes (`js_print`,
 *     `js_call_native`, `js_fetch_bytes`).
 *   - The WASM exports the JS shim drives (`run`, `register_*_module`,
 *     `wasm_alloc`, `host_edge_*`, etc.).
 *   - `WasmHostResolver` (walk-up `packages.json` resolution + native
 *     binding closure that bridges `CallExtern` to `js_call_native`).
 *   - VM-coupled implementations of `host_edge_op`'s op codes (Call,
 *     GetAttr, GetItem, etc.) — they need access to `methods.rs` and
 *     `HeapPool` so they live next to the parser/VM glue.
 *   - SRC / OUT / INP buffers, the `LeakingPageAllocator`, and the
 *     panic handler. */

#[cfg(target_arch = "wasm32")]
mod runtime {
    use lol_alloc::{AssumeSingleThreaded, LeakingAllocator};
    use crate::abi::{
        classify_decode, classify_encode,
        DecodeBits, EncodeRequest, ErrorKind, ErrorStash, HandleTable, Op,
        PrimitiveBytes, TAG_INVALID,
    };
    use crate::modules::{lexer::lex, parser::{Parser, Diagnostic}, vm::{VM, Limits}};
    use crate::modules::vm::types::{HeapObj, HeapPool, Val, VmErr};
    use crate::modules::vm::handlers::methods::{lookup_method, dispatch_method};
    use crate::modules::packages::{
        NativeBinding, Resolved, Resolver,
        Manifest, parse_manifest, walk_up_dirs, dir_of, join_relative,
    };
    use crate::modules::fx::FxHashSet;
    use alloc::{boxed::Box, rc::Rc, string::{String, ToString}, sync::Arc, vec, vec::Vec};
    use core::cell::RefCell;
    use crate::s;

    #[link(wasm_import_module = "env")]
    unsafe extern "C" {
        fn js_print(ptr: *const u8, len: usize);

        /* Invoked when the Edge Python VM dispatches a `CallExtern` for a
           function registered via `register_native_module`. The host
           fills `argv` with `argc` handles (already registered in the
           host's handle table; valid for the call's duration) and the
           guest writes the return handle into `out`. Returns 0/1 status. */
        fn js_call_native(
            id: u32,
            argv_ptr: *const u32, argc: u32,
            out: *mut u32,
        ) -> i32;

        /* Returns the host-cached bytes for `spec` so the parser can
           verify a `#sha256-...` integrity fragment OR walk up looking
           for a `<dir>/packages.json`. A null return means "no bytes",
           which the resolver treats as "keep walking up". */
        fn js_fetch_bytes(spec_ptr: *const u8, spec_len: u32, out_len: *mut u32) -> *mut u8;
    }

    fn stream_print(s: &str) {
        unsafe { js_print(s.as_ptr(), s.len()); }
    }

    /* Bump-pointer allocator: places multiple allocations per WebAssembly
       page instead of one page per alloc. The default `LeakingPageAllocator`
       requests `memory.grow(1)` for every Vec/String, which on hosts that
       gate page commits through a hypervisor (Snapdragon X Elite Copilot+
       PCs run V8 with HVCI/VBS active) costs ~0.2 ms per call. A perceptron
       training run produces ~3,000 small allocs ⇒ ~600 ms of grow overhead.
       Bumping inside pages cuts that to ~50 grows total.
       `AssumeSingleThreaded` wraps the non-`Sync` bump allocator so it can
       live in a `static`; sound because each Web Worker / host instance
       runs the WASM on a single thread. */
    #[global_allocator]
    static A: AssumeSingleThreaded<LeakingAllocator> =
        unsafe { AssumeSingleThreaded::new(LeakingAllocator::new()) };

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

    const SZ: usize = 1 << 20;
    static mut SRC: [u8; SZ] = [0; SZ];
    static mut OUT: [u8; SZ] = [0; SZ];
    static mut INP: [u8; SZ] = [0; SZ];
    static mut INP_LEN: usize = 0;

    /* ---------- Module registry (existing, unchanged shape) -------------- */

    enum ModuleEntry {
        Code(String),
        Native(Vec<(String, u32)>),
    }

    static mut REGISTRY: Option<Vec<(String, ModuleEntry)>> = None;
    static mut MANIFESTS: Option<Vec<(String, Manifest)>> = None;

    unsafe fn registry() -> &'static mut Vec<(String, ModuleEntry)> {
        unsafe {
            let p = core::ptr::addr_of_mut!(REGISTRY);
            if (*p).is_none() { *p = Some(Vec::new()); }
            (*p).as_mut().unwrap()
        }
    }

    unsafe fn manifests() -> &'static mut Vec<(String, Manifest)> {
        unsafe {
            let p = core::ptr::addr_of_mut!(MANIFESTS);
            if (*p).is_none() { *p = Some(Vec::new()); }
            (*p).as_mut().unwrap()
        }
    }

    /* ---------- ABI plumbing (HandleTable + ErrorStash from `abi`) ------- */

    /* The contract — handle layout, op codes, tag values, primitive
       codec — lives in `crate::abi`. Here we just hold the singletons
       and provide thin accessors. Both are reset by `reset_modules()`. */
    static mut HANDLES: Option<HandleTable> = None;
    static mut ERROR_STASH: Option<ErrorStash> = None;

    fn handles() -> &'static mut HandleTable {
        unsafe {
            let p = core::ptr::addr_of_mut!(HANDLES);
            if (*p).is_none() { *p = Some(HandleTable::new()); }
            (*p).as_mut().unwrap()
        }
    }

    fn error_stash() -> &'static mut ErrorStash {
        unsafe {
            let p = core::ptr::addr_of_mut!(ERROR_STASH);
            if (*p).is_none() { *p = Some(ErrorStash::new()); }
            (*p).as_mut().unwrap()
        }
    }

    /* Convenience: the Val ↔ u32-handle round-trip. The ABI module
       stores raw u64 bits; we cast via `Val(bits)`. */
    fn put_val(v: Val) -> u32 { handles().put(v.0) }
    fn get_val(h: u32) -> Option<Val> { handles().get(h).map(Val) }

    /* Map a VmErr into the ABI's typed ErrorKind + message. */
    fn err_to_kind(e: &VmErr) -> ErrorKind {
        match e {
            VmErr::Type(_) | VmErr::TypeMsg(_) => ErrorKind::Type,
            VmErr::Value(_)                    => ErrorKind::Value,
            VmErr::Runtime(_)                  => ErrorKind::Runtime,
            VmErr::Attribute(_) | VmErr::Name(_) => ErrorKind::Attribute,
            VmErr::Raised(s) => {
                if s.starts_with("ValueError")      { ErrorKind::Value }
                else if s.starts_with("IndexError") { ErrorKind::Index }
                else if s.starts_with("KeyError")   { ErrorKind::Key }
                else                                { ErrorKind::Runtime }
            }
            _ => ErrorKind::Runtime,
        }
    }

    fn stash_error(e: VmErr) {
        error_stash().set_typed(err_to_kind(&e), e.render());
    }

    /* ---------- Reentry pointer to the running VM ------------------------ */

    /* Set during `run()` so that `host_edge_op` (called re-entrantly from
       a guest's edge_op) can dispatch methods through the VM's heap and
       method table. Cleared at end of run. The static *mut is sound only
       inside that window. */
    static mut CURRENT_VM: *mut VM<'static> = core::ptr::null_mut();

    fn with_vm<R>(f: impl FnOnce(&mut VM<'static>) -> R) -> Option<R> {
        unsafe {
            if CURRENT_VM.is_null() { None }
            else { Some(f(&mut *CURRENT_VM)) }
        }
    }

    /* ---------- The walk-up resolver (unchanged) ------------------------- */

    struct WasmHostResolver { dir: String }

    impl Resolver for WasmHostResolver {
        fn resolve(&mut self, spec: &str) -> Result<Resolved, String> {
            if !spec.contains('/') {
                let dir = self.dir.clone();
                return self.resolve_bare(spec, &dir);
            }
            let canonical = if spec.contains("://") || spec.starts_with('/') {
                spec.to_string()
            } else {
                join_relative(&self.dir, spec)
            };
            self.resolve_canonical(&canonical)
        }

        fn fetch_bytes(&mut self, spec: &str) -> Result<Vec<u8>, String> {
            let mut len: u32 = 0;
            let ptr = unsafe {
                js_fetch_bytes(spec.as_ptr(), spec.len() as u32, &mut len as *mut u32)
            };
            if ptr.is_null() {
                return Err(s!("no bytes cached by host for '", str spec, "'"));
            }
            Ok(unsafe { Vec::from_raw_parts(ptr, len as usize, len as usize) })
        }

        fn child(&self, spec: &str) -> Box<dyn Resolver> {
            Box::new(WasmHostResolver { dir: dir_of(spec).to_string() })
        }
    }

    impl WasmHostResolver {
        fn resolve_bare(&mut self, name: &str, start_dir: &str) -> Result<Resolved, String> {
            let mut visited: FxHashSet<String> = FxHashSet::default();
            let mut search_dir = start_dir.to_string();
            let mut hops: u32 = 0;
            loop {
                if hops > 32 {
                    return Err(s!("packages.json walk-up exceeded 32 hops resolving '", str name, "'"));
                }
                hops += 1;

                let mut hit: Option<(String, Option<String>, Option<String>)> = None;
                for dir in walk_up_dirs(&search_dir) {
                    let m_spec = s!(str &dir, "packages.json");
                    if let Some((target, ext)) = self.lookup_in_manifest(&m_spec, name)? {
                        hit = Some((dir, target, ext));
                        break;
                    }
                }
                let Some((dir, target, ext)) = hit else {
                    return Err(s!(
                        "no packages.json above '", str start_dir,
                        "' declares '", str name, "'"));
                };
                if let Some(target) = target {
                    let canonical = join_relative(&dir, &target);
                    return self.resolve_canonical(&canonical);
                }
                let m_spec = s!(str &dir, "packages.json");
                if let Some(ext) = ext {
                    if !visited.insert(m_spec) {
                        return Err(s!("circular extends chain in packages.json"));
                    }
                    let mut next = join_relative(&dir, &ext);
                    if !next.ends_with('/') { next.push('/'); }
                    search_dir = next;
                    continue;
                }
                return Err(s!(
                    "alias '", str name, "' not declared in '", str &m_spec, "'\n",
                    "help: declare it, add \"extends\": \"..\" to inherit, or use a quoted path"));
            }
        }

        #[allow(clippy::type_complexity)]
        fn lookup_in_manifest(
            &mut self, m_spec: &str, name: &str,
        ) -> Result<Option<(Option<String>, Option<String>)>, String> {
            let cache = unsafe { manifests() };
            if let Some((_, m)) = cache.iter().find(|(s, _)| s == m_spec) {
                return Ok(Some((m.imports.get(name).cloned(), m.extends.clone())));
            }
            let bytes = match self.fetch_bytes(m_spec) {
                Ok(b) => b,
                Err(_) => return Ok(None),
            };
            let parsed = parse_manifest(&bytes).map_err(|e| s!(
                "packages.json at '", str m_spec, "': ", str &e))?;
            let target = parsed.imports.get(name).cloned();
            let ext = parsed.extends.clone();
            cache.push((m_spec.to_string(), parsed));
            Ok(Some((target, ext)))
        }

        fn resolve_canonical(&self, spec: &str) -> Result<Resolved, String> {
            let reg = unsafe { registry() };
            let entry = reg.iter().find(|(s, _)| s == spec)
                .ok_or_else(|| s!(
                    "module '", str spec,
                    "' not registered (host did not pre-fetch / register before run())"))?;
            match &entry.1 {
                ModuleEntry::Code(src) => Ok(Resolved::Code(src.clone())),
                ModuleEntry::Native(funcs) => {
                    let bindings: Vec<NativeBinding> = funcs.iter().map(|(name, id)| {
                        let id = *id;
                        // Closure that translates the VM's CallExtern into
                        // the universal ABI's wire shape (handles in/out).
                        let closure = move |_: &mut HeapPool, args: &[Val]|
                            -> Result<Val, VmErr>
                        {
                            // 1. Register every arg as a fresh handle.
                            let argv: Vec<u32> = args.iter().map(|v| put_val(*v)).collect();
                            let mut out_handle: u32 = 0;

                            // 2. Invoke the guest export through the JS shim.
                            let status = unsafe {
                                js_call_native(
                                    id,
                                    argv.as_ptr(), argv.len() as u32,
                                    &mut out_handle as *mut u32,
                                )
                            };

                            // 3. Translate status / out_handle back into Result<Val>.
                            //    Order: read result FIRST, THEN release everything.
                            //    This is what lets a guest function pass an argv
                            //    handle straight through to *out without the
                            //    handle being freed before we read it.
                            if status != 0 {
                                for h in &argv { handles().release(*h); }
                                let (kind, msg) = error_stash().take()
                                    .unwrap_or((ErrorKind::Runtime as u32,
                                                String::from("native call failed")));
                                return Err(error_from_kind(kind, msg));
                            }
                            let result = get_val(out_handle)
                                .ok_or(VmErr::Runtime("native returned invalid handle"))?;
                            for h in &argv { handles().release(*h); }
                            handles().release(out_handle);
                            Ok(result)
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

    fn error_from_kind(kind: u32, msg: String) -> VmErr {
        match kind {
            0 => VmErr::TypeMsg(msg),
            1 => VmErr::Raised(s!("ValueError: ", str &msg)),
            3 => VmErr::Attribute(msg),
            4 => VmErr::Raised(s!("IndexError: ", str &msg)),
            5 => VmErr::Raised(s!("KeyError: ", str &msg)),
            _ => VmErr::Raised(msg),
        }
    }

    /* ---------- Existing exports (unchanged shape) ----------------------- */

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

            // Publish the VM pointer so `host_edge_op` calls re-entered
            // from a guest module can dispatch through this same VM.
            // SAFETY: cleared before this scope returns; guests can only
            // re-enter while vm.run() is on the stack.
            let vm_ptr: *mut VM<'static> = (&mut vm as *mut VM<'_>).cast();
            unsafe { CURRENT_VM = vm_ptr; }
            let result = vm.run();
            unsafe { CURRENT_VM = core::ptr::null_mut(); }

            match result {
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

    /* ---------- Universal ABI exports (host-side dispatch) --------------- */

    /* These are the host functions the JS shim calls when a guest module
       invokes one of the six wire imports. JS is responsible for copying
       guest-memory pointers into the host's memory before calling these
       (using `wasm_alloc` for the staging buffers), and for copying the
       host-produced output back into guest memory.

       The contract — `Op`, `Tag`, `ErrorKind`, the handle layout, the
       primitive codec — lives in `crate::abi`. This file just wires it
       to the parser/VM. */

    /* Shared prologue for every `dispatch_*`: resolve `recv_h` against the
       handle table and run `f` against the live VM. Two failure modes:
       the handle is stale/invalid (`invalid_recv_msg`) or the host called
       us outside `run()` ("edge_op called outside run()"). Caller passes
       the op-specific invalid-handle message as a `&'static str` so each
       error keeps its existing exact text. */
    fn with_recv<F>(invalid_recv_msg: &'static str, recv_h: u32, f: F) -> Result<Val, VmErr>
    where F: FnOnce(&mut VM<'static>, Val) -> Result<Val, VmErr>
    {
        let recv = get_val(recv_h).ok_or(VmErr::Runtime(invalid_recv_msg))?;
        with_vm(|vm| f(vm, recv))
            .ok_or(VmErr::Runtime("edge_op called outside run()"))?
    }

    /// Universal dispatch entry point. JS calls this after staging
    /// `name` and `argv` into host linear memory. Returns 0 with a
    /// fresh handle in `*out_handle`, or 1 with the error stashed for
    /// `host_edge_take_error`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn host_edge_op(
        op: u32, recv: u32,
        name_ptr: *const u8, name_len: u32,
        argv_ptr: *const u32, argc: u32,
        out_handle: *mut u32,
    ) -> i32 {
        let name = if name_len == 0 { String::new() } else {
            core::str::from_utf8(unsafe {
                core::slice::from_raw_parts(name_ptr, name_len as usize)
            }).unwrap_or("").to_string()
        };
        let args: Vec<Val> = (0..argc).filter_map(|i| {
            let h = unsafe { *argv_ptr.add(i as usize) };
            get_val(h)
        }).collect();

        let result: Result<Val, VmErr> = match Op::from_u32(op) {
            Some(Op::Call)     => dispatch_call(recv, &name, args),
            Some(Op::GetAttr)  => dispatch_get_attr(recv, &name),
            Some(Op::SetAttr)  => dispatch_set_attr(recv, &name, &args),
            Some(Op::GetItem) => dispatch_get_item(recv, &args),
            Some(Op::SetItem) => dispatch_set_item(recv, &args),
            Some(Op::Len)      => dispatch_len(recv),
            Some(Op::Iter)     => dispatch_iter(recv),
            Some(Op::IterNext) => dispatch_iter_next(recv),
            None => Err(VmErr::Raised(s!("edge_op: unsupported op ", int op as i64))),
        };

        match result {
            Ok(v) => { unsafe { *out_handle = put_val(v); } 0 }
            Err(e) => { stash_error(e); 1 }
        }
    }

    fn dispatch_call(recv_h: u32, name: &str, args: Vec<Val>) -> Result<Val, VmErr> {
        with_recv("edge_op call: invalid receiver handle", recv_h, |vm, recv| {
            let ty = vm.type_name(recv);
            let mid = lookup_method(ty, name)
                .ok_or_else(|| VmErr::Attribute(s!(
                    "'", str ty, "' object has no method '", str name, "'")))?;
            let stack_before = vm.stack.len();
            dispatch_method(vm, mid, recv, args, vec![])?;
            if vm.stack.len() != stack_before + 1 {
                return Err(VmErr::Runtime("edge_op call: method left no result"));
            }
            Ok(vm.stack.pop().unwrap())
        })
    }

    /* GetAttr: module / instance attribute, or bind a builtin method as a
       BoundMethod handle the guest can later Call. */
    fn dispatch_get_attr(recv_h: u32, name: &str) -> Result<Val, VmErr> {
        with_recv("edge_op get_attr: invalid receiver handle", recv_h, |vm, recv| {
            // Module attribute.
            if recv.is_heap()
                && let HeapObj::Module(_, attrs) = vm.heap.get(recv)
            {
                let bare = name;
                if let Some((_, v)) = attrs.iter().find(|(n, _)| n == bare) {
                    return Ok(*v);
                }
                return Err(VmErr::Attribute(s!(
                    "module has no attribute '", str name, "'")));
            }
            // Instance attribute.
            if recv.is_heap()
                && let HeapObj::Instance(_cls, attrs) = vm.heap.get(recv)
            {
                let entries = attrs.borrow().entries.clone();
                for (k, v) in &entries {
                    if k.is_heap()
                        && let HeapObj::Str(s) = vm.heap.get(*k)
                        && s == name
                    {
                        return Ok(*v);
                    }
                }
                return Err(VmErr::Attribute(s!(
                    "instance has no attribute '", str name, "'")));
            }
            // Builtin method → BoundMethod.
            let ty = vm.type_name(recv);
            if let Some(mid) = lookup_method(ty, name) {
                return vm.heap.alloc(HeapObj::BoundMethod(recv, mid));
            }
            Err(VmErr::Attribute(s!(
                "'", str ty, "' object has no attribute '", str name, "'")))
        })
    }

    /* SetAttr: write `name` on an instance's __dict__. Modules and builtin
       types reject the operation. */
    fn dispatch_set_attr(recv_h: u32, name: &str, args: &[Val]) -> Result<Val, VmErr> {
        if args.len() != 1 {
            return Err(VmErr::TypeMsg(s!("set_attr expects exactly 1 value, got ", int args.len() as i64)));
        }
        let value = args[0];
        with_recv("edge_op set_attr: invalid receiver handle", recv_h, |vm, recv| {
            if !recv.is_heap() {
                return Err(VmErr::Type("cannot set attribute on this type"));
            }
            if let HeapObj::Instance(_cls, attrs) = vm.heap.get(recv) {
                let attrs = attrs.clone();
                let key = vm.heap.alloc(HeapObj::Str(name.to_string()))?;
                attrs.borrow_mut().insert(key, value);
                return Ok(Val::none());
            }
            Err(VmErr::Type("cannot set attribute on this type"))
        })
    }

    /* GetItem: container[index]. Routes through the VM's existing `get_item`
       so list / tuple / dict / str semantics are identical to script-side. */
    fn dispatch_get_item(recv_h: u32, args: &[Val]) -> Result<Val, VmErr> {
        if args.len() != 1 {
            return Err(VmErr::TypeMsg(s!("get_item expects 1 index, got ", int args.len() as i64)));
        }
        let idx = args[0];
        with_recv("edge_op get_item: invalid receiver handle", recv_h, |vm, recv| {
            let stack_before = vm.stack.len();
            vm.push(recv);
            vm.push(idx);
            let _ = vm.get_item()?;   // discard the bool (slice-path indicator).
            if vm.stack.len() != stack_before + 1 {
                return Err(VmErr::Runtime("edge_op get_item: get_item left no result"));
            }
            Ok(vm.stack.pop().unwrap())
        })
    }

    /* SetItem: container[index] = value. Routes through `store_item`. */
    fn dispatch_set_item(recv_h: u32, args: &[Val]) -> Result<Val, VmErr> {
        if args.len() != 2 {
            return Err(VmErr::TypeMsg(s!("set_item expects (index, value), got ", int args.len() as i64, " args")));
        }
        let idx = args[0];
        let value = args[1];
        with_recv("edge_op set_item: invalid receiver handle", recv_h, |vm, recv| {
            // store_item pops top→bottom as (value, idx, container), so
            // push in the matching order.
            vm.push(recv);
            vm.push(idx);
            vm.push(value);
            vm.store_item()?;
            Ok(Val::none())
        })
    }

    fn dispatch_len(recv_h: u32) -> Result<Val, VmErr> {
        with_recv("edge_op len: invalid receiver handle", recv_h, |vm, recv| {
            let n: i64 = match vm.heap.get(recv) {
                HeapObj::Str(s)    => s.chars().count() as i64,
                HeapObj::List(rc)  => rc.borrow().len() as i64,
                HeapObj::Dict(rc)  => rc.borrow().entries.len() as i64,
                HeapObj::Set(rc)   => rc.borrow().len() as i64,
                HeapObj::Tuple(t)  => t.len() as i64,
                _ => return Err(VmErr::TypeMsg(s!(
                    "object of type '", str vm.type_name(recv), "' has no len()"))),
            };
            Ok(Val::int(n))
        })
    }

    /* Iter: materialize the receiver into a List the guest can index via
       GetItem + Len. This is a flattening shortcut — the guest doesn't see
       the VM's iter_stack abstraction, so anything iterable becomes a
       random-access sequence. Range / Set / Dict.keys all flatten here. */
    fn dispatch_iter(recv_h: u32) -> Result<Val, VmErr> {
        with_recv("edge_op iter: invalid receiver handle", recv_h, |vm, recv| {
            let items: Vec<Val> = match vm.heap.get(recv) {
                HeapObj::List(rc)  => rc.borrow().clone(),
                HeapObj::Tuple(t)  => t.clone(),
                HeapObj::Set(rc)   => {
                    let mut v: Vec<Val> = rc.borrow().iter().copied().collect();
                    vm.sort_set_items(&mut v);
                    v
                }
                HeapObj::Dict(rc)  => rc.borrow().keys().collect(),
                HeapObj::Range(s, e, st) => {
                    let mut out = Vec::new();
                    let (mut cur, end, step) = (*s, *e, *st);
                    if step > 0 {
                        while cur < end { out.push(Val::int(cur)); cur += step; }
                    } else if step < 0 {
                        while cur > end { out.push(Val::int(cur)); cur += step; }
                    }
                    out
                }
                HeapObj::Str(s) => {
                    let chars: Vec<String> = s.chars().map(|c| c.to_string()).collect();
                    chars.into_iter()
                        .map(|cs| vm.heap.alloc(HeapObj::Str(cs)))
                        .collect::<Result<Vec<_>, _>>()?
                }
                _ => return Err(VmErr::TypeMsg(s!(
                    "object of type '", str vm.type_name(recv), "' is not iterable"))),
            };
            vm.heap.alloc(HeapObj::List(Rc::new(RefCell::new(items))))
        })
    }

    /* IterNext: convenience over Iter — guests typically use Iter + GetItem
       indexed by a counter. We expose IterNext for ergonomic Lua-like
       loops: it pops the head of a list-handle and returns it; on empty,
       returns a Raised("StopIteration"). */
    fn dispatch_iter_next(recv_h: u32) -> Result<Val, VmErr> {
        with_recv("edge_op iter_next: invalid receiver handle", recv_h, |vm, recv| {
            if let HeapObj::List(rc) = vm.heap.get(recv) {
                let mut v = rc.borrow_mut();
                if v.is_empty() {
                    return Err(VmErr::Raised(s!("StopIteration")));
                }
                Ok(v.remove(0))
            } else {
                Err(VmErr::TypeMsg(s!(
                    "iter_next expects a List iterator (produced by Op::Iter), got '",
                    str vm.type_name(recv), "'")))
            }
        })
    }

    /// Bootstrap encoder: wrap a primitive value in a fresh handle.
    /// The Val layout for None / Bool / Int / Float comes from
    /// `abi::classify_encode`; UTF-8 bytes are routed through the VM's
    /// heap as a `HeapObj::Str` so the handle can subsequently flow
    /// through `edge_op` like any other Edge Python value.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn host_edge_encode(tag: u32, ptr: *const u8, len: u32) -> u32 {
        let bytes = if len == 0 || ptr.is_null() {
            &[][..]
        } else {
            unsafe { core::slice::from_raw_parts(ptr, len as usize) }
        };
        match classify_encode(tag, bytes) {
            EncodeRequest::Direct(bits) => put_val(Val(bits)),
            EncodeRequest::AllocStr(s) => {
                let owned = s.to_string();
                let v = with_vm(|vm| vm.heap.alloc(HeapObj::Str(owned)).ok()).flatten();
                match v {
                    Some(val) => put_val(val),
                    None => 0,
                }
            }
            EncodeRequest::Invalid => 0,
        }
    }

    /// Bootstrap decoder: write a handle's tag at `*out_tag` and copy
    /// its bytes into the caller-provided buffer `dst[..dst_max]`. The
    /// classification (which tag, which bytes) is `abi::classify_decode`;
    /// only the Str heap-resident path drops back into the VM to read
    /// the actual UTF-8 bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn host_edge_decode(
        h: u32,
        out_tag: *mut u32,
        dst: *mut u8, dst_max: u32,
    ) -> i32 {
        let copy_into = |tag: u32, bytes: &[u8]| -> i32 {
            unsafe { *out_tag = tag; }
            if bytes.len() > dst_max as usize {
                return -(bytes.len() as i32);
            }
            if !bytes.is_empty() {
                unsafe {
                    core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
                }
            }
            bytes.len() as i32
        };

        let v = match get_val(h) {
            Some(v) => v,
            None => { unsafe { *out_tag = TAG_INVALID; } return 0; }
        };

        match classify_decode(v.0) {
            DecodeBits::Primitive { tag, bytes } => match bytes {
                PrimitiveBytes::None      => copy_into(tag, &[]),
                PrimitiveBytes::Bool(b)   => copy_into(tag, &[b]),
                PrimitiveBytes::Eight(a) => copy_into(tag, &a),
            },
            DecodeBits::Heap => {
                // Only Str decodes; composites must go through edge_op.
                let result = with_vm(|vm| {
                    if let HeapObj::Str(s) = vm.heap.get(v) {
                        Some(s.clone())
                    } else { None }
                }).flatten();
                match result {
                    Some(s) => copy_into(crate::abi::Tag::Bytes as u32, s.as_bytes()),
                    None => { unsafe { *out_tag = TAG_INVALID; } 0 }
                }
            }
            DecodeBits::Invalid => { unsafe { *out_tag = TAG_INVALID; } 0 }
        }
    }

    /// Decrement refcount on a handle. No-op for invalid handles.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn host_edge_release(h: u32) {
        handles().release(h);
    }

    /// Stash an error from the guest so the host sees it after the
    /// guest returns 1. Overwrites any pending error.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn host_edge_throw(kind: u32, msg_ptr: *const u8, msg_len: u32) {
        let msg = if msg_len == 0 { String::new() } else {
            core::str::from_utf8(unsafe {
                core::slice::from_raw_parts(msg_ptr, msg_len as usize)
            }).unwrap_or("").to_string()
        };
        error_stash().set(kind, msg);
    }

    /// Drain the most recent error. Writes the kind to `*out_kind` and
    /// copies the UTF-8 message into `dst[..dst_max]`. Returns the number
    /// of message bytes copied (>=0), `-bytes_needed` if dst_max was too
    /// small (the error stays pending — retry with a bigger buffer), or
    /// `-1` if no error was pending.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn host_edge_take_error(
        out_kind: *mut u32,
        dst: *mut u8, dst_max: u32,
    ) -> i32 {
        // Peek first so a buffer-too-small caller can retry without
        // having lost the error.
        let stash = error_stash();
        let (kind, len) = match stash.peek() {
            Some((k, m)) => (k, m.len()),
            None => return -1,
        };
        if len > dst_max as usize { return -(len as i32); }
        // Buffer fits — drain and copy.
        let (_, msg) = stash.take().expect("peek returned Some");
        let bytes = msg.as_bytes();
        unsafe {
            *out_kind = kind;
            if !bytes.is_empty() {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
            }
        }
        bytes.len() as i32
    }
}
