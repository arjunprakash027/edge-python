/* Single source of truth for built-in methods (str/list/dict). The
   define_methods! macro at the bottom generates the BuiltinMethodId
   enum, name lookup, and dispatcher from one declarative table — adding
   a new method is one row. */

use super::*;
use crate::alloc::string::ToString;
use crate::s;

#[inline]
fn recv_str(vm: &VM, recv: Val) -> Result<String, VmErr> {
    match vm.heap.get(recv) {
        HeapObj::Str(s) => Ok(s.clone()),
        _ => Err(cold_type("method requires a string receiver")),
    }
}

#[inline]
fn recv_bytes(vm: &VM, recv: Val) -> Result<alloc::vec::Vec<u8>, VmErr> {
    match vm.heap.get(recv) {
        HeapObj::Bytes(b) => Ok(b.clone()),
        _ => Err(cold_type("method requires a bytes receiver")),
    }
}

#[inline]
fn val_to_str(vm: &VM, v: Val) -> Result<String, VmErr> {
    match vm.heap.get(v) {
        HeapObj::Str(s) => Ok(s.clone()),
        _ => Err(cold_type("argument must be a string")),
    }
}

#[inline]
fn check_arity(pos: &[Val], min: usize, max: usize, msg: &'static str) -> Result<(), VmErr> {
    if pos.len() < min || pos.len() > max {
        return Err(cold_type(msg));
    }
    Ok(())
}

#[inline]
fn list_clone(vm: &VM, recv: Val) -> Result<Vec<Val>, VmErr> {
    match vm.heap.get(recv) {
        HeapObj::List(rc) => Ok(rc.borrow().clone()),
        _ => Err(cold_type("method requires a list receiver")),
    }
}

#[inline]
fn dict_entries(vm: &VM, recv: Val) -> Result<Vec<(Val, Val)>, VmErr> {
    match vm.heap.get(recv) {
        HeapObj::Dict(rc) => Ok(rc.borrow().entries.clone()),
        _ => Err(cold_type("method requires a dict receiver")),
    }
}

/* Borrow the list inside `recv` mutably for the duration of `f`. The closure
   can't touch `vm` (it's borrowed by `heap.get_mut`), so any subsequent push
   has to happen after the helper returns. Replaces an 8× repeated
   `match heap.get_mut { HeapObj::List(rc) => ..., _ => err }` cascade in the
   list-mutating method bodies. */
