/*
Built-in methods (str/list/dict). The `define_methods!` macro generates the enum, lookup, and dispatcher from one table — adding a method is one row.
*/

use super::*;
use super::methods_helpers::*;
use crate::alloc::string::ToString;
use crate::s;

// `resolve_attr` result — every shape LoadAttr / CallMethod dispatches on.
pub(crate) enum AttrLookup {
    ModuleAttr(Val),
    ClassMember(Val),
    InstanceField(Val),
    // `class` is where `func` was found; the called frame needs it so `super()` knows where to resume.
    InstanceMethod { recv: Val, func: Val, class: Val },
    BuiltinMethod(BuiltinMethodId),
    // `e.args` on ExcInstance — caller picks: LoadAttr materialises the tuple, CallMethod errors.
    ExcArgs(Vec<Val>),
}

impl<'a> VM<'a> {
    // Direct-then-DFS member lookup; first hit wins. Cycles are impossible: bases are validated at `MakeClass` time and `HeapObj::Class` is immutable, so the class graph is a static DAG.
    // Returns `(value, defining_class)` so callers building `BoundUserMethod` / `InstanceMethod` can record where the method came from for `super()`.
    pub(crate) fn lookup_class_member(&self, cls: Val, name: &str) -> Option<(Val, Val)> {
        if !cls.is_heap() { return None; }
        let HeapObj::Class(_, bases, members) = self.heap.get(cls) else { return None; };
        if let Some((_, v)) = members.iter().find(|(n, _)| n == name) { return Some((*v, cls)); }
        for &b in bases {
            if let Some(found) = self.lookup_class_member(b, name) { return Some(found); }
        }
        None
    }

    // Same lookup but skipping `cls` itself; powers `super()` which must search strictly above the current class.
    pub(crate) fn lookup_class_member_after(&self, cls: Val, name: &str) -> Option<(Val, Val)> {
        if !cls.is_heap() { return None; }
        let HeapObj::Class(_, bases, _) = self.heap.get(cls) else { return None; };
        for &b in bases {
            if let Some(found) = self.lookup_class_member(b, name) { return Some(found); }
        }
        None
    }

    // `obj.<name>` resolution shared by `handle_load_attr` and `exec_call_method`.
    pub(crate) fn resolve_attr(&self, obj: Val, name: &str) -> Result<AttrLookup, VmErr> {
        let bare = crate::modules::parser::ssa_strip(name);

        // Module attr: linear scan; the table is sized for around 30 entries.
        if obj.is_heap()
            && let HeapObj::Module(mod_name, attrs) = self.heap.get(obj) {
                if let Some((_, v)) = attrs.iter().find(|(n, _)| n == bare) {
                    return Ok(AttrLookup::ModuleAttr(*v));
                }
                return Err(VmErr::Attribute(s!("module '", str mod_name, "' has no attribute '", str bare, "'")));
            }

        // ExcInstance attr: only `e.args` is defined.
        if obj.is_heap()
            && let HeapObj::ExcInstance(_, args) = self.heap.get(obj) {
                if bare == "args" { return Ok(AttrLookup::ExcArgs(args.clone())); }
                let ty = self.type_name(obj);
                return Err(VmErr::Attribute(s!("'", str ty, "' object has no attribute '", str bare, "'")));
            }

        // Class attr: `MyClass.method` returns the unbound function (no `self` prepended).
        if obj.is_heap()
            && let HeapObj::Class(cls_name, _, _) = self.heap.get(obj) {
                if let Some((v, _)) = self.lookup_class_member(obj, bare) { return Ok(AttrLookup::ClassMember(v)); }
                let cls_name = cls_name.clone();
                return Err(VmErr::Attribute(s!("type object '", str &cls_name, "' has no attribute '", str bare, "'")));
            }

        // Instance attribute lookup: check `__dict__` first, then the class chain (direct + bases).
        if obj.is_heap()
            && let HeapObj::Instance(cls_val, attrs) = self.heap.get(obj) {
                let cls_val = *cls_val;
                let found = attrs.borrow().entries.iter()
                    .find(|(k, _)| k.is_heap() && matches!(self.heap.get(*k), HeapObj::Str(s) if s == name))
                    .map(|(_, v)| *v);
                if let Some(v) = found { return Ok(AttrLookup::InstanceField(v)); }
                if let Some((mv, defining)) = self.lookup_class_member(cls_val, name) {
                    return Ok(AttrLookup::InstanceMethod { recv: obj, func: mv, class: defining });
                }
                let ty = self.type_name(obj);
                return Err(VmErr::Attribute(s!("'", str ty, "' object has no attribute '", str name, "'")));
            }

        // `super().<name>`: search strictly above the proxy's stored class; methods bind to the proxy's `recv`.
        if obj.is_heap()
            && let HeapObj::Super(cls_val, recv) = self.heap.get(obj) {
                let (cls_val, recv) = (*cls_val, *recv);
                if let Some((mv, defining)) = self.lookup_class_member_after(cls_val, name) {
                    return Ok(AttrLookup::InstanceMethod { recv, func: mv, class: defining });
                }
                return Err(VmErr::Attribute(s!("'super' object has no attribute '", str name, "'")));
            }

        // Builtin type method.
        let ty = self.type_name(obj);
        lookup_method(ty, name)
            .map(AttrLookup::BuiltinMethod)
            .ok_or_else(|| VmErr::Attribute(s!("'", str ty, "' object has no attribute '", str name, "'")))
    }

