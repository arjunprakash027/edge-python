/* Python-compatible f-string format spec engine.

   Mini-language (subset of PEP 3101 sufficient for typical numeric/string
   formatting): `[[fill]align][sign][#][0][width][,][.precision][type]`.

   Supported types:
     d/none — decimal int
     b/o/x/X — base 2/8/16
     f/F/e/E — fixed / exponential float
     g/G    — general (Rust default for floats)
     %      — percentage (multiply by 100, append '%')
     s      — string (only valid for str values)
     c      — int → unicode codepoint

   Layout features:
     fill+align (`<`, `>`, `^`)
     `0` zero-pad shortcut
     width (digits)
     `,` thousands separator (decimal int / float)
     `.precision` (float precision, or str truncation)
     sign (`+`, `-`, ` `)

   Returns Ok(formatted) or Err(message) so the caller can raise ValueError. */

use alloc::string::{String, ToString};
use crate::modules::vm::types::{Val, HeapObj, HeapPool, BigInt};

pub fn format_value(v: Val, spec: &str, heap: &HeapPool) -> Result<String, &'static str> {
    if spec.is_empty() {
        return Ok(display_inline(v, heap));
    }
    let parsed = parse_spec(spec)?;
    apply(v, &parsed, heap)
}

#[derive(Default)]
struct Spec {
    fill: char,
    align: Option<u8>,   // b'<' b'>' b'^' b'='
    sign: u8,            // 0, b'+', b'-', b' '
    zero_pad: bool,
    width: usize,
    thousands: bool,
    precision: Option<usize>,
    ty: u8,              // 0 means default
}

fn parse_spec(spec: &str) -> Result<Spec, &'static str> {
    let bytes = spec.as_bytes();
    let mut s = Spec { fill: ' ', ..Spec::default() };
    let mut i = 0;

    /* fill+align is "<char><align>" only when char #2 is one of `<>^=`. */
    if bytes.len() >= 2 && matches!(bytes[1], b'<' | b'>' | b'^' | b'=') {
        s.fill = bytes[0] as char;
        s.align = Some(bytes[1]);
        i = 2;
    } else if !bytes.is_empty() && matches!(bytes[0], b'<' | b'>' | b'^' | b'=') {
        s.align = Some(bytes[0]);
        i = 1;
    }

    if i < bytes.len() && matches!(bytes[i], b'+' | b'-' | b' ') {
        s.sign = bytes[i];
        i += 1;
    }

    /* '#' alternate form is parsed but ignored — emitted by Python for `0b…`
       prefixes. We emit prefixes unconditionally for `b/o/x` so behaviour
       matches `bin()`/`hex()` builtins. */
    if i < bytes.len() && bytes[i] == b'#' { i += 1; }

    if i < bytes.len() && bytes[i] == b'0' {
        s.zero_pad = true;
        if s.align.is_none() {
            s.align = Some(b'=');
            s.fill = '0';
        }
        i += 1;
    }

    let w_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
    if i > w_start {
        s.width = spec[w_start..i].parse().map_err(|_| "invalid width in format spec")?;
    }

    if i < bytes.len() && bytes[i] == b',' {
        s.thousands = true;
        i += 1;
    }

    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let p_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
        if i == p_start { return Err("missing precision in format spec"); }
        s.precision = Some(spec[p_start..i].parse().map_err(|_| "invalid precision in format spec")?);
    }

    if i < bytes.len() {
        if !bytes[i].is_ascii() { return Err("invalid type in format spec"); }
        s.ty = bytes[i];
        i += 1;
    }

    if i != bytes.len() { return Err("trailing characters in format spec"); }
    Ok(s)
}

