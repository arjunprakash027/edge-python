/*
Builtin-method descriptor table + dispatcher. Bodies live in per-type files (string/bytes/list/dict/set) as `pub fn`.
Descriptor: name + fn ptr + mutating flag + arity range; dispatcher checks arity uniformly.
*/

mod prelude;
pub mod bytes;
pub mod dict;
pub mod list;
pub mod set;
pub mod string;

use prelude::{VM, Val, VmErr, cold_type};
use crate::s;
use alloc::string::String;

pub type MethodFn = fn(&mut VM, Val, &[Val]) -> Result<(), VmErr>;

pub struct MethodDesc {
    pub name: &'static str,
    pub func: MethodFn,
    pub mutating: bool,
    pub min_args: u8,
    pub max_args: u8, // 255 = unbounded (unused today; reserved).
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinMethodId(u8);

impl BuiltinMethodId {
    #[inline] pub fn name(self) -> &'static str { ALL_METHODS[self.0 as usize].name }
}

// Contiguous-by-type so per-type lookup is a range scan over `ALL_METHODS`.
const STR_RANGE: core::ops::Range<usize> = 0..25;
const BYTES_RANGE: core::ops::Range<usize> = 25..34;
const LIST_RANGE: core::ops::Range<usize> = 34..45;
const DICT_RANGE: core::ops::Range<usize> = 45..54;
const SET_RANGE: core::ops::Range<usize> = 54..68;

pub static ALL_METHODS: &[MethodDesc] = &[
    // str (0..25)
    MethodDesc { name: "encode", func: string::encode, mutating: false, min_args: 0, max_args: 1 },
    MethodDesc { name: "upper", func: string::upper, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "lower", func: string::lower, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "strip", func: string::strip, mutating: false, min_args: 0, max_args: 1 },
    MethodDesc { name: "capitalize", func: string::capitalize, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "title", func: string::title, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "lstrip", func: string::lstrip, mutating: false, min_args: 0, max_args: 1 },
    MethodDesc { name: "rstrip", func: string::rstrip, mutating: false, min_args: 0, max_args: 1 },
    MethodDesc { name: "isdigit", func: string::isdigit, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "isalpha", func: string::isalpha, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "isalnum", func: string::isalnum, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "startswith", func: string::startswith, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "endswith", func: string::endswith, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "find", func: string::find, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "count", func: string::count, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "split", func: string::split, mutating: false, min_args: 0, max_args: 1 },
    MethodDesc { name: "join", func: string::join, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "replace", func: string::replace, mutating: false, min_args: 2, max_args: 2 },
    MethodDesc { name: "removeprefix", func: string::removeprefix, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "removesuffix", func: string::removesuffix, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "splitlines", func: string::splitlines, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "partition", func: string::partition, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "rpartition", func: string::rpartition, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "center", func: string::center, mutating: false, min_args: 1, max_args: 2 },
    MethodDesc { name: "zfill", func: string::zfill, mutating: false, min_args: 1, max_args: 1 },

    // bytes (25..34)
    MethodDesc { name: "decode", func: bytes::decode, mutating: false, min_args: 0, max_args: 1 },
    MethodDesc { name: "hex", func: bytes::hex, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "startswith", func: bytes::startswith, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "endswith", func: bytes::endswith, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "find", func: bytes::find, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "index", func: bytes::index, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "count", func: bytes::count, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "replace", func: bytes::replace, mutating: false, min_args: 2, max_args: 2 },
    MethodDesc { name: "split", func: bytes::split, mutating: false, min_args: 1, max_args: 1 },

    // list (34..45)
    MethodDesc { name: "index", func: list::index, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "count", func: list::count, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "copy", func: list::copy, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "append", func: list::append, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { name: "clear", func: list::clear, mutating: true, min_args: 0, max_args: 0 },
    MethodDesc { name: "reverse", func: list::reverse, mutating: true, min_args: 0, max_args: 0 },
    MethodDesc { name: "extend", func: list::extend, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { name: "insert", func: list::insert, mutating: true, min_args: 2, max_args: 2 },
    MethodDesc { name: "remove", func: list::remove, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { name: "pop", func: list::pop, mutating: true, min_args: 0, max_args: 1 },
    MethodDesc { name: "sort", func: list::sort, mutating: true, min_args: 0, max_args: 0 },

    // dict (45..54)
    MethodDesc { name: "keys", func: dict::keys, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "values", func: dict::values, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "items", func: dict::items, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "copy", func: dict::copy, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "popitem", func: dict::popitem, mutating: true, min_args: 0, max_args: 0 },
    MethodDesc { name: "get", func: dict::get, mutating: false, min_args: 1, max_args: 2 },
    MethodDesc { name: "update", func: dict::update, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { name: "pop", func: dict::pop, mutating: true, min_args: 1, max_args: 2 },
    MethodDesc { name: "setdefault", func: dict::setdefault, mutating: true, min_args: 1, max_args: 2 },

    // set (54..68)
    MethodDesc { name: "add", func: set::add, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { name: "remove", func: set::remove, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { name: "discard", func: set::discard, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { name: "pop", func: set::pop, mutating: true, min_args: 0, max_args: 0 },
    MethodDesc { name: "clear", func: set::clear, mutating: true, min_args: 0, max_args: 0 },
    MethodDesc { name: "update", func: set::update, mutating: true, min_args: 1, max_args: 1 },
    MethodDesc { name: "copy", func: set::copy, mutating: false, min_args: 0, max_args: 0 },
    MethodDesc { name: "union", func: set::union, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "intersection", func: set::intersection, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "difference", func: set::difference, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "symmetric_difference", func: set::symmetric_difference, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "issubset", func: set::issubset, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "issuperset", func: set::issuperset, mutating: false, min_args: 1, max_args: 1 },
    MethodDesc { name: "isdisjoint", func: set::isdisjoint, mutating: false, min_args: 1, max_args: 1 },
];

#[inline]
pub(crate) fn dispatch_method(
    vm: &mut VM, id: BuiltinMethodId, recv: Val, pos: &[Val], kw: &[Val],
) -> Result<(), VmErr> {
    if !kw.is_empty() {
        return Err(cold_type("builtin method takes no keyword arguments"));
    }
    let m = &ALL_METHODS[id.0 as usize];
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
    let range = match ty {
        "str" => STR_RANGE,
        "bytes" => BYTES_RANGE,
        "list" => LIST_RANGE,
        "dict" => DICT_RANGE,
        "set" => SET_RANGE,
        _ => return None,
    };
    ALL_METHODS[range.clone()]
        .iter()
        .position(|m| m.name == attr)
        .map(|i| BuiltinMethodId((range.start + i) as u8))
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