    // `resolve_attr` that swallows `AttributeError` into `None`; other VmErrs still propagate — dunder probes need a miss to be silent.
    pub(crate) fn resolve_attr_silent(&self, obj: Val, name: &str) -> Result<Option<AttrLookup>, VmErr> {
        match self.resolve_attr(obj, name) {
            Ok(lookup) => Ok(Some(lookup)),
            Err(VmErr::Attribute(_)) => Ok(None),
            Err(other) => Err(other),
        }
    }

    /* F2.10: instance fallback via `__getattr__(name)`. Called by `LoadAttr` / `CallMethod` after the normal lookup raises `AttributeError`. */
    pub(crate) fn try_getattr_fallback(&mut self, obj: Val, name: &str, chunk: &SSAChunk, slots: &mut [Val]) -> Result<Option<Val>, VmErr> {
        if !obj.is_heap() || !matches!(self.heap.get(obj), HeapObj::Instance(..)) { return Ok(None); }
        let bare = crate::modules::parser::ssa_strip(name);
        let name_val = self.heap.alloc(HeapObj::Str(bare.to_string()))?;
        self.try_call_dunder(obj, "__getattr__", &[name_val], chunk, slots)
    }

    pub(crate) fn handle_load_attr(&mut self, name_idx: u16, chunk: &SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        let name = chunk.names.get(name_idx as usize).ok_or(VmErr::Runtime("LoadAttr: bad name index"))?.clone();
        let obj = self.pop()?;
        let lookup = match self.resolve_attr(obj, &name) {
            Ok(l) => l,
            Err(VmErr::Attribute(msg)) => {
                if let Some(v) = self.try_getattr_fallback(obj, &name, chunk, slots)? {
                    self.push(v);
                    return Ok(());
                }
                return Err(VmErr::Attribute(msg));
            }
            Err(other) => return Err(other),
        };
        match lookup {
            AttrLookup::ModuleAttr(v)
            | AttrLookup::ClassMember(v)
            | AttrLookup::InstanceField(v) => {
                self.push(v);
                Ok(())
            }
            AttrLookup::InstanceMethod { recv, func, class } => {
                let bound = self.heap.alloc(HeapObj::BoundUserMethod(recv, func, class))?;
                self.push(bound);
                Ok(())
            }
            AttrLookup::BuiltinMethod(id) => {
                let bound = self.heap.alloc(HeapObj::BoundMethod(obj, id))?;
                self.push(bound);
                Ok(())
            }
            AttrLookup::ExcArgs(args) => {
                let v = self.heap.alloc(HeapObj::Tuple(args))?;
                self.push(v);
                Ok(())
            }
        }
    }
}

// Row: (Variant, "name", category, body). `mutating` auto-emits mark_impure; variant prefix picks the receiver.
macro_rules! define_methods {
    ( $( ($variant:ident, $name:literal, $cat:ident, |$vm:ident, $recv:ident, $pos:ident| $body:block) ),* $(,)? ) => {

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u8)]
        pub enum BuiltinMethodId {
            $( $variant ),*
        }

        impl BuiltinMethodId {
            #[inline]
            pub fn name(self) -> &'static str {
                match self { $( Self::$variant => $name ),* }
            }
        }

        pub(crate) fn dispatch_method(vm: &mut VM, id: BuiltinMethodId, recv: Val, pos: &[Val], kw: &[Val]) -> Result<(), VmErr> {
            if !kw.is_empty() {
                return Err(cold_type("builtin method takes no keyword arguments"));
            }
            match id {
                $(
                    BuiltinMethodId::$variant => {
                        let $vm = vm; let $recv = recv; let $pos = pos;
                        let result: Result<(), VmErr> = (|| $body)();
                        define_methods!(@maybe_impure $cat, $vm, result)
                    }
                ),*
            }
        }

        // Off the hot path — CallMethod fusion bypasses LoadAttr+Call entirely.
        pub fn lookup_method(ty: &str, attr: &str) -> Option<BuiltinMethodId> {
            let prefix = match ty {
                "str" => "Str",
                "bytes" => "Bytes",
                "list" => "List",
                "dict" => "Dict",
                "set" => "Set",
                _ => return None,
            };
            $(
                if attr == $name && stringify!($variant).starts_with(prefix) {
                    return Some(BuiltinMethodId::$variant);
                }
            )*
            None
        }
    };

    (@maybe_impure mutating, $vm:ident, $r:ident) => {{
        if $r.is_ok() { $vm.mark_impure(); }
        $r
    }};
    (@maybe_impure pure, $vm:ident, $r:ident) => { $r };
}