fn apply(v: Val, s: &Spec, heap: &HeapPool) -> Result<String, &'static str> {
    /* Dispatch by inferred kind: integer types route through int formatters;
       float types coerce ints up; string types accept strings only. */
    match s.ty {
        0 | b's' => {
            if v.is_int() || v.is_float() || v.is_bool() || v.is_none()
                || (v.is_heap() && matches!(heap.get(v), HeapObj::BigInt(_)))
            {
                if s.ty == b's' { return Err("'s' format spec requires a string"); }
                /* No type char + numeric: format like default unless precision/
                   thousands present, in which case treat as float-general. */
                if s.precision.is_some() || s.thousands {
                    return format_float(v, s, b'g', heap);
                }
                if v.is_int() {
                    return Ok(pad_numeric(s, &itoa_str(v.as_int())));
                }
                if v.is_heap() && let HeapObj::BigInt(b) = heap.get(v) {
                    return Ok(pad_numeric(s, &b.to_decimal()));
                }
                if v.is_float() {
                    return format_float(v, s, b'g', heap);
                }
            }
            let raw = display_inline(v, heap);
            let truncated = match s.precision {
                Some(p) => raw.chars().take(p).collect::<String>(),
                None => raw,
            };
            Ok(pad_string(s, &truncated))
        }
        b'd' | b'b' | b'o' | b'x' | b'X' => format_int(v, s, heap),
        b'c' => {
            let i = require_int(v, heap)?;
            let cp = u32::try_from(i).map_err(|_| "%c arg not in range(0x110000)")?;
            let ch = char::from_u32(cp).ok_or("%c arg not in range(0x110000)")?;
            Ok(pad_string(s, &ch.to_string()))
        }
        b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'%' => format_float(v, s, s.ty, heap),
        _ => Err("unknown format type"),
    }
}

fn format_int(v: Val, s: &Spec, heap: &HeapPool) -> Result<String, &'static str> {
    let (neg, mag) = int_to_decimal_parts(v, heap)?;
    let (digits, prefix): (String, &'static str) = match s.ty {
        b'd' => (mag, ""),
        b'b' => (decimal_to_radix(&mag, 2), "0b"),
        b'o' => (decimal_to_radix(&mag, 8), "0o"),
        b'x' => (decimal_to_radix(&mag, 16), "0x"),
        b'X' => (decimal_to_radix(&mag, 16).to_uppercase(), "0X"),
        _ => unreachable!(),
    };
    let body = if s.thousands && s.ty == b'd' { add_thousands(&digits) } else { digits };
    let sign_ch = sign_char(neg, s.sign);
    let mut left = String::new();
    if let Some(c) = sign_ch { left.push(c); }
    left.push_str(prefix);
    left.push_str(&body);
    Ok(pad_aligned(s, &left, sign_ch.map(|_| 1).unwrap_or(0) + prefix.len()))
}

fn format_float(v: Val, s: &Spec, ty: u8, heap: &HeapPool) -> Result<String, &'static str> {
    let f = require_float(v, heap)?;
    let prec = s.precision.unwrap_or(6);

    /* NaN/inf go through unchanged (Python emits "nan"/"inf" before padding). */
    if f.is_nan() {
        let body = if matches!(ty, b'F' | b'E' | b'G') { "NAN" } else { "nan" };
        return Ok(pad_string(s, body));
    }
    if f.is_infinite() {
        let mut out = String::new();
        let sign_ch = sign_char(f.is_sign_negative(), s.sign);
        if let Some(c) = sign_ch { out.push(c); }
        out.push_str(if matches!(ty, b'F' | b'E' | b'G') { "INF" } else { "inf" });
        return Ok(pad_aligned(s, &out, sign_ch.map(|_| 1).unwrap_or(0)));
    }

    let mag = f.abs();
    let body = match ty {
        b'%' => fixed(mag * 100.0, prec) + "%",
        b'f' | b'F' => fixed(mag, prec),
        b'e' => exp(mag, prec, false),
        b'E' => exp(mag, prec, true).to_uppercase(),
        b'g' | b'G' => {
            let g = general(mag, if s.precision.is_some() { prec } else { 6 });
            if ty == b'G' { g.to_uppercase() } else { g }
        }
        _ => unreachable!(),
    };
    let body = if s.thousands { add_thousands_float(&body) } else { body };
    let sign_ch = sign_char(f.is_sign_negative(), s.sign);
    let mut left = String::new();
    if let Some(c) = sign_ch { left.push(c); }
    left.push_str(&body);
    Ok(pad_aligned(s, &left, sign_ch.map(|_| 1).unwrap_or(0)))
}

fn require_int(v: Val, heap: &HeapPool) -> Result<i64, &'static str> {
    if v.is_int() { return Ok(v.as_int()); }
    if v.is_bool() { return Ok(v.as_bool() as i64); }
    if v.is_heap() && let HeapObj::BigInt(b) = heap.get(v) {
        return b.to_i64_checked().ok_or("integer too large for format type");
    }
    Err("format spec requires an integer")
}

fn require_float(v: Val, heap: &HeapPool) -> Result<f64, &'static str> {
    if v.is_float() { return Ok(v.as_float()); }
    if v.is_int() { return Ok(v.as_int() as f64); }
    if v.is_bool() { return Ok(v.as_bool() as i64 as f64); }
    if v.is_heap() && let HeapObj::BigInt(b) = heap.get(v) { return Ok(b.to_f64()); }
    Err("format spec requires a number")
}

