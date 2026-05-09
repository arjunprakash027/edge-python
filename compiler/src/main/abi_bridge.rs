use crate::abi::{classify_decode, classify_encode, DecodeBits, EncodeRequest, Op, PrimitiveBytes, TAG_INVALID};
use crate::modules::vm::types::{HeapObj, Val, VmErr};
use crate::modules::vm::handlers::methods::{lookup_method, dispatch_method};
use alloc::{rc::Rc, string::{String, ToString}, vec, vec::Vec};
use core::cell::RefCell;
use crate::s;

use super::{error_stash, get_val, handles, put_val, with_recv, with_vm};
use super::errors::stash_error;

// Universal dispatch. Returns 0 + handle in `*out_handle`, or 1 + stashed error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn host_edge_op(op: u32, recv: u32, name_ptr: *const u8, name_len: u32,argv_ptr: *const u32, argc: u32, out_handle: *mut u32) -> i32 {
    let name = if name_len == 0 { String::new() } else {
        core::str::from_utf8(unsafe {core::slice::from_raw_parts(name_ptr, name_len as usize)}).unwrap_or("").to_string()
    };
    let args: Vec<Val> = (0..argc).filter_map(|i| {
        let h = unsafe { *argv_ptr.add(i as usize) };
        get_val(h)
    }).collect();

    let result: Result<Val, VmErr> = match Op::from_u32(op) {
        Some(Op::Call) => dispatch_call(recv, &name, args),
        Some(Op::GetAttr) => dispatch_get_attr(recv, &name),
        Some(Op::SetAttr) => dispatch_set_attr(recv, &name, &args),
        Some(Op::GetItem) => dispatch_get_item(recv, &args),
        Some(Op::SetItem) => dispatch_set_item(recv, &args),
        Some(Op::Len) => dispatch_len(recv),
        Some(Op::Iter) => dispatch_iter(recv),
        Some(Op::IterNext) => dispatch_iter_next(recv),
        None => Err(VmErr::Raised(s!("edge_op: unsupported op ", int op as i64))),
    };

    match result {
        Ok(v) => { unsafe { *out_handle = put_val(v); } 0 }
        Err(e) => { stash_error(e); 1 }
    }
}

fn dispatch_call(recv_h: u32, name: &str, args: Vec<Val>) -> Result<Val, VmErr> {
    with_recv("edge_op call: invalid receiver handle", recv_h, |vm, recv| {
        let ty = vm.type_name(recv);
        let mid = lookup_method(ty, name)
            .ok_or_else(|| VmErr::Attribute(s!("'", str ty, "' object has no method '", str name, "'")))?;
        let stack_before = vm.stack.len();
        dispatch_method(vm, mid, recv, args, vec![])?;
        if vm.stack.len() != stack_before + 1 {
            return Err(VmErr::Runtime("edge_op call: method left no result"));
        }
        Ok(vm.stack.pop().unwrap())
    })
}

/* GetAttr: module/instance attr, or bind builtin method as BoundMethod. */
fn dispatch_get_attr(recv_h: u32, name: &str) -> Result<Val, VmErr> {
    with_recv("edge_op get_attr: invalid receiver handle", recv_h, |vm, recv| {
        // Module attribute.
        if recv.is_heap()
            && let HeapObj::Module(_, attrs) = vm.heap.get(recv)
        {
            let bare = name;
            if let Some((_, v)) = attrs.iter().find(|(n, _)| n == bare) {
                return Ok(*v);
            }
            return Err(VmErr::Attribute(s!(
                "module has no attribute '", str name, "'")));
        }
        // Instance attribute.
        if recv.is_heap()
            && let HeapObj::Instance(_cls, attrs) = vm.heap.get(recv)
        {
            let entries = attrs.borrow().entries.clone();
            for (k, v) in &entries {
                if k.is_heap()
                    && let HeapObj::Str(s) = vm.heap.get(*k)
                    && s == name
                {
                    return Ok(*v);
                }
            }
            return Err(VmErr::Attribute(s!(
                "instance has no attribute '", str name, "'")));
        }
        // Builtin method -> BoundMethod.
        let ty = vm.type_name(recv);
        if let Some(mid) = lookup_method(ty, name) {
            return vm.heap.alloc(HeapObj::BoundMethod(recv, mid));
        }
        Err(VmErr::Attribute(s!("'", str ty, "' object has no attribute '", str name, "'")))
    })
}