define_methods! {
    // `str.encode([encoding])` — UTF-8/ASCII only; other names error to block silent mismatches.
    (StrEncode, "encode", pure, |vm, recv, pos| {
        check_arity(pos, 0, 1, "encode takes 0 or 1 arguments")?;
        let s = recv_str(vm, recv)?;
        if let Some(arg) = pos.first() {
            let enc = val_to_str(vm, *arg)?;
            match enc.as_str() {
                "utf-8" | "utf8" => {}
                "ascii" if !s.is_ascii() => {
                    return Err(cold_value("'ascii' codec can't encode non-ASCII characters"));
                }
                "ascii" => {}
                _ => return Err(cold_value("unsupported encoding (expected 'utf-8' or 'ascii')")),
            }
        }
        let v = vm.heap.alloc(HeapObj::Bytes(s.into_bytes()))?;
        vm.push(v); Ok(())
    }),

    // `bytes.decode([encoding])` — invalid UTF-8 errors as ValueError.
    (BytesDecode, "decode", pure, |vm, recv, pos| {
        check_arity(pos, 0, 1, "decode takes 0 or 1 arguments")?;
        let buf = recv_bytes(vm, recv)?;
        if let Some(arg) = pos.first() {
            let enc = val_to_str(vm, *arg)?;
            if !matches!(enc.as_str(), "utf-8" | "utf8" | "ascii") {
                return Err(cold_value("unsupported encoding (expected 'utf-8' or 'ascii')"));
            }
        }
        let text = alloc::string::String::from_utf8(buf)
            .map_err(|_| cold_value("invalid UTF-8 in bytes.decode()"))?;
        let v = vm.heap.alloc(HeapObj::Str(text))?;
        vm.push(v); Ok(())
    }),

    // `bytes.hex()` — lowercase hex of every byte. No separator.
    (BytesHex, "hex", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "hex takes no arguments")?;
        let buf = recv_bytes(vm, recv)?;
        let mut out = alloc::string::String::with_capacity(buf.len() * 2);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for &b in &buf {
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0F) as usize] as char);
        }
        let v = vm.heap.alloc(HeapObj::Str(out))?;
        vm.push(v); Ok(())
    }),

    // `bytes.startswith` / `bytes.endswith` — bytes-only; strings go through `str.startswith`.
    (BytesStartswith, "startswith", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "startswith takes 1 argument")?;
        let buf = recv_bytes(vm, recv)?;
        let prefix = recv_bytes(vm, pos[0])?;
        vm.push(Val::bool(buf.starts_with(&prefix)));
        Ok(())
    }),
    (BytesEndswith, "endswith", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "endswith takes 1 argument")?;
        let buf = recv_bytes(vm, recv)?;
        let suffix = recv_bytes(vm, pos[0])?;
        vm.push(Val::bool(buf.ends_with(&suffix)));
        Ok(())
    }),

    // str: zero-arg transforms.
    (StrUpper, "upper", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "upper takes no arguments")?;
        let s = recv_str(vm, recv)?;
        let v = vm.heap.alloc(HeapObj::Str(s.to_uppercase()))?;
        vm.push(v); Ok(())
    }),
    (StrLower, "lower", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "lower takes no arguments")?;
        let s = recv_str(vm, recv)?;
        let v = vm.heap.alloc(HeapObj::Str(s.to_lowercase()))?;
        vm.push(v); Ok(())
    }),
    (StrStrip, "strip", pure, |vm, recv, pos| {
        check_arity(pos, 0, 1, "strip takes 0 or 1 arguments")?;
        let s = recv_str(vm, recv)?;
        let out = if pos.is_empty() {
            s.trim().to_string()
        } else {
            let p = val_to_str(vm, pos[0])?;
            s.trim_matches(|c| p.contains(c)).to_string()
        };
        let v = vm.heap.alloc(HeapObj::Str(out))?;
        vm.push(v); Ok(())
    }),
    (StrCapitalize, "capitalize", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "capitalize takes no arguments")?;
        let s = recv_str(vm, recv)?;
        let v = vm.heap.alloc(HeapObj::Str(capitalize_first(&s)))?;
        vm.push(v); Ok(())
    }),
    (StrTitle, "title", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "title takes no arguments")?;
        let s = recv_str(vm, recv)?;
        let v = vm.heap.alloc(HeapObj::Str(title_case(&s)))?;
        vm.push(v); Ok(())
    }),

    // str: optional separator.
    (StrLstrip, "lstrip", pure, |vm, recv, pos| {
        check_arity(pos, 0, 1, "lstrip takes 0 or 1 arguments")?;
        let s = recv_str(vm, recv)?;
        let out = if pos.is_empty() {
            s.trim_start().to_string()
        } else {
            let p = val_to_str(vm, pos[0])?;
            s.trim_start_matches(|c| p.contains(c)).to_string()
        };
        let v = vm.heap.alloc(HeapObj::Str(out))?;
        vm.push(v); Ok(())
    }),
    (StrRstrip, "rstrip", pure, |vm, recv, pos| {
        check_arity(pos, 0, 1, "rstrip takes 0 or 1 arguments")?;
        let s = recv_str(vm, recv)?;
        let out = if pos.is_empty() {
            s.trim_end().to_string()
        } else {
            let p = val_to_str(vm, pos[0])?;
            s.trim_end_matches(|c| p.contains(c)).to_string()
        };
        let v = vm.heap.alloc(HeapObj::Str(out))?;
        vm.push(v); Ok(())
    }),

    // str: predicates.
    (StrIsDigit, "isdigit", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "isdigit takes no arguments")?;
        let s = recv_str(vm, recv)?;
        vm.push(Val::bool(!s.is_empty() && s.chars().all(|c| c.is_ascii_digit())));
        Ok(())
    }),
    (StrIsAlpha, "isalpha", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "isalpha takes no arguments")?;
        let s = recv_str(vm, recv)?;
        vm.push(Val::bool(!s.is_empty() && s.chars().all(|c| c.is_alphabetic())));
        Ok(())
    }),
    (StrIsAlnum, "isalnum", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "isalnum takes no arguments")?;
        let s = recv_str(vm, recv)?;
        vm.push(Val::bool(!s.is_empty() && s.chars().all(|c| c.is_alphanumeric())));
        Ok(())
    }),

    // str: queries with one string arg.
    (StrStartswith, "startswith", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "startswith takes 1 argument")?;
        let s = recv_str(vm, recv)?;
        let p = val_to_str(vm, pos[0])?;
        vm.push(Val::bool(s.starts_with(p.as_str())));
        Ok(())
    }),
    (StrEndswith, "endswith", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "endswith takes 1 argument")?;
        let s = recv_str(vm, recv)?;
        let p = val_to_str(vm, pos[0])?;
        vm.push(Val::bool(s.ends_with(p.as_str())));
        Ok(())
    }),
    (StrFind, "find", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "find takes 1 argument")?;
        let s = recv_str(vm, recv)?;
        let sub = val_to_str(vm, pos[0])?;
        let idx = s.find(sub.as_str())
            .map(|i| s[..i].chars().count() as i64)
            .unwrap_or(-1);
        vm.push(Val::int(idx));
        Ok(())
    }),
    (StrCount, "count", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "count takes 1 argument")?;
        let s = recv_str(vm, recv)?;
        let sub = val_to_str(vm, pos[0])?;
        vm.push(Val::int(s.matches(sub.as_str()).count() as i64));
        Ok(())
    }),

    // str: split / join / replace.
    (StrSplit, "split", pure, |vm, recv, pos| {
        check_arity(pos, 0, 1, "split takes 0 or 1 arguments")?;
        let s = recv_str(vm, recv)?;
        let parts: Vec<Val> = if pos.is_empty() {
            s.split_whitespace()
                .map(|p| vm.heap.alloc(HeapObj::Str(p.to_string())))
                .collect::<Result<_, _>>()?
        } else {
            let sep = val_to_str(vm, pos[0])?;
            s.split(sep.as_str())
                .map(|p| vm.heap.alloc(HeapObj::Str(p.to_string())))
                .collect::<Result<_, _>>()?
        };
        vm.alloc_and_push_list(parts)
    }),
    (StrJoin, "join", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "join takes 1 argument")?;
        let sep = recv_str(vm, recv)?;
        let items = match vm.heap.get(pos[0]) {
            HeapObj::List(rc) => rc.borrow().clone(),
            HeapObj::Tuple(v) => v.clone(),
            _ => return Err(cold_type("join() argument must be iterable")),
        };
        let mut parts: Vec<String> = Vec::with_capacity(items.len());
        for v in items { parts.push(val_to_str(vm, v)?); }
        let v = vm.heap.alloc(HeapObj::Str(parts.join(sep.as_str())))?;
        vm.push(v); Ok(())
    }),
    (StrReplace, "replace", pure, |vm, recv, pos| {
        check_arity(pos, 2, 2, "replace takes 2 arguments")?;
        let s = recv_str(vm, recv)?;
        let old = val_to_str(vm, pos[0])?;
        let new = val_to_str(vm, pos[1])?;
        let v = vm.heap.alloc(HeapObj::Str(s.replace(old.as_str(), new.as_str())))?;
        vm.push(v); Ok(())
    }),

    // `str.removeprefix` / `removesuffix` — strip if present, else return unchanged.
    (StrRemovePrefix, "removeprefix", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "removeprefix takes 1 argument")?;
        let s = recv_str(vm, recv)?;
        let p = val_to_str(vm, pos[0])?;
        let out = s.strip_prefix(p.as_str()).map(|t| t.to_string()).unwrap_or(s);
        let v = vm.heap.alloc(HeapObj::Str(out))?;
        vm.push(v); Ok(())
    }),
    (StrRemoveSuffix, "removesuffix", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "removesuffix takes 1 argument")?;
        let s = recv_str(vm, recv)?;
        let suf = val_to_str(vm, pos[0])?;
        let out = s.strip_suffix(suf.as_str()).map(|t| t.to_string()).unwrap_or(s);
        let v = vm.heap.alloc(HeapObj::Str(out))?;
        vm.push(v); Ok(())
    }),

    // `str.splitlines()` — split on \n / \r / \r\n, dropping the separator (keepends=False).
    (StrSplitlines, "splitlines", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "splitlines takes no arguments")?;
        let s = recv_str(vm, recv)?;
        let mut parts: Vec<Val> = Vec::new();
        for line in s.split_inclusive(['\n', '\r']) {
            let trimmed = line.trim_end_matches(['\n', '\r']).to_string();
            parts.push(vm.heap.alloc(HeapObj::Str(trimmed))?);
        }
        // Drop the trailing empty segment that split_inclusive leaves when the input ends in a separator.
        if let Some(last) = parts.last()
            && let HeapObj::Str(t) = vm.heap.get(*last)
            && t.is_empty() && s.ends_with(['\n', '\r']) {
                parts.pop();
            }
        vm.alloc_and_push_list(parts)
    }),

    // `str.partition` / `rpartition` — (head, sep, tail); on miss returns (s,"","") / ("","",s).
    (StrPartition, "partition", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "partition takes 1 argument")?;
        let s = recv_str(vm, recv)?;
        let sep = val_to_str(vm, pos[0])?;
        if sep.is_empty() { return Err(cold_value("empty separator")); }
        let (a, b, c): (String, String, String) = match s.find(sep.as_str()) {
            Some(i) => (s[..i].to_string(), sep.clone(), s[i + sep.len()..].to_string()),
            None => (s, String::new(), String::new()),
        };
        let av = vm.heap.alloc(HeapObj::Str(a))?;
        let bv = vm.heap.alloc(HeapObj::Str(b))?;
        let cv = vm.heap.alloc(HeapObj::Str(c))?;
        vm.alloc_and_push_tuple(vec![av, bv, cv])
    }),
    (StrRPartition, "rpartition", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "rpartition takes 1 argument")?;
        let s = recv_str(vm, recv)?;
        let sep = val_to_str(vm, pos[0])?;
        if sep.is_empty() { return Err(cold_value("empty separator")); }
        let (a, b, c): (String, String, String) = match s.rfind(sep.as_str()) {
            Some(i) => (s[..i].to_string(), sep.clone(), s[i + sep.len()..].to_string()),
            None => (String::new(), String::new(), s),
        };
        let av = vm.heap.alloc(HeapObj::Str(a))?;
        let bv = vm.heap.alloc(HeapObj::Str(b))?;
        let cv = vm.heap.alloc(HeapObj::Str(c))?;
        vm.alloc_and_push_tuple(vec![av, bv, cv])
    }),

    // `bytes.find` / index / count / replace — byte-oriented analogs of str methods.
    (BytesFind, "find", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "find takes 1 argument")?;
        let buf = recv_bytes(vm, recv)?;
        let sub = recv_bytes(vm, pos[0])?;
        let idx = buf.windows(sub.len()).position(|w| w == sub.as_slice())
            .map(|i| i as i64).unwrap_or(-1);
        vm.push(Val::int(idx));
        Ok(())
    }),
    (BytesIndex, "index", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "index takes 1 argument")?;
        let buf = recv_bytes(vm, recv)?;
        let sub = recv_bytes(vm, pos[0])?;
        let idx = buf.windows(sub.len()).position(|w| w == sub.as_slice()).ok_or(cold_value("subsection not found"))?;
        vm.push(Val::int(idx as i64));
        Ok(())
    }),
    (BytesCount, "count", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "count takes 1 argument")?;
        let buf = recv_bytes(vm, recv)?;
        let sub = recv_bytes(vm, pos[0])?;
        if sub.is_empty() {
            vm.push(Val::int(buf.len() as i64 + 1));
            return Ok(());
        }
        let mut n = 0i64;
        let mut i = 0usize;
        while i + sub.len() <= buf.len() {
            if buf[i..i + sub.len()] == sub[..] { n += 1; i += sub.len(); }
            else { i += 1; }
        }
        vm.push(Val::int(n));
        Ok(())
    }),
    (BytesReplace, "replace", pure, |vm, recv, pos| {
        check_arity(pos, 2, 2, "replace takes 2 arguments")?;
        let buf = recv_bytes(vm, recv)?;
        let old = recv_bytes(vm, pos[0])?;
        let new = recv_bytes(vm, pos[1])?;
        if old.is_empty() {
            let v = vm.heap.alloc(HeapObj::Bytes(buf))?;
            vm.push(v); return Ok(());
        }
        let mut out: Vec<u8> = Vec::with_capacity(buf.len());
        let mut i = 0usize;
        while i < buf.len() {
            if i + old.len() <= buf.len() && buf[i..i + old.len()] == old[..] {
                out.extend_from_slice(&new); i += old.len();
            } else {
                out.push(buf[i]); i += 1;
            }
        }
        let v = vm.heap.alloc(HeapObj::Bytes(out))?;
        vm.push(v); Ok(())
    }),
    (BytesSplit, "split", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "split takes 1 argument")?;
        let buf = recv_bytes(vm, recv)?;
        let sep = recv_bytes(vm, pos[0])?;
        if sep.is_empty() { return Err(cold_value("empty separator")); }
        let mut parts: Vec<Val> = Vec::new();
        let mut start = 0usize;
        let mut i = 0usize;
        while i + sep.len() <= buf.len() {
            if buf[i..i + sep.len()] == sep[..] {
                parts.push(vm.heap.alloc(HeapObj::Bytes(buf[start..i].to_vec()))?);
                i += sep.len(); start = i;
            } else { i += 1; }
        }
        parts.push(vm.heap.alloc(HeapObj::Bytes(buf[start..].to_vec()))?);
        vm.alloc_and_push_list(parts)
    }),

    // str: padding.
    (StrCenter, "center", pure, |vm, recv, pos| {
        check_arity(pos, 1, 2, "center takes 1 or 2 arguments")?;
        let s = recv_str(vm, recv)?;
        if !pos[0].is_int() { return Err(cold_type("center() width must be an integer")); }
        let width = pos[0].as_int() as usize;
        let fill = if pos.len() > 1 {
            val_to_str(vm, pos[1])?.chars().next().unwrap_or(' ')
        } else { ' ' };
        // Padding measured in code points, not UTF-8 bytes (Unicode parity).
        let pad = width.saturating_sub(s.chars().count());
        let left = pad / 2;
        let right = pad - left;
        let out = fill.to_string().repeat(left) + &s + &fill.to_string().repeat(right);
        let v = vm.heap.alloc(HeapObj::Str(out))?;
        vm.push(v); Ok(())
    }),
    (StrZfill, "zfill", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "zfill takes 1 argument")?;
        if !pos[0].is_int() { return Err(cold_type("zfill() requires an integer argument")); }
        let s = recv_str(vm, recv)?;
        let width = pos[0].as_int() as usize;
        let nchars = s.chars().count();
        let out = if nchars >= width {
            s
        } else {
            let pad = "0".repeat(width - nchars);
            if s.starts_with('+') || s.starts_with('-') {
                s[..1].to_string() + &pad + &s[1..]
            } else {
                pad + &s
            }
        };
        let v = vm.heap.alloc(HeapObj::Str(out))?;
        vm.push(v); Ok(())
    }),

    // list: pure.
    (ListIndex, "index", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "index takes 1 argument")?;
        let items = list_clone(vm, recv)?;
        let idx = items.iter()
            .position(|&v| eq_vals_with_heap(v, pos[0], &vm.heap))
            .map(|i| i as i64)
            .ok_or(cold_value("value not found in list"))?;
        vm.push(Val::int(idx));
        Ok(())
    }),
    (ListCount, "count", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "count takes 1 argument")?;
        let items = list_clone(vm, recv)?;
        let n = items.iter().filter(|&&v| eq_vals_with_heap(v, pos[0], &vm.heap)).count() as i64;
        vm.push(Val::int(n));
        Ok(())
    }),
    (ListCopy, "copy", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "copy takes no arguments")?;
        let items = list_clone(vm, recv)?;
        vm.alloc_and_push_list(items)
    }),

    // list: mutating.
    (ListAppend, "append", mutating, |vm, recv, pos| {
        check_arity(pos, 1, 1, "append takes 1 argument")?;
        list_mut(vm, recv, "append: receiver is not a list", |list| {
            list.push(pos[0]); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (ListClear, "clear", mutating, |vm, recv, pos| {
        check_arity(pos, 0, 0, "clear takes no arguments")?;
        list_mut(vm, recv, "clear: receiver is not a list", |list| {
            list.clear(); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (ListReverse, "reverse", mutating, |vm, recv, pos| {
        check_arity(pos, 0, 0, "reverse takes no arguments")?;
        list_mut(vm, recv, "reverse: receiver is not a list", |list| {
            list.reverse(); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (ListExtend, "extend", mutating, |vm, recv, pos| {
        check_arity(pos, 1, 1, "extend takes 1 argument")?;
        let items = vm.extract_iter(pos[0], true)?;
        list_mut(vm, recv, "extend: receiver is not a list", |list| {
            list.extend_from_slice(&items); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (ListInsert, "insert", mutating, |vm, recv, pos| {
        check_arity(pos, 2, 2, "insert takes 2 arguments")?;
        if !pos[0].is_int() { return Err(cold_type("list indices must be integers")); }
        list_mut(vm, recv, "insert: receiver is not a list", |list| {
            let i = pos[0].as_int();
            let ui = if i < 0 {
                (list.len() as i64 + i).max(0) as usize
            } else {
                (i as usize).min(list.len())
            };
            list.insert(ui, pos[1]);
            Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (ListRemove, "remove", mutating, |vm, recv, pos| {
        check_arity(pos, 1, 1, "remove takes 1 argument")?;
        let items = list_clone(vm, recv)?;
        let idx = items.iter()
            .position(|&v| eq_vals_with_heap(v, pos[0], &vm.heap))
            .ok_or(cold_value("list.remove: value not found"))?;
        list_mut(vm, recv, "remove: receiver is not a list", |list| {
            list.remove(idx); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (ListPop, "pop", mutating, |vm, recv, pos| {
        check_arity(pos, 0, 1, "pop takes 0 or 1 arguments")?;
        let popped = list_mut(vm, recv, "pop: receiver is not a list", |list| {
            if list.is_empty() { return Err(cold_value("pop from empty list")); }
            if pos.is_empty() { return Ok(list.pop().unwrap()); }
            if !pos[0].is_int() { return Err(cold_type("list indices must be integers")); }
            let i = pos[0].as_int();
            let ui = if i < 0 { (list.len() as i64 + i) as usize } else { i as usize };
            if ui >= list.len() { return Err(cold_value("pop index out of range")); }
            Ok(list.remove(ui))
        })?;
        vm.push(popped); Ok(())
    }),
    (ListSort, "sort", mutating, |vm, recv, pos| {
        check_arity(pos, 0, 0, "sort takes no arguments")?;
        let mut sorted = list_clone(vm, recv)?;
        vm.sort_by_lt(&mut sorted)?;
        list_mut(vm, recv, "sort: receiver is not a list", |list| {
            *list = sorted; Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),

    // dict.
    (DictKeys, "keys", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "keys takes no arguments")?;
        let entries = dict_entries(vm, recv)?;
        let keys: Vec<Val> = entries.into_iter().map(|(k, _)| k).collect();
        vm.alloc_and_push_list(keys)
    }),
    (DictValues, "values", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "values takes no arguments")?;
        let entries = dict_entries(vm, recv)?;
        let vals: Vec<Val> = entries.into_iter().map(|(_, v)| v).collect();
        vm.alloc_and_push_list(vals)
    }),
    (DictItems, "items", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "items takes no arguments")?;
        let entries = dict_entries(vm, recv)?;
        let mut items: Vec<Val> = Vec::with_capacity(entries.len());
        for (k, vv) in entries {
            let t = vm.heap.alloc(HeapObj::Tuple(vec![k, vv]))?;
            items.push(t);
        }
        vm.alloc_and_push_list(items)
    }),
    // `dict.copy()` — shallow copy; mutations don't affect the original.
    (DictCopy, "copy", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "copy takes no arguments")?;
        let entries = dict_entries(vm, recv)?;
        let mut dm = DictMap::with_capacity(entries.len());
        for (k, v) in entries { dm.insert(k, v); }
        vm.alloc_and_push_dict(dm)
    }),
    // `dict.popitem()` — pop the last (k, v); KeyError on empty dict.
    (DictPopItem, "popitem", mutating, |vm, recv, pos| {
        check_arity(pos, 0, 0, "popitem takes no arguments")?;
        let pair = dict_mut(vm, recv, "popitem: receiver is not a dict", |dict| {
            let (k, v) = dict.entries.last().copied().ok_or(cold_value("popitem(): dictionary is empty"))?;
            dict.remove(&k);
            Ok((k, v))
        })?;
        vm.alloc_and_push_tuple(vec![pair.0, pair.1])
    }),
    (DictGet, "get", pure, |vm, recv, pos| {
        check_arity(pos, 1, 2, "get takes 1 or 2 arguments")?;
        let default = if pos.len() == 2 { pos[1] } else { Val::none() };
        let result = match vm.heap.get(recv) {
            HeapObj::Dict(rc) => rc.borrow().get(&pos[0]).copied().unwrap_or(default),
            _ => return Err(cold_type("get: receiver is not a dict")),
        };
        vm.push(result); Ok(())
    }),
    (DictUpdate, "update", mutating, |vm, recv, pos| {
        check_arity(pos, 1, 1, "update takes 1 argument")?;
        // Accept a dict or an iterable of 2-element pairs.
        let pairs: Vec<(Val, Val)> = if let HeapObj::Dict(rc) = vm.heap.get(pos[0]) {
            rc.borrow().entries.clone()
        } else {
            let items = vm.extract_iter(pos[0], true)?;
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                let pair = match vm.heap.get(it) {
                    HeapObj::Tuple(v) if v.len() == 2 => (v[0], v[1]),
                    HeapObj::List(v) if v.borrow().len() == 2 => { let v = v.borrow(); (v[0], v[1]) }
                    _ => return Err(cold_value("dictionary update sequence element must have length 2")),
                };
                out.push(pair);
            }
            out
        };
        dict_mut(vm, recv, "update: receiver is not a dict", |dict| {
            for (k, v) in pairs { dict.insert(k, v); }
            Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (DictPop, "pop", mutating, |vm, recv, pos| {
        check_arity(pos, 1, 2, "pop takes 1 or 2 arguments")?;
        let default = if pos.len() == 2 { Some(pos[1]) } else { None };
        let result = dict_mut(vm, recv, "pop: receiver is not a dict", |dict| {
            match dict.remove(&pos[0]) {
                Some(val) => Ok(val),
                None => default.ok_or(cold_value("key not found")),
            }
        })?;
        vm.push(result); Ok(())
    }),
    (DictSetDefault, "setdefault", mutating, |vm, recv, pos| {
        check_arity(pos, 1, 2, "setdefault takes 1 or 2 arguments")?;
        let default = if pos.len() > 1 { pos[1] } else { Val::none() };
        let result = dict_mut(vm, recv, "setdefault: receiver is not a dict", |dict| {
            if let Some(v) = dict.get(&pos[0]).copied() { Ok(v) }
            else { dict.insert(pos[0], default); Ok(default) }
        })?;
        vm.push(result); Ok(())
    }),

    // set: mutating.
    (SetAdd, "add", mutating, |vm, recv, pos| {
        check_arity(pos, 1, 1, "add takes 1 argument")?;
        set_mut(vm, recv, "add: receiver is not a set", |set| {
            set.insert(pos[0]); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (SetRemove, "remove", mutating, |vm, recv, pos| {
        check_arity(pos, 1, 1, "remove takes 1 argument")?;
        set_mut(vm, recv, "remove: receiver is not a set", |set| {
            // CPython: KeyError, not ValueError.
            if !set.remove(&pos[0]) { return Err(VmErr::Raised("KeyError".into())); }
            Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (SetDiscard, "discard", mutating, |vm, recv, pos| {
        check_arity(pos, 1, 1, "discard takes 1 argument")?;
        set_mut(vm, recv, "discard: receiver is not a set", |set| {
            set.remove(&pos[0]); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (SetPop, "pop", mutating, |vm, recv, pos| {
        check_arity(pos, 0, 0, "pop takes no arguments")?;
        let popped = set_mut(vm, recv, "pop: receiver is not a set", |set| {
            // HashSet has no pop() — grab via `iter()` and remove. Empty set raises.
            let pick = set.iter().next().copied()
                .ok_or(cold_value("pop from an empty set"))?;
            set.remove(&pick);
            Ok(pick)
        })?;
        vm.push(popped); Ok(())
    }),
    (SetClear, "clear", mutating, |vm, recv, pos| {
        check_arity(pos, 0, 0, "clear takes no arguments")?;
        set_mut(vm, recv, "clear: receiver is not a set", |set| {
            set.clear(); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (SetUpdate, "update", mutating, |vm, recv, pos| {
        check_arity(pos, 1, 1, "update takes 1 argument")?;
        let items = iter_to_vec(vm, pos[0])?;
        set_mut(vm, recv, "update: receiver is not a set", |set| {
            for v in items { set.insert(v); }
            Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),

    // set: pure (return a fresh set or a bool).
    (SetCopy, "copy", pure, |vm, recv, pos| {
        check_arity(pos, 0, 0, "copy takes no arguments")?;
        let items = set_clone(vm, recv)?;
        vm.alloc_and_push_set(items)
    }),
    (SetUnion, "union", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "union takes 1 argument")?;
        let mut out = set_clone(vm, recv)?;
        out.extend(iter_to_vec(vm, pos[0])?);
        vm.alloc_and_push_set(out)
    }),
    (SetIntersection, "intersection", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "intersection takes 1 argument")?;
        let lhs = set_clone(vm, recv)?;
        let rhs_items = iter_to_vec(vm, pos[0])?;
        let rhs: crate::util::fx::FxHashSet<Val> = rhs_items.into_iter().collect();
        let out: Vec<Val> = lhs.into_iter().filter(|v| rhs.contains(v)).collect();
        vm.alloc_and_push_set(out)
    }),
    (SetDifference, "difference", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "difference takes 1 argument")?;
        let lhs = set_clone(vm, recv)?;
        let rhs_items = iter_to_vec(vm, pos[0])?;
        let rhs: crate::util::fx::FxHashSet<Val> = rhs_items.into_iter().collect();
        let out: Vec<Val> = lhs.into_iter().filter(|v| !rhs.contains(v)).collect();
        vm.alloc_and_push_set(out)
    }),
    (SetSymmetricDifference, "symmetric_difference", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "symmetric_difference takes 1 argument")?;
        let lhs: crate::util::fx::FxHashSet<Val> = set_clone(vm, recv)?.into_iter().collect();
        let rhs: crate::util::fx::FxHashSet<Val> = iter_to_vec(vm, pos[0])?.into_iter().collect();
        let out: Vec<Val> = lhs.symmetric_difference(&rhs).copied().collect();
        vm.alloc_and_push_set(out)
    }),
    (SetIsSubset, "issubset", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "issubset takes 1 argument")?;
        let lhs = set_clone(vm, recv)?;
        let rhs: crate::util::fx::FxHashSet<Val> = iter_to_vec(vm, pos[0])?.into_iter().collect();
        vm.push(Val::bool(lhs.iter().all(|v| rhs.contains(v))));
        Ok(())
    }),
    (SetIsSuperset, "issuperset", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "issuperset takes 1 argument")?;
        let lhs: crate::util::fx::FxHashSet<Val> = set_clone(vm, recv)?.into_iter().collect();
        let rhs = iter_to_vec(vm, pos[0])?;
        vm.push(Val::bool(rhs.iter().all(|v| lhs.contains(v))));
        Ok(())
    }),
    (SetIsDisjoint, "isdisjoint", pure, |vm, recv, pos| {
        check_arity(pos, 1, 1, "isdisjoint takes 1 argument")?;
        let lhs: crate::util::fx::FxHashSet<Val> = set_clone(vm, recv)?.into_iter().collect();
        let rhs = iter_to_vec(vm, pos[0])?;
        vm.push(Val::bool(!rhs.iter().any(|v| lhs.contains(v))));
        Ok(())
    }),
}
