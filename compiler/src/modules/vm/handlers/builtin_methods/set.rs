/*
Built-in methods for `set` receivers. Arity is checked by the dispatcher; `mutating` is marked by the dispatcher when `MethodDesc::mutating` is true.
*/

use super::prelude::*;

pub fn add(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    set_mut(vm, recv, "add: receiver is not a set", |set| {
        set.insert(pos[0]); Ok(())
    })?;
    vm.push(Val::none()); Ok(())
}

pub fn remove(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    set_mut(vm, recv, "remove: receiver is not a set", |set| {
        // KeyError, not ValueError.
        if !set.remove(&pos[0]) { return Err(VmErr::Raised("KeyError".into())); }
        Ok(())
    })?;
    vm.push(Val::none()); Ok(())
}

pub fn discard(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    set_mut(vm, recv, "discard: receiver is not a set", |set| {
        set.remove(&pos[0]); Ok(())
    })?;
    vm.push(Val::none()); Ok(())
}

pub fn pop(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let popped = set_mut(vm, recv, "pop: receiver is not a set", |set| {
        // HashSet has no `pop()`, grab via `iter()` and remove. Empty set raises.
        let pick = set.iter().next().copied().ok_or(cold_key("pop from an empty set"))?;
        set.remove(&pick);
        Ok(pick)
    })?;
    vm.push(popped); Ok(())
}

pub fn clear(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    set_mut(vm, recv, "clear: receiver is not a set", |set| {
        set.clear(); Ok(())
    })?;
    vm.push(Val::none()); Ok(())
}

// Materialize every argument iterable up front (each may run iteration code).
fn collect_args(vm: &mut VM, pos: &[Val]) -> Result<Vec<Vec<Val>>, VmErr> {
    pos.iter().map(|&a| iter_to_vec(vm, a)).collect()
}

pub fn update(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let args = collect_args(vm, pos)?;
    set_mut(vm, recv, "update: receiver is not a set", |set| {
        for items in args { for v in items { set.insert(v); } }
        Ok(())
    })?;
    vm.push(Val::none()); Ok(())
}

pub fn copy(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let items = set_clone(vm, recv)?;
    vm.alloc_and_push_set(items)
}

pub fn union(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let args = collect_args(vm, pos)?;
    let mut out = set_clone(vm, recv)?;
    for items in args { out.extend(items); }
    vm.alloc_and_push_set(out)
}

pub fn intersection(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let args = collect_args(vm, pos)?;
    let mut out: crate::util::fx::FxHashSet<Val> = set_clone(vm, recv)?.into_iter().collect();
    for items in args {
        let rhs: crate::util::fx::FxHashSet<Val> = items.into_iter().collect();
        out.retain(|v| rhs.contains(v));
    }
    vm.alloc_and_push_set(out.into_iter().collect())
}

pub fn difference(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let args = collect_args(vm, pos)?;
    let mut out: crate::util::fx::FxHashSet<Val> = set_clone(vm, recv)?.into_iter().collect();
    for items in args { for v in items { out.remove(&v); } }
    vm.alloc_and_push_set(out.into_iter().collect())
}

pub fn symmetric_difference(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let lhs: crate::util::fx::FxHashSet<Val> = set_clone(vm, recv)?.into_iter().collect();
    let rhs: crate::util::fx::FxHashSet<Val> = iter_to_vec(vm, pos[0])?.into_iter().collect();
    let out: Vec<Val> = lhs.symmetric_difference(&rhs).copied().collect();
    vm.alloc_and_push_set(out)
}

pub fn intersection_update(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let args = collect_args(vm, pos)?;
    set_mut(vm, recv, "intersection_update: receiver is not a set", |set| {
        for items in args {
            let rhs: crate::util::fx::FxHashSet<Val> = items.into_iter().collect();
            set.retain(|v| rhs.contains(v));
        }
        Ok(())
    })?;
    vm.push(Val::none()); Ok(())
}

pub fn difference_update(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let args = collect_args(vm, pos)?;
    set_mut(vm, recv, "difference_update: receiver is not a set", |set| {
        for items in args { for v in items { set.remove(&v); } }
        Ok(())
    })?;
    vm.push(Val::none()); Ok(())
}

pub fn symmetric_difference_update(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let other: crate::util::fx::FxHashSet<Val> = iter_to_vec(vm, pos[0])?.into_iter().collect();
    set_mut(vm, recv, "symmetric_difference_update: receiver is not a set", |set| {
        for v in other { if !set.remove(&v) { set.insert(v); } }
        Ok(())
    })?;
    vm.push(Val::none()); Ok(())
}

pub fn issubset(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let lhs = set_clone(vm, recv)?;
    let rhs: crate::util::fx::FxHashSet<Val> = iter_to_vec(vm, pos[0])?.into_iter().collect();
    vm.push(Val::bool(lhs.iter().all(|v| rhs.contains(v))));
    Ok(())
}

pub fn issuperset(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let lhs: crate::util::fx::FxHashSet<Val> = set_clone(vm, recv)?.into_iter().collect();
    let rhs = iter_to_vec(vm, pos[0])?;
    vm.push(Val::bool(rhs.iter().all(|v| lhs.contains(v))));
    Ok(())
}

pub fn isdisjoint(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let lhs: crate::util::fx::FxHashSet<Val> = set_clone(vm, recv)?.into_iter().collect();
    let rhs = iter_to_vec(vm, pos[0])?;
    vm.push(Val::bool(!rhs.iter().any(|v| lhs.contains(v))));
    Ok(())
}
