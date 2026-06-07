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

pub fn update(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let items = iter_to_vec(vm, pos[0])?;
    set_mut(vm, recv, "update: receiver is not a set", |set| {
        for v in items { set.insert(v); }
        Ok(())
    })?;
    vm.push(Val::none()); Ok(())
}

pub fn copy(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let items = set_clone(vm, recv)?;
    vm.alloc_and_push_set(items)
}

pub fn union(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let mut out = set_clone(vm, recv)?;
    out.extend(iter_to_vec(vm, pos[0])?);
    vm.alloc_and_push_set(out)
}

pub fn intersection(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let lhs = set_clone(vm, recv)?;
    let rhs_items = iter_to_vec(vm, pos[0])?;
    let rhs: crate::util::fx::FxHashSet<Val> = rhs_items.into_iter().collect();
    let out: Vec<Val> = lhs.into_iter().filter(|v| rhs.contains(v)).collect();
    vm.alloc_and_push_set(out)
}

pub fn difference(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let lhs = set_clone(vm, recv)?;
    let rhs_items = iter_to_vec(vm, pos[0])?;
    let rhs: crate::util::fx::FxHashSet<Val> = rhs_items.into_iter().collect();
    let out: Vec<Val> = lhs.into_iter().filter(|v| !rhs.contains(v)).collect();
    vm.alloc_and_push_set(out)
}

pub fn symmetric_difference(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let lhs: crate::util::fx::FxHashSet<Val> = set_clone(vm, recv)?.into_iter().collect();
    let rhs: crate::util::fx::FxHashSet<Val> = iter_to_vec(vm, pos[0])?.into_iter().collect();
    let out: Vec<Val> = lhs.symmetric_difference(&rhs).copied().collect();
    vm.alloc_and_push_set(out)
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
