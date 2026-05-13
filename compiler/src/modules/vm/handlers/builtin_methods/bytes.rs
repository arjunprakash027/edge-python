/*
Built-in methods for `bytes` receivers. Arity is checked by the dispatcher.
*/

use super::prelude::*;

// `bytes.decode([encoding])` — invalid UTF-8 errors as ValueError.
pub fn decode(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
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
}

// `bytes.hex()` — lowercase hex of every byte. No separator.
pub fn hex(vm: &mut VM, recv: Val, _pos: &[Val]) -> Result<(), VmErr> {
    let buf = recv_bytes(vm, recv)?;
    let mut out = alloc::string::String::with_capacity(buf.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &b in &buf {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    let v = vm.heap.alloc(HeapObj::Str(out))?;
    vm.push(v); Ok(())
}

// bytes-only; strings go through `string::startswith`.
pub fn startswith(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let buf = recv_bytes(vm, recv)?;
    let prefix = recv_bytes(vm, pos[0])?;
    vm.push(Val::bool(buf.starts_with(&prefix)));
    Ok(())
}

pub fn endswith(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let buf = recv_bytes(vm, recv)?;
    let suffix = recv_bytes(vm, pos[0])?;
    vm.push(Val::bool(buf.ends_with(&suffix)));
    Ok(())
}

pub fn find(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let buf = recv_bytes(vm, recv)?;
    let sub = recv_bytes(vm, pos[0])?;
    let idx = buf.windows(sub.len()).position(|w| w == sub.as_slice()).map(|i| i as i64).unwrap_or(-1);
    vm.push(Val::int(idx));
    Ok(())
}

pub fn index(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
    let buf = recv_bytes(vm, recv)?;
    let sub = recv_bytes(vm, pos[0])?;
    let idx = buf.windows(sub.len()).position(|w| w == sub.as_slice()).ok_or(cold_value("subsection not found"))?;
    vm.push(Val::int(idx as i64));
    Ok(())
}

pub fn count(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
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
}

pub fn replace(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
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
}

pub fn split(vm: &mut VM, recv: Val, pos: &[Val]) -> Result<(), VmErr> {
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
}