fn int_to_decimal_parts(v: Val, heap: &HeapPool) -> Result<(bool, String), &'static str> {
    if v.is_int() {
        let i = v.as_int();
        let neg = i < 0;
        let mag = if i == i64::MIN {
            BigInt::from_i64(i).abs().to_decimal()
        } else {
            let mut b = itoa::Buffer::new();
            b.format(i.unsigned_abs()).to_string()
        };
        return Ok((neg, mag));
    }
    if v.is_bool() { return Ok((false, itoa_str(v.as_bool() as i64))); }
    if v.is_heap() && let HeapObj::BigInt(b) = heap.get(v) {
        let s = b.to_decimal();
        if let Some(rest) = s.strip_prefix('-') { return Ok((true, rest.to_string())); }
        return Ok((false, s));
    }
    Err("format spec requires an integer")
}

fn decimal_to_radix(mag: &str, radix: u32) -> String {
    /* Convert a non-negative decimal magnitude string to the given radix.
       Goes through BigInt to handle values beyond i64. */
    if mag == "0" { return String::from("0"); }
    let mut bi = BigInt::from_decimal(mag);
    let r = BigInt::from_i64(radix as i64);
    let mut out = String::new();
    while !bi.is_zero() {
        let (q, rem) = bi.divmod(&r).unwrap();
        let d = rem.to_i64_checked().unwrap() as u32;
        out.push(core::char::from_digit(d, radix).unwrap());
        bi = q;
    }
    out.chars().rev().collect()
}

fn add_thousands(digits: &str) -> String {
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    let chars: alloc::vec::Vec<char> = digits.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && (chars.len() - i) % 3 == 0 { out.push(','); }
        out.push(c);
    }
    out
}

fn add_thousands_float(s: &str) -> String {
    /* Insert separators only in the integer portion (before `.` / `e`). */
    let split = s.find(|c: char| c == '.' || c == 'e' || c == 'E');
    let (int_part, rest) = match split {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    };
    let mut out = add_thousands(int_part);
    out.push_str(rest);
    out
}

fn sign_char(neg: bool, sign_flag: u8) -> Option<char> {
    if neg { Some('-') }
    else if sign_flag == b'+' { Some('+') }
    else if sign_flag == b' ' { Some(' ') }
    else { None }
}

fn pad_numeric(s: &Spec, body: &str) -> String {
    let (neg, mag) = if let Some(rest) = body.strip_prefix('-') { (true, rest) } else { (false, body) };
    let sign_ch = sign_char(neg, s.sign);
    let mut left = String::new();
    if let Some(c) = sign_ch { left.push(c); }
    left.push_str(mag);
    pad_aligned(s, &left, sign_ch.map(|_| 1).unwrap_or(0))
}

fn pad_string(s: &Spec, body: &str) -> String {
    let len = body.chars().count();
    if len >= s.width { return body.to_string(); }
    let pad = s.width - len;
    let align = s.align.unwrap_or(b'<');
    pad_with(body, pad, align, s.fill, 0)
}

fn pad_aligned(s: &Spec, body: &str, sign_prefix_len: usize) -> String {
    let len = body.chars().count();
    if len >= s.width { return body.to_string(); }
    let pad = s.width - len;
    let align = s.align.unwrap_or(b'>');
    pad_with(body, pad, align, s.fill, sign_prefix_len)
}

fn pad_with(body: &str, pad: usize, align: u8, fill: char, sign_prefix_len: usize) -> String {
    let mut out = String::new();
    match align {
        b'<' => {
            out.push_str(body);
            for _ in 0..pad { out.push(fill); }
        }
        b'>' => {
            for _ in 0..pad { out.push(fill); }
            out.push_str(body);
        }
        b'^' => {
            let l = pad / 2;
            let r = pad - l;
            for _ in 0..l { out.push(fill); }
            out.push_str(body);
            for _ in 0..r { out.push(fill); }
        }
        b'=' => {
            /* Sign-aware: insert padding between the sign/prefix and the digits. */
            let mut chars = body.chars();
            for _ in 0..sign_prefix_len {
                if let Some(c) = chars.next() { out.push(c); }
            }
            for _ in 0..pad { out.push(fill); }
            out.push_str(chars.as_str());
        }
        _ => unreachable!(),
    }
    out
}

