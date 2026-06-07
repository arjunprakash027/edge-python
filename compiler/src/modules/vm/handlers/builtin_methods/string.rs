/*
Built-in methods for `str` receivers. Arity is checked by the dispatcher.
*/

use super::prelude::*;

// `str.encode([encoding])`, UTF-8/ASCII only; other names error to block silent mismatches.
pub fn encode(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
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
}

// str: zero-arg transforms `recv_str -> f -> push`.
macro_rules! str_transform {
    ($name:ident, $f:expr) => {
        pub fn $name(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
            let s = recv_str(vm, recv)?;
            vm.alloc_and_push_str($f(s.as_str()))
        }
    };
}
str_transform!(upper, |s: &str| s.to_uppercase());
str_transform!(lower, |s: &str| s.to_lowercase());
str_transform!(capitalize, capitalize_first);
str_transform!(title, title_case);

// str.strip / lstrip / rstrip: trim whitespace, or any char in the optional arg.
enum Trim { Both, Start, End }
fn strip_impl(vm: &mut VM, recv: Val, pos: &[Val], mode: Trim) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let out = if pos.is_empty() {
        match mode { Trim::Both => s.trim(), Trim::Start => s.trim_start(), Trim::End => s.trim_end() }.to_string()
    } else {
        let p = val_to_str(vm, pos[0])?;
        let f = |c: char| p.contains(c);
        match mode { Trim::Both => s.trim_matches(f), Trim::Start => s.trim_start_matches(f), Trim::End => s.trim_end_matches(f) }.to_string()
    };
    vm.alloc_and_push_str(out)
}
pub fn strip(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> { strip_impl(vm, recv, pos, Trim::Both) }
pub fn lstrip(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> { strip_impl(vm, recv, pos, Trim::Start) }
pub fn rstrip(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> { strip_impl(vm, recv, pos, Trim::End) }

pub fn isdigit(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    vm.push(Val::bool(!s.is_empty() && s.chars().all(|c| c.is_ascii_digit())));
    Ok(())
}

pub fn isalpha(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    vm.push(Val::bool(!s.is_empty() && s.chars().all(|c| c.is_alphabetic())));
    Ok(())
}

pub fn isalnum(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    vm.push(Val::bool(!s.is_empty() && s.chars().all(|c| c.is_alphanumeric())));
    Ok(())
}

pub fn startswith(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let p = val_to_str(vm, pos[0])?;
    vm.push(Val::bool(s.starts_with(p.as_str())));
    Ok(())
}

pub fn endswith(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let p = val_to_str(vm, pos[0])?;
    vm.push(Val::bool(s.ends_with(p.as_str())));
    Ok(())
}

pub fn find(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let sub = val_to_str(vm, pos[0])?;
    let idx = s.find(sub.as_str())
        .map(|i| s[..i].chars().count() as i64)
        .unwrap_or(-1);
    vm.push(Val::int(idx));
    Ok(())
}

pub fn count(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let sub = val_to_str(vm, pos[0])?;
    vm.push(Val::int(s.matches(sub.as_str()).count() as i64));
    Ok(())
}

pub fn split(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
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
}

pub fn join(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let sep = recv_str(vm, recv)?;
    // `try_get` is panic-free: an inline int arg would make `heap.get` index a bogus slot and abort.
    let items = match vm.heap.try_get(pos[0]) {
        Some(HeapObj::List(rc)) => rc.borrow().clone(),
        Some(HeapObj::Tuple(v)) => v.clone(),
        _ => return Err(cold_type("join() argument must be iterable")),
    };
    let mut parts: Vec<String> = Vec::with_capacity(items.len());
    for v in items { parts.push(val_to_str(vm, v)?); }
    vm.alloc_and_push_str(parts.join(sep.as_str()))
}

pub fn replace(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let old = val_to_str(vm, pos[0])?;
    let new = val_to_str(vm, pos[1])?;
    vm.alloc_and_push_str(s.replace(old.as_str(), new.as_str()))
}

// `str.removeprefix` / `removesuffix`, strip if present, else return unchanged.
pub fn removeprefix(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let p = val_to_str(vm, pos[0])?;
    let out = s.strip_prefix(p.as_str()).map(|t| t.to_string()).unwrap_or(s);
    vm.alloc_and_push_str(out)
}

pub fn removesuffix(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let suf = val_to_str(vm, pos[0])?;
    let out = s.strip_suffix(suf.as_str()).map(|t| t.to_string()).unwrap_or(s);
    vm.alloc_and_push_str(out)
}

// `str.splitlines()`, split on \n / \r / \r\n, dropping the separator (keepends=False).
pub fn splitlines(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
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
}

// `str.partition` / `rpartition`, (head, sep, tail); on miss returns (s,"","") / ("","",s).
fn partition_impl(vm: &mut VM, recv: Val, pos: &[Val], from_right: bool) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let sep = val_to_str(vm, pos[0])?;
    if sep.is_empty() { return Err(cold_value("empty separator")); }
    let hit = if from_right { s.rfind(sep.as_str()) } else { s.find(sep.as_str()) };
    let (a, b, c): (String, String, String) = match hit {
        Some(i) => (s[..i].to_string(), sep.clone(), s[i + sep.len()..].to_string()),
        None if from_right => (String::new(), String::new(), s), // miss: original at the tail
        None => (s, String::new(), String::new()), // miss: original at the head
    };
    let av = vm.heap.alloc(HeapObj::Str(a))?;
    let bv = vm.heap.alloc(HeapObj::Str(b))?;
    let cv = vm.heap.alloc(HeapObj::Str(c))?;
    vm.alloc_and_push_tuple(vec![av, bv, cv])
}
pub fn partition(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> { partition_impl(vm, recv, pos, false) }
pub fn rpartition(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> { partition_impl(vm, recv, pos, true) }

// str: padding.
pub fn center(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    if !pos[0].is_int() { return Err(cold_type("center() width must be an integer")); }
    let width = pos[0].as_int().max(0) as usize;
    // User-controlled width drives the output size; cap it so a huge value errors instead of aborting in the allocator.
    if width > vm.heap.limit() { return Err(cold_heap()); }
    let fill = if pos.len() > 1 {
        val_to_str(vm, pos[1])?.chars().next().unwrap_or(' ')
    } else { ' ' };
    // Padding measured in code points, not UTF-8 bytes (Unicode parity).
    let pad = width.saturating_sub(s.chars().count());
    let left = pad / 2;
    let right = pad - left;
    let out = fill.to_string().repeat(left) + &s + &fill.to_string().repeat(right);
    vm.alloc_and_push_str(out)
}

pub fn zfill(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    if !pos[0].is_int() { return Err(cold_type("zfill() requires an integer argument")); }
    let s = recv_str(vm, recv)?;
    let width = pos[0].as_int().max(0) as usize;
    // User-controlled width drives the output size; cap it so a huge value errors instead of aborting in the allocator.
    if width > vm.heap.limit() { return Err(cold_heap()); }
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
    vm.alloc_and_push_str(out)
}
