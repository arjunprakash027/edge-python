use super::{DictMap, HeapObj, HeapPool, Val, as_i128};

pub(in crate::modules::vm) fn eq_seq(a: &[Val], b: &[Val], eq: impl Fn(Val,Val)->bool) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x,y)| eq(*x,*y))
}
pub(in crate::modules::vm) fn eq_dict(a: &DictMap, b: &DictMap, eq: impl Fn(Val,Val)->bool) -> bool {
    a.len() == b.len() && a.iter().all(|(k,v)| b.get(&k).is_some_and(|&v2| eq(v,v2)))
}

/* Recursion cap so self-referential containers fall back instead of overflowing the stack. */
const EQ_DEPTH_MAX: usize = 100;

pub fn eq_vals_with_heap(a: Val, b: Val, heap: &HeapPool) -> bool {
    eq_vals_depth(a, b, heap, 0)
}

/* f64 view of any numeric Val (int/bool/float/LongInt); None for non-numerics. */
fn num_as_f64(v: Val, heap: &HeapPool) -> Option<f64> {
    if v.is_float() { Some(v.as_float()) }
    else if v.is_int() { Some(v.as_int() as f64) }
    else if v.is_bool() { Some(v.as_bool() as i64 as f64) }
    else if v.is_heap() { if let HeapObj::LongInt(i) = heap.get(v) { Some(*i as f64) } else { None } }
    else { None }
}

fn eq_vals_depth(a: Val, b: Val, heap: &HeapPool, depth: usize) -> bool {
    // Past the cap fall back to identity; cyclic structures terminate.
    if depth > EQ_DEPTH_MAX { return a.0 == b.0; }

    // Unify all int-flavoured pairs through i128 (LongInt, inline int, bool).
    if let (Some(ai), Some(bi)) = (as_i128(a, heap), as_i128(b, heap)) {
        return ai == bi;
    }

    // One side is a float here (all-integer handled above): compare numerically so float unifies with int/bool/LongInt, e.g. `1.0 == True`, `1e16 == 10**16`.
    if let (Some(af), Some(bf)) = (num_as_f64(a, heap), num_as_f64(b, heap)) {
        return af == bf;
    }

    if !a.is_heap() || !b.is_heap() {
        return a.0 == b.0;
    }

    // A heap object equals itself; short-circuits self-referential containers before the element walk.
    if a.0 == b.0 { return true; }

    let d = depth + 1;
    match (heap.get(a), heap.get(b)) {
        (HeapObj::Str(x), HeapObj::Str(y)) => x == y,
        (HeapObj::Bytes(x), HeapObj::Bytes(y)) => x == y,
        (HeapObj::Tuple(x), HeapObj::Tuple(y)) => eq_seq(x, y, |a,b| eq_vals_depth(a, b, heap, d)),
        (HeapObj::List(x), HeapObj::List(y)) => eq_seq(&x.borrow(), &y.borrow(), |a,b| eq_vals_depth(a, b, heap, d)),
        (HeapObj::Set(x), HeapObj::Set(y)) => *x.borrow() == *y.borrow(),
        (HeapObj::FrozenSet(x), HeapObj::FrozenSet(y)) => **x == **y,
        (HeapObj::Set(x), HeapObj::FrozenSet(y)) => *x.borrow() == **y,
        (HeapObj::FrozenSet(x), HeapObj::Set(y)) => **x == *y.borrow(),
        (HeapObj::Dict(x), HeapObj::Dict(y)) => eq_dict(&x.borrow(), &y.borrow(), |a,b| eq_vals_depth(a, b, heap, d)),
        (HeapObj::Type(x), HeapObj::Type(y)) => x == y, // by name; interning also makes `is` hold
        // Cross-type comparisons fall through to false. Notably `bytes == str` is False, even when the bytes are valid UTF-8 of the str.
        _ => false,
    }
}