#[inline]
fn list_mut<F, R>(vm: &mut VM, recv: Val, err: &'static str, f: F) -> Result<R, VmErr>
where F: FnOnce(&mut Vec<Val>) -> Result<R, VmErr>
{
    match vm.heap.get_mut(recv) {
        HeapObj::List(rc) => f(&mut rc.borrow_mut()),
        _ => Err(cold_type(err)),
    }
}

/* Same shape as `list_mut` for dict receivers. Used by the three mutating
   dict methods (update, pop, setdefault). */
#[inline]
fn dict_mut<F, R>(vm: &mut VM, recv: Val, err: &'static str, f: F) -> Result<R, VmErr>
where F: FnOnce(&mut DictMap) -> Result<R, VmErr>
{
    match vm.heap.get_mut(recv) {
        HeapObj::Dict(rc) => f(&mut rc.borrow_mut()),
        _ => Err(cold_type(err)),
    }
}

/* Snapshot of a set's contents. Returned to callers as a Vec rather than
   loaning a borrow of the heap so the heap is free during subsequent
   allocations (alloc_set, eq scans). Order is HashSet iteration order; set
   methods that produce a `set` re-collect into a fresh HashSet anyway. */
#[inline]
fn set_clone(vm: &VM, recv: Val) -> Result<Vec<Val>, VmErr> {
    match vm.heap.get(recv) {
        HeapObj::Set(rc) => Ok(rc.borrow().iter().copied().collect()),
        _ => Err(cold_type("method requires a set receiver")),
    }
}

/* Same shape as `list_mut` for set receivers. Used by mutating set methods
   (add, remove, discard, pop, clear, update). */
#[inline]
fn set_mut<F, R>(vm: &mut VM, recv: Val, err: &'static str, f: F) -> Result<R, VmErr>
where F: FnOnce(&mut crate::modules::fx::FxHashSet<Val>) -> Result<R, VmErr>
{
    match vm.heap.get_mut(recv) {
        HeapObj::Set(rc) => f(&mut rc.borrow_mut()),
        _ => Err(cold_type(err)),
    }
}

/* Pull a Vec<Val> from any iterable (list/tuple/set). Used by set ops that
   accept a non-set iterable (e.g. `s.update([1, 2])`, `s.union((3, 4))`). */
#[inline]
fn iter_to_vec(vm: &VM, v: Val) -> Result<Vec<Val>, VmErr> {
    if !v.is_heap() { return Err(cold_type("expected an iterable")); }
    match vm.heap.get(v) {
        HeapObj::List(rc) => Ok(rc.borrow().clone()),
        HeapObj::Tuple(t) => Ok(t.clone()),
        HeapObj::Set(rc) => Ok(rc.borrow().iter().copied().collect()),
        _ => Err(cold_type("expected an iterable")),
    }
}

#[inline]
fn capitalize_first(s: &str) -> String {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) => c.to_uppercase().to_string() + cs.as_str().to_lowercase().as_str(),
        None => String::new(),
    }
}

#[inline]
fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut cs = w.chars();
            cs.next()
                .map(|c| c.to_uppercase().to_string() + cs.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl<'a> VM<'a> {
    pub(crate) fn handle_load_attr(&mut self, name_idx: u16, chunk: &SSAChunk) -> Result<(), VmErr> {
        let name = chunk.names.get(name_idx as usize)
            .ok_or(VmErr::Runtime("LoadAttr: bad name index"))?;
        let obj = self.pop()?;

        // Module attribute lookup: linear scan over the attr table. Sized
        // for ~30 entries; any module larger than that is unusual.
        if obj.is_heap()
            && let HeapObj::Module(mod_name, attrs) = self.heap.get(obj) {
                let bare = crate::modules::parser::ssa_strip(name);
                if let Some((_, v)) = attrs.iter().find(|(n, _)| n == bare) {
                    let v = *v;
                    self.push(v);
                    return Ok(());
                }
                return Err(VmErr::Attribute(s!(
                    "module '", str mod_name, "' has no attribute '", str bare, "'")));
            }

        // ExcInstance attribute lookup: `e.args` returns the constructor
        // args as a tuple. Anything else falls through to the generic
        // `'exception' object has no attribute …` error.
        if obj.is_heap()
            && let HeapObj::ExcInstance(_, args) = self.heap.get(obj) {
                let bare = crate::modules::parser::ssa_strip(name);
                if bare == "args" {
                    let args = args.clone();
                    let v = self.heap.alloc(HeapObj::Tuple(args))?;
                    self.push(v);
                    return Ok(());
                }
                let ty = self.type_name(obj);
                return Err(VmErr::Attribute(s!(
                    "'", str ty, "' object has no attribute '", str bare, "'")));
            }

        // Class attribute lookup: `MyClass.method` returns the unbound
        // function directly (no `self` prepended). Useful for class-as-namespace
        // patterns and for accessing class-level constants.
        if obj.is_heap()
            && let HeapObj::Class(_, members) = self.heap.get(obj) {
                let bare = crate::modules::parser::ssa_strip(name);
                if let Some((_, v)) = members.iter().find(|(n, _)| n == bare) {
                    let v = *v;
                    self.push(v);
                    return Ok(());
                }
                let cls_name = match self.heap.get(obj) {
                    HeapObj::Class(n, _) => n.clone(),
                    _ => alloc::string::String::new(),
                };
                return Err(VmErr::Attribute(s!(
                    "type object '", str &cls_name, "' has no attribute '", str bare, "'")));
            }

        // Instance attribute lookup: check `__dict__` first, then class methods.
        if obj.is_heap()
            && let HeapObj::Instance(cls_val, attrs) = self.heap.get(obj) {
                let cls_val = *cls_val;
                let found = attrs.borrow().entries.iter()
                    .find(|(k, _)| k.is_heap() && matches!(self.heap.get(*k), HeapObj::Str(s) if s == name))
                    .map(|(_, v)| *v);
                if let Some(v) = found {
                    self.push(v);
                    return Ok(());
                }
                if cls_val.is_heap()
                    && let HeapObj::Class(_, methods) = self.heap.get(cls_val)
                    && let Some((_, mv)) = methods.iter().find(|(n, _)| n == name) {
                        let mv = *mv;
                            let bound = self.heap.alloc(HeapObj::BoundUserMethod(obj, mv))?;
                            self.push(bound);
                            return Ok(());
                        }
                let ty = self.type_name(obj);
                return Err(VmErr::Attribute(s!("'", str ty, "' object has no attribute '", str name, "'")));
            }

        // Builtin type method.
        let ty = self.type_name(obj);
        let method_id = lookup_method(ty, name.as_str())
            .ok_or_else(|| VmErr::Attribute(s!("'", str ty, "' object has no attribute '", str name, "'")))?;
        let bound = self.heap.alloc(HeapObj::BoundMethod(obj, method_id))?;
        self.push(bound);
        Ok(())
    }
}

/* Generates the BuiltinMethodId enum, name lookup, dispatcher, AND the
   (type, attr) -> BuiltinMethodId resolver in one go. Each row:
   (Variant, "name", category, |vm, recv, pos| body).
   Category `mutating` auto-emits mark_impure() on success.
   Receiver type is derived from the variant prefix: Str* -> "str",
   List* -> "list", Dict* -> "dict". Add new prefixes to `lookup_method` if
   you introduce methods on a new receiver type. */
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

        pub(crate) fn dispatch_method(
            vm: &mut VM, id: BuiltinMethodId,
            recv: Val, pos: Vec<Val>, kw: Vec<Val>,
        ) -> Result<(), VmErr> {
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

        /* Single source of truth — derived from the entries above. Off the
           hot path: CallMethod fusion bypasses LoadAttr+Call entirely. */
        pub fn lookup_method(ty: &str, attr: &str) -> Option<BuiltinMethodId> {
            let prefix = match ty {
                "str"   => "Str",
                "bytes" => "Bytes",
                "list"  => "List",
                "dict"  => "Dict",
                "set"   => "Set",
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
    // str.encode([encoding]) — to bytes. Default and only supported
    // encoding is UTF-8 (ASCII is a strict subset that succeeds when the
    // string is pure ASCII; any other name errors out so silent
    // mismatches don't sneak through).
    (StrEncode, "encode", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 1, "encode takes 0 or 1 arguments")?;
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

    // bytes.decode([encoding]) — back to str. Validates UTF-8 and errors
    // on invalid sequences (Python raises UnicodeDecodeError; we surface
    // it as a ValueError to keep the error taxonomy compact).
    (BytesDecode, "decode", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 1, "decode takes 0 or 1 arguments")?;
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

    // bytes.hex() — lowercase hex of every byte. No separator.
    (BytesHex, "hex", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "hex takes no arguments")?;
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

    // bytes.startswith(prefix) / bytes.endswith(suffix) — bytes-only
    // prefix matching (str.startswith handles strings).
    (BytesStartswith, "startswith", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "startswith takes 1 argument")?;
        let buf = recv_bytes(vm, recv)?;
        let prefix = recv_bytes(vm, pos[0])?;
        vm.push(Val::bool(buf.starts_with(&prefix)));
        Ok(())
    }),
    (BytesEndswith, "endswith", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "endswith takes 1 argument")?;
        let buf = recv_bytes(vm, recv)?;
        let suffix = recv_bytes(vm, pos[0])?;
        vm.push(Val::bool(buf.ends_with(&suffix)));
        Ok(())
    }),

    // str: zero-arg transforms.
    (StrUpper, "upper", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "upper takes no arguments")?;
        let s = recv_str(vm, recv)?;
        let v = vm.heap.alloc(HeapObj::Str(s.to_uppercase()))?;
        vm.push(v); Ok(())
    }),
    (StrLower, "lower", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "lower takes no arguments")?;
        let s = recv_str(vm, recv)?;
        let v = vm.heap.alloc(HeapObj::Str(s.to_lowercase()))?;
        vm.push(v); Ok(())
    }),
    (StrStrip, "strip", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 1, "strip takes 0 or 1 arguments")?;
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
        check_arity(&pos, 0, 0, "capitalize takes no arguments")?;
        let s = recv_str(vm, recv)?;
        let v = vm.heap.alloc(HeapObj::Str(capitalize_first(&s)))?;
        vm.push(v); Ok(())
    }),
    (StrTitle, "title", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "title takes no arguments")?;
        let s = recv_str(vm, recv)?;
        let v = vm.heap.alloc(HeapObj::Str(title_case(&s)))?;
        vm.push(v); Ok(())
    }),

    // str: optional separator.
    (StrLstrip, "lstrip", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 1, "lstrip takes 0 or 1 arguments")?;
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
        check_arity(&pos, 0, 1, "rstrip takes 0 or 1 arguments")?;
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
        check_arity(&pos, 0, 0, "isdigit takes no arguments")?;
        let s = recv_str(vm, recv)?;
        vm.push(Val::bool(!s.is_empty() && s.chars().all(|c| c.is_ascii_digit())));
        Ok(())
    }),
    (StrIsAlpha, "isalpha", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "isalpha takes no arguments")?;
        let s = recv_str(vm, recv)?;
        vm.push(Val::bool(!s.is_empty() && s.chars().all(|c| c.is_alphabetic())));
        Ok(())
    }),
    (StrIsAlnum, "isalnum", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "isalnum takes no arguments")?;
        let s = recv_str(vm, recv)?;
        vm.push(Val::bool(!s.is_empty() && s.chars().all(|c| c.is_alphanumeric())));
        Ok(())
    }),

    // str: queries with one string arg.
    (StrStartswith, "startswith", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "startswith takes 1 argument")?;
        let s = recv_str(vm, recv)?;
        let p = val_to_str(vm, pos[0])?;
        vm.push(Val::bool(s.starts_with(p.as_str())));
        Ok(())
    }),
    (StrEndswith, "endswith", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "endswith takes 1 argument")?;
        let s = recv_str(vm, recv)?;
        let p = val_to_str(vm, pos[0])?;
        vm.push(Val::bool(s.ends_with(p.as_str())));
        Ok(())
    }),
    (StrFind, "find", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "find takes 1 argument")?;
        let s = recv_str(vm, recv)?;
        let sub = val_to_str(vm, pos[0])?;
        let idx = s.find(sub.as_str())
            .map(|i| s[..i].chars().count() as i64)
            .unwrap_or(-1);
        vm.push(Val::int(idx));
        Ok(())
    }),
    (StrCount, "count", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "count takes 1 argument")?;
        let s = recv_str(vm, recv)?;
        let sub = val_to_str(vm, pos[0])?;
        vm.push(Val::int(s.matches(sub.as_str()).count() as i64));
        Ok(())
    }),

    // str: split / join / replace.
    (StrSplit, "split", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 1, "split takes 0 or 1 arguments")?;
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
        check_arity(&pos, 1, 1, "join takes 1 argument")?;
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
        check_arity(&pos, 2, 2, "replace takes 2 arguments")?;
        let s = recv_str(vm, recv)?;
        let old = val_to_str(vm, pos[0])?;
        let new = val_to_str(vm, pos[1])?;
        let v = vm.heap.alloc(HeapObj::Str(s.replace(old.as_str(), new.as_str())))?;
        vm.push(v); Ok(())
    }),

    /* str.removeprefix(p) — strip leading prefix if present, else return
       unchanged. str.removesuffix(s) — same for trailing suffix. */
    (StrRemovePrefix, "removeprefix", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "removeprefix takes 1 argument")?;
        let s = recv_str(vm, recv)?;
        let p = val_to_str(vm, pos[0])?;
        let out = s.strip_prefix(p.as_str()).map(|t| t.to_string()).unwrap_or(s);
        let v = vm.heap.alloc(HeapObj::Str(out))?;
        vm.push(v); Ok(())
    }),
    (StrRemoveSuffix, "removesuffix", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "removesuffix takes 1 argument")?;
        let s = recv_str(vm, recv)?;
        let suf = val_to_str(vm, pos[0])?;
        let out = s.strip_suffix(suf.as_str()).map(|t| t.to_string()).unwrap_or(s);
        let v = vm.heap.alloc(HeapObj::Str(out))?;
        vm.push(v); Ok(())
    }),

    /* str.splitlines() — split on every \n, \r, or \r\n, dropping the
       separator. Mirrors Python's keepends=False default. */
    (StrSplitlines, "splitlines", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "splitlines takes no arguments")?;
        let s = recv_str(vm, recv)?;
        let mut parts: Vec<Val> = Vec::new();
        for line in s.split_inclusive(['\n', '\r']) {
            let trimmed = line.trim_end_matches(['\n', '\r']).to_string();
            parts.push(vm.heap.alloc(HeapObj::Str(trimmed))?);
        }
        // split_inclusive does not yield an empty trailing chunk, but if the
        // last char is a separator the loop above produces one extra empty
        // segment — drop it to match Python.
        if let Some(last) = parts.last()
            && let HeapObj::Str(t) = vm.heap.get(*last)
            && t.is_empty() && s.ends_with(['\n', '\r']) {
                parts.pop();
            }
        vm.alloc_and_push_list(parts)
    }),

    /* str.partition(sep) — find sep, return (head, sep, tail). If sep is
       absent: (s, "", ""). rpartition splits at the last occurrence. */
    (StrPartition, "partition", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "partition takes 1 argument")?;
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
        check_arity(&pos, 1, 1, "rpartition takes 1 argument")?;
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

    /* bytes.find(sub) / bytes.index(sub) / bytes.count(sub) / bytes.replace(old, new)
       — byte-oriented analogs of the str methods. */
    (BytesFind, "find", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "find takes 1 argument")?;
        let buf = recv_bytes(vm, recv)?;
        let sub = recv_bytes(vm, pos[0])?;
        let idx = buf.windows(sub.len()).position(|w| w == sub.as_slice())
            .map(|i| i as i64).unwrap_or(-1);
        vm.push(Val::int(idx));
        Ok(())
    }),
    (BytesIndex, "index", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "index takes 1 argument")?;
        let buf = recv_bytes(vm, recv)?;
        let sub = recv_bytes(vm, pos[0])?;
        let idx = buf.windows(sub.len()).position(|w| w == sub.as_slice())
            .ok_or(cold_value("subsection not found"))?;
        vm.push(Val::int(idx as i64));
        Ok(())
    }),
    (BytesCount, "count", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "count takes 1 argument")?;
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
        check_arity(&pos, 2, 2, "replace takes 2 arguments")?;
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
        check_arity(&pos, 1, 1, "split takes 1 argument")?;
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
        check_arity(&pos, 1, 2, "center takes 1 or 2 arguments")?;
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
        check_arity(&pos, 1, 1, "zfill takes 1 argument")?;
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
        check_arity(&pos, 1, 1, "index takes 1 argument")?;
        let items = list_clone(vm, recv)?;
        let idx = items.iter()
            .position(|&v| eq_vals_with_heap(v, pos[0], &vm.heap))
            .map(|i| i as i64)
            .ok_or(cold_value("value not found in list"))?;
        vm.push(Val::int(idx));
        Ok(())
    }),
    (ListCount, "count", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "count takes 1 argument")?;
        let items = list_clone(vm, recv)?;
        let n = items.iter().filter(|&&v| eq_vals_with_heap(v, pos[0], &vm.heap)).count() as i64;
        vm.push(Val::int(n));
        Ok(())
    }),
    (ListCopy, "copy", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "copy takes no arguments")?;
        let items = list_clone(vm, recv)?;
        vm.alloc_and_push_list(items)
    }),

    // list: mutating.
    (ListAppend, "append", mutating, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "append takes 1 argument")?;
        list_mut(vm, recv, "append: receiver is not a list", |list| {
            list.push(pos[0]); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (ListClear, "clear", mutating, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "clear takes no arguments")?;
        list_mut(vm, recv, "clear: receiver is not a list", |list| {
            list.clear(); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (ListReverse, "reverse", mutating, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "reverse takes no arguments")?;
        list_mut(vm, recv, "reverse: receiver is not a list", |list| {
            list.reverse(); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (ListExtend, "extend", mutating, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "extend takes 1 argument")?;
        let items = vm.extract_iter(pos[0], true)?;
        list_mut(vm, recv, "extend: receiver is not a list", |list| {
            list.extend_from_slice(&items); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (ListInsert, "insert", mutating, |vm, recv, pos| {
        check_arity(&pos, 2, 2, "insert takes 2 arguments")?;
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
        check_arity(&pos, 1, 1, "remove takes 1 argument")?;
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
        check_arity(&pos, 0, 1, "pop takes 0 or 1 arguments")?;
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
        check_arity(&pos, 0, 0, "sort takes no arguments")?;
        let mut sorted = list_clone(vm, recv)?;
        vm.sort_by_lt(&mut sorted)?;
        list_mut(vm, recv, "sort: receiver is not a list", |list| {
            *list = sorted; Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),

    // dict.
    (DictKeys, "keys", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "keys takes no arguments")?;
        let entries = dict_entries(vm, recv)?;
        let keys: Vec<Val> = entries.into_iter().map(|(k, _)| k).collect();
        vm.alloc_and_push_list(keys)
    }),
    (DictValues, "values", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "values takes no arguments")?;
        let entries = dict_entries(vm, recv)?;
        let vals: Vec<Val> = entries.into_iter().map(|(_, v)| v).collect();
        vm.alloc_and_push_list(vals)
    }),
    (DictItems, "items", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "items takes no arguments")?;
        let entries = dict_entries(vm, recv)?;
        let mut items: Vec<Val> = Vec::with_capacity(entries.len());
        for (k, vv) in entries {
            let t = vm.heap.alloc(HeapObj::Tuple(vec![k, vv]))?;
            items.push(t);
        }
        vm.alloc_and_push_list(items)
    }),
    /* dict.copy() — shallow copy. Mutations to the result don't affect
       the original. */
    (DictCopy, "copy", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "copy takes no arguments")?;
        let entries = dict_entries(vm, recv)?;
        let mut dm = DictMap::with_capacity(entries.len());
        for (k, v) in entries { dm.insert(k, v); }
        vm.alloc_and_push_dict(dm)
    }),
    /* dict.popitem() — remove and return the last (key, value) tuple.
       Raises KeyError on an empty dict. */
    (DictPopItem, "popitem", mutating, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "popitem takes no arguments")?;
        let pair = dict_mut(vm, recv, "popitem: receiver is not a dict", |dict| {
            let (k, v) = dict.entries.last().copied().ok_or(cold_value("popitem(): dictionary is empty"))?;
            dict.remove(&k);
            Ok((k, v))
        })?;
        vm.alloc_and_push_tuple(vec![pair.0, pair.1])
    }),
    (DictGet, "get", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 2, "get takes 1 or 2 arguments")?;
        let default = if pos.len() == 2 { pos[1] } else { Val::none() };
        let result = match vm.heap.get(recv) {
            HeapObj::Dict(rc) => rc.borrow().get(&pos[0]).copied().unwrap_or(default),
            _ => return Err(cold_type("get: receiver is not a dict")),
        };
        vm.push(result); Ok(())
    }),
    (DictUpdate, "update", mutating, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "update takes 1 argument")?;
        // Accept either a dict (entries reused directly) or an iterable of
        // 2-element pair sequences ([(k, v), ...]) — matches CPython contract.
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
        check_arity(&pos, 1, 2, "pop takes 1 or 2 arguments")?;
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
        check_arity(&pos, 1, 2, "setdefault takes 1 or 2 arguments")?;
        let default = if pos.len() > 1 { pos[1] } else { Val::none() };
        let result = dict_mut(vm, recv, "setdefault: receiver is not a dict", |dict| {
            if let Some(v) = dict.get(&pos[0]).copied() { Ok(v) }
            else { dict.insert(pos[0], default); Ok(default) }
        })?;
        vm.push(result); Ok(())
    }),

    // set: mutating.
    (SetAdd, "add", mutating, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "add takes 1 argument")?;
        set_mut(vm, recv, "add: receiver is not a set", |set| {
            set.insert(pos[0]); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (SetRemove, "remove", mutating, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "remove takes 1 argument")?;
        set_mut(vm, recv, "remove: receiver is not a set", |set| {
            // CPython: KeyError, not ValueError.
            if !set.remove(&pos[0]) { return Err(VmErr::Raised("KeyError".into())); }
            Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (SetDiscard, "discard", mutating, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "discard takes 1 argument")?;
        set_mut(vm, recv, "discard: receiver is not a set", |set| {
            set.remove(&pos[0]); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (SetPop, "pop", mutating, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "pop takes no arguments")?;
        let popped = set_mut(vm, recv, "pop: receiver is not a set", |set| {
            // Pop an arbitrary element. HashSet doesn't expose pop(), so we
            // grab one via iter() and remove it. Empty set raises like CPython.
            let pick = set.iter().next().copied()
                .ok_or(cold_value("pop from an empty set"))?;
            set.remove(&pick);
            Ok(pick)
        })?;
        vm.push(popped); Ok(())
    }),
    (SetClear, "clear", mutating, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "clear takes no arguments")?;
        set_mut(vm, recv, "clear: receiver is not a set", |set| {
            set.clear(); Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),
    (SetUpdate, "update", mutating, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "update takes 1 argument")?;
        let items = iter_to_vec(vm, pos[0])?;
        set_mut(vm, recv, "update: receiver is not a set", |set| {
            for v in items { set.insert(v); }
            Ok(())
        })?;
        vm.push(Val::none()); Ok(())
    }),

    // set: pure (return a fresh set or a bool).
    (SetCopy, "copy", pure, |vm, recv, pos| {
        check_arity(&pos, 0, 0, "copy takes no arguments")?;
        let items = set_clone(vm, recv)?;
        vm.alloc_and_push_set(items)
    }),
    (SetUnion, "union", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "union takes 1 argument")?;
        let mut out = set_clone(vm, recv)?;
        out.extend(iter_to_vec(vm, pos[0])?);
        vm.alloc_and_push_set(out)
    }),
    (SetIntersection, "intersection", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "intersection takes 1 argument")?;
        let lhs = set_clone(vm, recv)?;
        let rhs_items = iter_to_vec(vm, pos[0])?;
        let rhs: crate::modules::fx::FxHashSet<Val> = rhs_items.into_iter().collect();
        let out: Vec<Val> = lhs.into_iter().filter(|v| rhs.contains(v)).collect();
        vm.alloc_and_push_set(out)
    }),
    (SetDifference, "difference", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "difference takes 1 argument")?;
        let lhs = set_clone(vm, recv)?;
        let rhs_items = iter_to_vec(vm, pos[0])?;
        let rhs: crate::modules::fx::FxHashSet<Val> = rhs_items.into_iter().collect();
        let out: Vec<Val> = lhs.into_iter().filter(|v| !rhs.contains(v)).collect();
        vm.alloc_and_push_set(out)
    }),
    (SetSymmetricDifference, "symmetric_difference", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "symmetric_difference takes 1 argument")?;
        let lhs: crate::modules::fx::FxHashSet<Val> = set_clone(vm, recv)?.into_iter().collect();
        let rhs: crate::modules::fx::FxHashSet<Val> = iter_to_vec(vm, pos[0])?.into_iter().collect();
        let out: Vec<Val> = lhs.symmetric_difference(&rhs).copied().collect();
        vm.alloc_and_push_set(out)
    }),
    (SetIsSubset, "issubset", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "issubset takes 1 argument")?;
        let lhs = set_clone(vm, recv)?;
        let rhs: crate::modules::fx::FxHashSet<Val> = iter_to_vec(vm, pos[0])?.into_iter().collect();
        vm.push(Val::bool(lhs.iter().all(|v| rhs.contains(v))));
        Ok(())
    }),
    (SetIsSuperset, "issuperset", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "issuperset takes 1 argument")?;
        let lhs: crate::modules::fx::FxHashSet<Val> = set_clone(vm, recv)?.into_iter().collect();
        let rhs = iter_to_vec(vm, pos[0])?;
        vm.push(Val::bool(rhs.iter().all(|v| lhs.contains(v))));
        Ok(())
    }),
    (SetIsDisjoint, "isdisjoint", pure, |vm, recv, pos| {
        check_arity(&pos, 1, 1, "isdisjoint takes 1 argument")?;
        let lhs: crate::modules::fx::FxHashSet<Val> = set_clone(vm, recv)?.into_iter().collect();
        let rhs = iter_to_vec(vm, pos[0])?;
        vm.push(Val::bool(!rhs.iter().any(|v| lhs.contains(v))));
        Ok(())
    }),
}