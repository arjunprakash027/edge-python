/*
Built-in methods for `dict` receivers. Arity is checked by the dispatcher; `mutating` is marked by the dispatcher when `MethodDesc::mutating` is true.
*/

use super::prelude::*;

pub fn keys(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let entries = dict_entries(vm, recv)?;
    let keys: Vec<Val> = entries.into_iter().map(|(k, _)| k).collect();
    vm.alloc_and_push_list(keys)
}

pub fn values(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let entries = dict_entries(vm, recv)?;
    let vals: Vec<Val> = entries.into_iter().map(|(_, v)| v).collect();
    vm.alloc_and_push_list(vals)
}

pub fn items(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let entries = dict_entries(vm, recv)?;
    let mut items: Vec<Val> = Vec::with_capacity(entries.len());
    for (k, vv) in entries {
        let t = vm.heap.alloc(HeapObj::Tuple(vec![k, vv]))?;
        items.push(t);
    }
    vm.alloc_and_push_list(items)
}

// `dict.copy()`, shallow copy; mutations don't affect the original.
pub fn copy(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let entries = dict_entries(vm, recv)?;
    let mut dm = DictMap::with_capacity(entries.len());
    for (k, v) in entries { dm.insert(k, v); }
    vm.alloc_and_push_dict(dm)
}

// `dict.popitem()`, pop the last (k, v); KeyError on empty dict.
pub fn popitem(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let pair = dict_mut(vm, recv, "popitem: receiver is not a dict", |dict| {
        let (k, v) = dict.entries.last().copied().ok_or(cold_value("popitem(): dictionary is empty"))?;
        dict.remove(&k);
        Ok((k, v))
    })?;
    vm.alloc_and_push_tuple(vec![pair.0, pair.1])
}

pub fn get(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let default = if pos.len() == 2 { pos[1] } else { Val::none() };
    let result = match vm.heap.get(recv) {
        HeapObj::Dict(rc) => rc.borrow().get(&pos[0]).copied().unwrap_or(default),
        _ => return Err(cold_type("get: receiver is not a dict")),
    };
    vm.push(result); Ok(())
}

pub fn update(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
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
}

pub fn pop(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let default = if pos.len() == 2 { Some(pos[1]) } else { None };
    let result = dict_mut(vm, recv, "pop: receiver is not a dict", |dict| {
        match dict.remove(&pos[0]) {
            Some(val) => Ok(val),
            None => default.ok_or(cold_value("key not found")),
        }
    })?;
    vm.push(result); Ok(())
}

pub fn setdefault(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let default = if pos.len() > 1 { pos[1] } else { Val::none() };
    let result = dict_mut(vm, recv, "setdefault: receiver is not a dict", |dict| {
        if let Some(v) = dict.get(&pos[0]).copied() { Ok(v) }
        else { dict.insert(pos[0], default); Ok(default) }
    })?;
    vm.push(result); Ok(())
}
