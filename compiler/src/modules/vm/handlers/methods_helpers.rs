/*
Receiver-type unwrap / arity-check primitives shared by the method dispatcher (methods.rs)
*/

use alloc::{string::{String, ToString}, vec::Vec};

use super::*;

#[inline]
pub(super) fn recv_str(vm: &VM, recv: Val) -> Result<String, VmErr> {
    match vm.heap.try_get(recv) {
        Some(HeapObj::Str(s)) => Ok(s.clone()),
        _ => Err(cold_type("method requires a string receiver")),
    }
}

#[inline]
pub(super) fn recv_bytes(vm: &VM, recv: Val) -> Result<Vec<u8>, VmErr> {
    match vm.heap.try_get(recv) {
        Some(HeapObj::Bytes(b)) => Ok(b.clone()),
        _ => Err(cold_type("method requires a bytes receiver")),
    }
}

#[inline]
pub(super) fn val_to_str(vm: &VM, v: Val) -> Result<String, VmErr> {
    match vm.heap.try_get(v) {
        Some(HeapObj::Str(s)) => Ok(s.clone()),
        _ => Err(cold_type("argument must be a string")),
    }
}

#[inline]
pub(super) fn list_clone(vm: &VM, recv: Val) -> Result<Vec<Val>, VmErr> {
    match vm.heap.try_get(recv) {
        Some(HeapObj::List(rc)) => Ok(rc.borrow().clone()),
        _ => Err(cold_type("method requires a list receiver")),
    }
}

#[inline]
pub(super) fn dict_entries(vm: &VM, recv: Val) -> Result<Vec<(Val, Val)>, VmErr> {
    match vm.heap.try_get(recv) {
        Some(HeapObj::Dict(rc)) => Ok(rc.borrow().entries.clone()),
        _ => Err(cold_type("method requires a dict receiver")),
    }
}

/* Borrow `recv`'s list mutably for `f`. The closure can't touch `vm` (held by `heap.get_mut`), so any push must happen after this returns. */
#[inline]
pub(super) fn list_mut<F, R>(vm: &mut VM, recv: Val, err: &'static str, f: F) -> Result<R, VmErr>
where F: FnOnce(&mut Vec<Val>) -> Result<R, VmErr>
{
    match vm.heap.try_get_mut(recv) {
        Some(HeapObj::List(rc)) => f(&mut rc.borrow_mut()),
        _ => Err(cold_type(err)),
    }
}

// Same shape as `list_mut` for dict receivers.
#[inline]
pub(super) fn dict_mut<F, R>(vm: &mut VM, recv: Val, err: &'static str, f: F) -> Result<R, VmErr>
where F: FnOnce(&mut DictMap) -> Result<R, VmErr>
{
    match vm.heap.try_get_mut(recv) {
        Some(HeapObj::Dict(rc)) => f(&mut rc.borrow_mut()),
        _ => Err(cold_type(err)),
    }
}

// Snapshot a set as Vec so the heap stays free for subsequent allocations.
#[inline]
pub(super) fn set_clone(vm: &VM, recv: Val) -> Result<Vec<Val>, VmErr> {
    match vm.heap.get(recv) {
        HeapObj::Set(rc) => Ok(rc.borrow().iter().copied().collect()),
        _ => Err(cold_type("method requires a set receiver")),
    }
}

// Same shape as `list_mut` for set receivers.
#[inline]
pub(super) fn set_mut<F, R>(vm: &mut VM, recv: Val, err: &'static str, f: F) -> Result<R, VmErr>
where F: FnOnce(&mut crate::util::fx::FxHashSet<Val>) -> Result<R, VmErr>
{
    match vm.heap.get_mut(recv) {
        HeapObj::Set(rc) => f(&mut rc.borrow_mut()),
        _ => Err(cold_type(err)),
    }
}

// `Vec<Val>` from any iterable (str/range/dict/bytes/frozenset/list/tuple/set), for set ops.
#[inline]
pub(super) fn iter_to_vec(vm: &mut VM, v: Val) -> Result<Vec<Val>, VmErr> {
    vm.extract_iter(v, true)
}

#[inline]
pub(super) fn capitalize_first(s: &str) -> String {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) => c.to_uppercase().to_string() + cs.as_str().to_lowercase().as_str(),
        None => String::new(),
    }
}

#[inline]
pub(super) fn title_case(s: &str) -> String {
    // each maximal run of cased chars is a word; first char titlecased, rest lowercased; non-cased chars (spaces, digits, punctuation) are boundaries.
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_cased { out.extend(c.to_lowercase()); } else { out.extend(c.to_uppercase()); }
            prev_cased = true;
        } else {
            out.push(c);
            prev_cased = false;
        }
    }
    out
}