/* SetAttr: writes to instance __dict__; rejects modules and builtins. */
fn dispatch_set_attr(recv_h: u32, name: &str, args: &[Val]) -> Result<Val, VmErr> {
    if args.len() != 1 {
        return Err(VmErr::TypeMsg(s!("set_attr expects exactly 1 value, got ", int args.len() as i64)));
    }
    let value = args[0];
    with_recv("edge_op set_attr: invalid receiver handle", recv_h, |vm, recv| {
        if !recv.is_heap() {
            return Err(VmErr::Type("cannot set attribute on this type"));
        }
        if let HeapObj::Instance(_cls, attrs) = vm.heap.get(recv) {
            let attrs = attrs.clone();
            let key = vm.heap.alloc(HeapObj::Str(name.to_string()))?;
            attrs.borrow_mut().insert(key, value);
            return Ok(Val::none());
        }
        Err(VmErr::Type("cannot set attribute on this type"))
    })
}

/* GetItem: routes through vm.get_item for script-identical semantics. */
fn dispatch_get_item(recv_h: u32, args: &[Val]) -> Result<Val, VmErr> {
    if args.len() != 1 {
        return Err(VmErr::TypeMsg(s!("get_item expects 1 index, got ", int args.len() as i64)));
    }
    let idx = args[0];
    with_recv("edge_op get_item: invalid receiver handle", recv_h, |vm, recv| {
        let stack_before = vm.stack.len();
        vm.push(recv);
        vm.push(idx);
        let _ = vm.get_item()?; // discard the bool (slice-path indicator).
        if vm.stack.len() != stack_before + 1 {
            return Err(VmErr::Runtime("edge_op get_item: get_item left no result"));
        }
        Ok(vm.stack.pop().unwrap())
    })
}

/* SetItem: routes through vm.store_item. */
fn dispatch_set_item(recv_h: u32, args: &[Val]) -> Result<Val, VmErr> {
    if args.len() != 2 {
        return Err(VmErr::TypeMsg(s!("set_item expects (index, value), got ", int args.len() as i64, " args")));
    }
    let idx = args[0];
    let value = args[1];
    with_recv("edge_op set_item: invalid receiver handle", recv_h, |vm, recv| {
        // store_item pops value, idx, container — push in that order.
        vm.push(recv);
        vm.push(idx);
        vm.push(value);
        vm.store_item()?;
        Ok(Val::none())
    })
}

fn dispatch_len(recv_h: u32) -> Result<Val, VmErr> {
    with_recv("edge_op len: invalid receiver handle", recv_h, |vm, recv| {
        let n: i64 = match vm.heap.get(recv) {
            HeapObj::Str(s) => s.chars().count() as i64,
            HeapObj::List(rc) => rc.borrow().len() as i64,
            HeapObj::Dict(rc) => rc.borrow().entries.len() as i64,
            HeapObj::Set(rc) => rc.borrow().len() as i64,
            HeapObj::Tuple(t) => t.len() as i64,
            _ => return Err(VmErr::TypeMsg(s!("object of type '", str vm.type_name(recv), "' has no len()"))),
        };
        Ok(Val::int(n))
    })
}

/* Iter: flatten any iterable into a List for guest GetItem/Len access. */
fn dispatch_iter(recv_h: u32) -> Result<Val, VmErr> {
    with_recv("edge_op iter: invalid receiver handle", recv_h, |vm, recv| {
        let items: Vec<Val> = match vm.heap.get(recv) {
            HeapObj::List(rc) => rc.borrow().clone(),
            HeapObj::Tuple(t) => t.clone(),
            HeapObj::Set(rc) => {
                let mut v: Vec<Val> = rc.borrow().iter().copied().collect();
                vm.sort_set_items(&mut v);
                v
            }
            HeapObj::Dict(rc) => rc.borrow().keys().collect(),
            HeapObj::Range(s, e, st) => {
                let mut out = Vec::new();
                let (mut cur, end, step) = (*s, *e, *st);
                if step > 0 {
                    while cur < end { out.push(Val::int(cur)); cur += step; }
                } else if step < 0 {
                    while cur > end { out.push(Val::int(cur)); cur += step; }
                }
                out
            }
            HeapObj::Str(s) => {
                let chars: Vec<String> = s.chars().map(|c| c.to_string()).collect();
                chars.into_iter()
                    .map(|cs| vm.heap.alloc(HeapObj::Str(cs)))
                    .collect::<Result<Vec<_>, _>>()?
            }
            _ => return Err(VmErr::TypeMsg(s!(
                "object of type '", str vm.type_name(recv), "' is not iterable"))),
        };
        vm.heap.alloc(HeapObj::List(Rc::new(RefCell::new(items))))
    })
}