fn fixed(mag: f64, prec: usize) -> String {
    /* Round-half-to-even to a given decimal precision, then render as
       integer-part '.' fractional-part. Avoids alloc::format!'s %f. */
    let scale = pow10(prec);
    let scaled = mag * scale;
    let rounded = if scaled.fract().abs() == 0.5 {
        let f = scaled.trunc();
        if (f as i64) % 2 == 0 { f } else if scaled > 0.0 { f + 1.0 } else { f - 1.0 }
    } else {
        (scaled + 0.5 * scaled.signum()).trunc()
    };
    let bi = BigInt::from_decimal(&u128_to_dec(rounded.abs() as u128));
    let dec = bi.to_decimal();
    if prec == 0 { return dec; }
    /* Pad on the left so we can insert a `.` exactly `prec` chars from the end. */
    let needed = prec + 1;
    let padded = if dec.len() < needed {
        let mut s = String::with_capacity(needed);
        for _ in 0..(needed - dec.len()) { s.push('0'); }
        s.push_str(&dec); s
    } else { dec };
    let dot = padded.len() - prec;
    let mut out = String::with_capacity(padded.len() + 1);
    out.push_str(&padded[..dot]);
    out.push('.');
    out.push_str(&padded[dot..]);
    out
}

fn exp(mag: f64, prec: usize, _upper: bool) -> String {
    if mag == 0.0 {
        let mut o = String::from("0");
        if prec > 0 {
            o.push('.');
            for _ in 0..prec { o.push('0'); }
        }
        o.push_str("e+00");
        return o;
    }
    let exponent = mag.log10().floor() as i32;
    let mantissa = mag / pow10_i(exponent);
    let mant_str = fixed(mantissa, prec);
    let mut o = mant_str;
    o.push('e');
    if exponent >= 0 { o.push('+'); } else { o.push('-'); }
    let abs = exponent.unsigned_abs();
    if abs < 10 { o.push('0'); }
    let mut b = itoa::Buffer::new();
    o.push_str(b.format(abs));
    o
}

fn general(mag: f64, prec: usize) -> String {
    /* CPython's `g`: switch to scientific when exp < -4 or >= prec.
       Trailing zeros are stripped (and trailing `.`). */
    let p = prec.max(1);
    if mag == 0.0 {
        return String::from("0");
    }
    let exponent = mag.log10().floor() as i32;
    let body = if exponent < -4 || exponent >= p as i32 {
        exp(mag, p - 1, false)
    } else {
        let frac_digits = (p as i32 - 1 - exponent).max(0) as usize;
        fixed(mag, frac_digits)
    };
    strip_trailing(&body)
}

fn strip_trailing(s: &str) -> String {
    /* Strip trailing zeros (and a trailing `.`) from the mantissa portion
       only. Anything after `e` is left alone. */
    let split = s.find('e');
    let (mant, tail) = match split {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    };
    let mut m = mant.to_string();
    if m.contains('.') {
        while m.ends_with('0') { m.pop(); }
        if m.ends_with('.') { m.pop(); }
    }
    m.push_str(tail);
    m
}

fn pow10(n: usize) -> f64 {
    let mut r = 1.0f64;
    for _ in 0..n { r *= 10.0; }
    r
}
fn pow10_i(n: i32) -> f64 {
    if n >= 0 { pow10(n as usize) } else { 1.0 / pow10((-n) as usize) }
}
fn itoa_str(i: i64) -> String {
    let mut b = itoa::Buffer::new(); b.format(i).to_string()
}
fn u128_to_dec(n: u128) -> String {
    if n == 0 { return String::from("0"); }
    let mut out = String::new();
    let mut x = n;
    while x > 0 { out.push((b'0' + (x % 10) as u8) as char); x /= 10; }
    out.chars().rev().collect()
}

/* Plain str() rendering inlined here so the hot path doesn't borrow from VM. */
pub fn display_inline(v: Val, heap: &HeapPool) -> String {
    if v.is_int() {
        let mut b = itoa::Buffer::new();
        return b.format(v.as_int()).to_string();
    }
    if v.is_bool() { return (if v.as_bool() { "True" } else { "False" }).to_string(); }
    if v.is_none() { return String::from("None"); }
    if v.is_float() { return crate::modules::fstr::format_f64(v.as_float()); }
    if v.is_heap()
        && let HeapObj::Str(s) = heap.get(v) { return s.clone(); }
    if v.is_heap()
        && let HeapObj::BigInt(b) = heap.get(v) { return b.to_decimal(); }
    /* Fall back to nothing — caller should use VM::display for full coverage. */
    String::new()
}
