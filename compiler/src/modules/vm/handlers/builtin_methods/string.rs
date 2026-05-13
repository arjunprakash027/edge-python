/*
Built-in methods for `str` receivers. Arity is checked by the dispatcher.
*/

use super::prelude::*;

// `str.encode([encoding])` — UTF-8/ASCII only; other names error to block silent mismatches.
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

// str: zero-arg transforms.
pub fn upper(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let v = vm.heap.alloc(HeapObj::Str(s.to_uppercase()))?;
    vm.push(v); Ok(())
}

pub fn lower(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let v = vm.heap.alloc(HeapObj::Str(s.to_lowercase()))?;
    vm.push(v); Ok(())
}

pub fn strip(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let out = if pos.is_empty() {
        s.trim().to_string()
    } else {
        let p = val_to_str(vm, pos[0])?;
        s.trim_matches(|c| p.contains(c)).to_string()
    };
    let v = vm.heap.alloc(HeapObj::Str(out))?;
    vm.push(v); Ok(())
}

pub fn capitalize(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let v = vm.heap.alloc(HeapObj::Str(capitalize_first(&s)))?;
    vm.push(v); Ok(())
}

pub fn title(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let v = vm.heap.alloc(HeapObj::Str(title_case(&s)))?;
    vm.push(v); Ok(())
}

pub fn lstrip(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let out = if pos.is_empty() {
        s.trim_start().to_string()
    } else {
        let p = val_to_str(vm, pos[0])?;
        s.trim_start_matches(|c| p.contains(c)).to_string()
    };
    let v = vm.heap.alloc(HeapObj::Str(out))?;
    vm.push(v); Ok(())
}

pub fn rstrip(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let out = if pos.is_empty() {
        s.trim_end().to_string()
    } else {
        let p = val_to_str(vm, pos[0])?;
        s.trim_end_matches(|c| p.contains(c)).to_string()
    };
    let v = vm.heap.alloc(HeapObj::Str(out))?;
    vm.push(v); Ok(())
}

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
    let items = match vm.heap.get(pos[0]) {
        HeapObj::List(rc) => rc.borrow().clone(),
        HeapObj::Tuple(v) => v.clone(),
        _ => return Err(cold_type("join() argument must be iterable")),
    };
    let mut parts: Vec<String> = Vec::with_capacity(items.len());
    for v in items { parts.push(val_to_str(vm, v)?); }
    let v = vm.heap.alloc(HeapObj::Str(parts.join(sep.as_str())))?;
    vm.push(v); Ok(())
}

pub fn replace(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let old = val_to_str(vm, pos[0])?;
    let new = val_to_str(vm, pos[1])?;
    let v = vm.heap.alloc(HeapObj::Str(s.replace(old.as_str(), new.as_str())))?;
    vm.push(v); Ok(())
}

// `str.removeprefix` / `removesuffix` — strip if present, else return unchanged.
pub fn removeprefix(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let p = val_to_str(vm, pos[0])?;
    let out = s.strip_prefix(p.as_str()).map(|t| t.to_string()).unwrap_or(s);
    let v = vm.heap.alloc(HeapObj::Str(out))?;
    vm.push(v); Ok(())
}

pub fn removesuffix(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let s = recv_str(vm, recv)?;
    let suf = val_to_str(vm, pos[0])?;
    let out = s.strip_suffix(suf.as_str()).map(|t| t.to_string()).unwrap_or(s);
    let v = vm.heap.alloc(HeapObj::Str(out))?;
    vm.push(v); Ok(())
}

// `str.splitlines()` — split on \n / \r / \r\n, dropping the separator (keepends=False).
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

// `str.partition` / `rpartition` — (head, sep, tail); on miss returns (s,"","") / ("","",s).
pub fn partition(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
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
}

pub fn rpartition(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
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
}

// str: padding.
pub fn center(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
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
}

pub fn zfill(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
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
}