/* IterNext: pops list head; raises StopIteration when empty. */
fn dispatch_iter_next(recv_h: u32) -> Result<Val, VmErr> {
    with_recv("edge_op iter_next: invalid receiver handle", recv_h, |vm, recv| {
        if let HeapObj::List(rc) = vm.heap.get(recv) {
            let mut v = rc.borrow_mut();
            if v.is_empty() {
                return Err(VmErr::Raised(s!("StopIteration")));
            }
            Ok(v.remove(0))
        } else {
            Err(VmErr::TypeMsg(s!(
                "iter_next expects a List iterator (produced by Op::Iter), got '",
                str vm.type_name(recv), "'")))
        }
    })
}

// Bootstrap decoder: writes tag to `*out_tag`, bytes to `dst[..dst_max]`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn host_edge_encode(tag: u32, ptr: *const u8, len: u32) -> u32 {
    let bytes = if len == 0 || ptr.is_null() {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(ptr, len as usize) }
    };
    match classify_encode(tag, bytes) {
        EncodeRequest::Direct(bits) => put_val(Val(bits)),
        EncodeRequest::AllocStr(s) => {
            let owned = s.to_string();
            let v = with_vm(|vm| vm.heap.alloc(HeapObj::Str(owned)).ok()).flatten();
            match v {
                Some(val) => put_val(val),
                None => 0,
            }
        }
        EncodeRequest::Invalid => 0,
    }
}

// Bootstrap decoder: writes tag to `*out_tag`, bytes to `dst[..dst_max]`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn host_edge_decode(h: u32, out_tag: *mut u32, dst: *mut u8, dst_max: u32) -> i32 {
    let copy_into = |tag: u32, bytes: &[u8]| -> i32 {
        unsafe { *out_tag = tag; }
        if bytes.len() > dst_max as usize { return -(bytes.len() as i32); }
        if !bytes.is_empty() {
            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
            }
        }
        bytes.len() as i32
    };

    let v = match get_val(h) {
        Some(v) => v,
        None => { unsafe { *out_tag = TAG_INVALID; } return 0; }
    };

    match classify_decode(v.0) {
        DecodeBits::Primitive { tag, bytes } => match bytes {
            PrimitiveBytes::None => copy_into(tag, &[]),
            PrimitiveBytes::Bool(b) => copy_into(tag, &[b]),
            PrimitiveBytes::Eight(a) => copy_into(tag, &a),
        },
        DecodeBits::Heap => {
            // Only Str decodes; composites must go through edge_op.
            let result = with_vm(|vm| {
                if let HeapObj::Str(s) = vm.heap.get(v) {
                    Some(s.clone())
                } else { None }
            }).flatten();
            match result {
                Some(s) => copy_into(crate::abi::Tag::Bytes as u32, s.as_bytes()),
                None => { unsafe { *out_tag = TAG_INVALID; } 0 }
            }
        }
        DecodeBits::Invalid => { unsafe { *out_tag = TAG_INVALID; } 0 }
    }
}

// Decrement refcount on a handle. No-op for invalid handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn host_edge_release(h: u32) {
    handles().release(h);
}

// Stash a guest error for the host. Overwrites any pending error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn host_edge_throw(kind: u32, msg_ptr: *const u8, msg_len: u32) {
    let msg = if msg_len == 0 { String::new() } else {
        core::str::from_utf8(unsafe {
            core::slice::from_raw_parts(msg_ptr, msg_len as usize)
        }).unwrap_or("").to_string()
    };
    error_stash().set(kind, msg);
}

// Drain the most recent error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn host_edge_take_error(out_kind: *mut u32, dst: *mut u8, dst_max: u32) -> i32 {
    // Peek first so buffer-too-small callers can retry.
    let stash = error_stash();
    let (kind, len) = match stash.peek() {
        Some((k, m)) => (k, m.len()),
        None => return -1,
    };
    if len > dst_max as usize { return -(len as i32); }
    // Buffer fits — drain and copy.
    let (_, msg) = stash.take().expect("peek returned Some");
    let bytes = msg.as_bytes();
    unsafe {
        *out_kind = kind;
        if !bytes.is_empty() {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        }
    }
    bytes.len() as i32
}
