/*
Builtin-method descriptor table + dispatcher. Bodies live in per-type files (string/bytes/list/dict/set) as `pub fn`.
Descriptor: name + fn ptr + mutating flag + arity range; dispatcher checks arity uniformly.
*/

mod prelude;
pub mod bytes;
pub mod dict;
pub mod list;
pub mod numeric;
pub mod set;
pub mod string;

use prelude::{VM, Val, VmErr, cold_type};
use crate::s;
use alloc::string::String;

pub type MethodFn = fn(&mut VM, Val, &[Val]) -> Result<(), VmErr>;

pub struct MethodDesc {
    pub ty: &'static str, // receiver type ("str"/"bytes"/"list"/"dict"/"set"); drives lookup.
    pub name: &'static str,
    pub func: MethodFn,
    pub mutating: bool,
    pub min_args: u8,
    pub max_args: u8, // 255 = unbounded (variadic).
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinMethodId(u8);

impl BuiltinMethodId {
    #[inline] pub fn name(self) -> &'static str { ALL_METHODS[self.0 as usize].name }
}

pub static ALL_METHODS: &[MethodDesc] = &[
    // str (0..25)
    MethodDesc { ty: "str", name: "encode", func: string::encode, mutating: false, min_args: 0, max_args: 1 },
    MethodDesc { ty: "str", name: "upper", func: string::upper, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "str", name: "lower", func: string::lower, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "str", name: "strip", func: string::strip, mutating: false, min_args: 0, max_args: 1 },
    MethodDesc { ty: "str", name: "capitalize", func: string::capitalize, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "str", name: "title", func: string::title, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "str", name: "lstrip", func: string::lstrip, mutating: false, min_args: 0, max_args: 1 },
    MethodDesc { ty: "str", name: "rstrip", func: string::rstrip, mutating: false, min_args: 0, max_args: 1 },
    MethodDesc { ty: "str", name: "isdigit", func: string::isdigit, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "str", name: "isalpha", func: string::isalpha, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "str", name: "isalnum", func: string::isalnum, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "str", name: "startswith", func: string::startswith, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "str", name: "endswith", func: string::endswith, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "str", name: "find", func: string::find, mutating: false, min_args: 1, max_args: 3 },
    MethodDesc { ty: "str", name: "count", func: string::count, mutating: false, min_args: 1, max_args: 3 },
    MethodDesc { ty: "str", name: "split", func: string::split, mutating: false, min_args: 0, max_args: 2 },
    MethodDesc { ty: "str", name: "join", func: string::join, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "str", name: "replace", func: string::replace, mutating: false, min_args: 2, max_args: 3 },
    MethodDesc { ty: "str", name: "removeprefix", func: string::removeprefix, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "str", name: "removesuffix", func: string::removesuffix, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "str", name: "splitlines", func: string::splitlines, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "str", name: "partition", func: string::partition, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "str", name: "rpartition", func: string::rpartition, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "str", name: "center", func: string::center, mutating: false, min_args: 1, max_args: 2 },
    MethodDesc { ty: "str", name: "zfill", func: string::zfill, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "str", name: "rsplit", func: string::rsplit, mutating: false, min_args: 0, max_args: 2 },
    MethodDesc { ty: "str", name: "casefold", func: string::casefold, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "str", name: "swapcase", func: string::swapcase, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "str", name: "ljust", func: string::ljust, mutating: false, min_args: 1, max_args: 2 },
    MethodDesc { ty: "str", name: "rjust", func: string::rjust, mutating: false, min_args: 1, max_args: 2 },
    MethodDesc { ty: "str", name: "expandtabs", func: string::expandtabs, mutating: false, min_args: 0, max_args: 1 },
    MethodDesc { ty: "str", name: "rfind", func: string::rfind, mutating: false, min_args: 1, max_args: 3 },
    MethodDesc { ty: "str", name: "index", func: string::index, mutating: false, min_args: 1, max_args: 3 },
    MethodDesc { ty: "str", name: "rindex", func: string::rindex, mutating: false, min_args: 1, max_args: 3 },
    MethodDesc { ty: "str", name: "isspace", func: string::isspace, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "str", name: "isupper", func: string::isupper, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "str", name: "islower", func: string::islower, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "str", name: "istitle", func: string::istitle, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "str", name: "format", func: string::format, mutating: false, min_args: 0, max_args: 255 },

    // bytes (25..34)
    MethodDesc { ty: "bytes", name: "decode", func: bytes::decode, mutating: false, min_args: 0, max_args: 2 },
    MethodDesc { ty: "bytes", name: "hex", func: bytes::hex, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "bytes", name: "startswith", func: bytes::startswith, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "bytes", name: "endswith", func: bytes::endswith, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "bytes", name: "find", func: bytes::find, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "bytes", name: "index", func: bytes::index, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "bytes", name: "count", func: bytes::count, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "bytes", name: "replace", func: bytes::replace, mutating: false, min_args: 2, max_args: 2 },
    MethodDesc { ty: "bytes", name: "split", func: bytes::split, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "bytes", name: "lower", func: bytes::lower, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "bytes", name: "upper", func: bytes::upper, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "bytes", name: "strip", func: bytes::strip, mutating: false, min_args: 0, max_args: 1 },
    MethodDesc { ty: "bytes", name: "lstrip", func: bytes::lstrip, mutating: false, min_args: 0, max_args: 1 },
    MethodDesc { ty: "bytes", name: "rstrip", func: bytes::rstrip, mutating: false, min_args: 0, max_args: 1 },
    MethodDesc { ty: "bytes", name: "join", func: bytes::join, mutating: false, min_args: 1, max_args: 1 },

    // list (34..45)
    MethodDesc { ty: "list", name: "index", func: list::index, mutating: false, min_args: 1, max_args: 3 },
    MethodDesc { ty: "list", name: "count", func: list::count, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "list", name: "copy", func: list::copy, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "list", name: "append", func: list::append, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { ty: "list", name: "clear", func: list::clear, mutating: true, min_args: 0, max_args: 0 },
    MethodDesc { ty: "list", name: "reverse", func: list::reverse, mutating: true, min_args: 0, max_args: 0 },
    MethodDesc { ty: "list", name: "extend", func: list::extend, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { ty: "list", name: "insert", func: list::insert, mutating: true, min_args: 2, max_args: 2 },
    MethodDesc { ty: "list", name: "remove", func: list::remove, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { ty: "list", name: "pop", func: list::pop, mutating: true, min_args: 0, max_args: 1 },
    MethodDesc { ty: "list", name: "sort", func: list::sort, mutating: true, min_args: 0, max_args: 0 },

    // dict (45..54)
    MethodDesc { ty: "dict", name: "keys", func: dict::keys, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "dict", name: "values", func: dict::values, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "dict", name: "items", func: dict::items, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "dict", name: "copy", func: dict::copy, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "dict", name: "popitem", func: dict::popitem, mutating: true, min_args: 0, max_args: 0 },
    MethodDesc { ty: "dict", name: "get", func: dict::get, mutating: false, min_args: 1, max_args: 2 },
    MethodDesc { ty: "dict", name: "update", func: dict::update, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { ty: "dict", name: "pop", func: dict::pop, mutating: true, min_args: 1, max_args: 2 },
    MethodDesc { ty: "dict", name: "setdefault", func: dict::setdefault, mutating: true, min_args: 1, max_args: 2 },

    // set (54..68)
    MethodDesc { ty: "set", name: "add", func: set::add, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { ty: "set", name: "remove", func: set::remove, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { ty: "set", name: "discard", func: set::discard, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { ty: "set", name: "pop", func: set::pop, mutating: true, min_args: 0, max_args: 0 },
    MethodDesc { ty: "set", name: "clear", func: set::clear, mutating: true, min_args: 0, max_args: 0 },
    MethodDesc { ty: "set", name: "update", func: set::update, mutating: true, min_args: 0, max_args: 255 },
    MethodDesc { ty: "set", name: "copy", func: set::copy, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "set", name: "union", func: set::union, mutating: false, min_args: 0, max_args: 255 },
    MethodDesc { ty: "set", name: "intersection", func: set::intersection, mutating: false, min_args: 0, max_args: 255 },
    MethodDesc { ty: "set", name: "difference", func: set::difference, mutating: false, min_args: 0, max_args: 255 },
    MethodDesc { ty: "set", name: "symmetric_difference", func: set::symmetric_difference, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "set", name: "intersection_update", func: set::intersection_update, mutating: true, min_args: 0, max_args: 255 },
    MethodDesc { ty: "set", name: "difference_update", func: set::difference_update, mutating: true, min_args: 0, max_args: 255 },
    MethodDesc { ty: "set", name: "symmetric_difference_update", func: set::symmetric_difference_update, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { ty: "set", name: "issubset", func: set::issubset, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "set", name: "issuperset", func: set::issuperset, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { ty: "set", name: "isdisjoint", func: set::isdisjoint, mutating: false, min_args: 1, max_args: 1 },

    // int / float methods + classmethods (resolved via the type's own name).
    MethodDesc { ty: "int", name: "bit_length", func: numeric::bit_length, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "int", name: "bit_count", func: numeric::bit_count, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "int", name: "to_bytes", func: numeric::to_bytes, mutating: false, min_args: 0, max_args: 2 },
    MethodDesc { ty: "int", name: "from_bytes", func: numeric::from_bytes, mutating: false, min_args: 1, max_args: 2 },
    MethodDesc { ty: "float", name: "is_integer", func: numeric::is_integer, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { ty: "dict", name: "fromkeys", func: dict::fromkeys, mutating: false, min_args: 1, max_args: 2 },
    MethodDesc { ty: "bytes", name: "fromhex", func: bytes::fromhex, mutating: false, min_args: 1, max_args: 1 },
];

#[inline]
pub(crate) fn dispatch_method(vm: &mut VM, id: BuiltinMethodId, recv: Val, pos: &[Val], kw: &[Val]) -> Result<(), VmErr> {
    let m = &ALL_METHODS[id.0 as usize];
    if !kw.is_empty() {
        // `dict.update(**kwargs)` is the one builtin method taking keywords: pack them into a dict and append as a positional, which `dict::update` already merges.
        if m.ty == "dict" && m.name == "update" {
            if let Some(kwd) = VM::pack_kw_dict(&mut vm.heap, kw)? {
                let mut p = alloc::vec::Vec::with_capacity(pos.len() + 1);
                p.extend_from_slice(pos);
                p.push(kwd);
                let result = (m.func)(vm, recv, &p);
                if result.is_ok() { vm.mark_impure(); }
                return result;
            }
        }
        return Err(cold_type("builtin method takes no keyword arguments"));
    }
    let n = pos.len();
    if n < m.min_args as usize || (m.max_args != 255 && n > m.max_args as usize) {
        return Err(arity_error(m.name, m.min_args, m.max_args, n));
    }
    let result = (m.func)(vm, recv, pos);
    if m.mutating && result.is_ok() {
        vm.mark_impure();
    }
    result
}

pub fn lookup_method(ty: &str, attr: &str) -> Option<BuiltinMethodId> {
    // Scan by (ty, name); order-independent, so new methods can be appended anywhere.
    ALL_METHODS
        .iter()
        .position(|m| m.ty == ty && m.name == attr)
        .map(|i| BuiltinMethodId(i as u8))
}

#[cold]
fn arity_error(name: &str, min: u8, max: u8, got: usize) -> VmErr {
    let msg: String = if min == max {
        s!(str name, "() takes ", int min as i64, " arg(s), got ", int got as i64)
    } else if max == 255 {
        s!(str name, "() takes at least ", int min as i64, ", got ", int got as i64)
    } else {
        s!(str name, "() takes ", int min as i64, "..", int max as i64, " args, got ", int got as i64)
    };
    VmErr::TypeMsg(msg)
}
