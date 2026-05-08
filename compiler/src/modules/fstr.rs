/* Format an f64 to a Python-display string (NaN, ±inf, ±0.0,
   whole-number floats with trailing ".0", else Rust's default). */
pub fn format_f64(f: f64) -> alloc::string::String {
    if f.is_nan() { return alloc::string::String::from("NaN"); }
    if f == f64::INFINITY { return alloc::string::String::from("inf"); }
    if f == f64::NEG_INFINITY { return alloc::string::String::from("-inf"); }
    if f == 0.0 {
        return if f.is_sign_negative() { alloc::string::String::from("-0.0") } else { alloc::string::String::from("0.0") };
    }

    // Whole-number floats: itoa + ".0" avoids Rust's "1" output for 1.0.
    const I64_UPPER: f64 = i64::MAX as f64;
    if f.is_finite() && f >= (i64::MIN as f64) && f < I64_UPPER && f == (f as i64) as f64 {
        let mut b = itoa::Buffer::new();
        let s = b.format(f as i64);
        let mut out = alloc::string::String::with_capacity(s.len() + 2);
        out.push_str(s);
        out.push_str(".0");
        return out;
    }

    format_general(f)
}

/* `ryu` produces the shortest decimal that round-trips through
   `parse::<f64>()`, identical guarantees to `core::fmt::Display for f64`,
   but skips ~30 KB of Grisu/Dragon/bignum machinery that the default path
   pulls in. Output may use scientific notation (`1e20`, `1e-7`) for
   extreme magnitudes — matches Python's `repr(float)` semantics, which
   the rest of the VM display logic targets. NaN / ±inf / ±0.0 / whole
   numbers are still short-circuited above so we never hand `ryu` a value
   it would format differently from Python. */
fn format_general(f: f64) -> alloc::string::String {
    let mut buf = ryu::Buffer::new();
    alloc::string::String::from(buf.format(f))
}

#[macro_export]
macro_rules! push {
    ($s:ident, $v:literal) => { $s.push_str($v); };
    ($s:ident, str $v:expr) => { $s.push_str($v); };
    ($s:ident, int $v:expr) => {{ let mut b = itoa::Buffer::new(); $s.push_str(b.format($v)); }};
    ($s:ident, float $v:expr) => { $s.push_str(&$crate::modules::fstr::format_f64($v)); };
    ($s:ident, char $v:expr) => { $s.push($v); };
    ($s:ident, bool $v:expr) => { $s.push_str(if $v { "true" } else { "false" }); };
}

#[macro_export]
macro_rules! s {
    (@b $s:ident;) => {};
    (@b $s:ident; $l:literal $(, $($r:tt)*)?) => { $s.push_str($l); $($crate::s!(@b $s; $($r)*);)? };
    (@b $s:ident; str $v:expr $(, $($r:tt)*)?) => { $s.push_str($v); $($crate::s!(@b $s; $($r)*);)? };
    (@b $s:ident; int $v:expr $(, $($r:tt)*)?) => {{ let mut _b = itoa::Buffer::new(); $s.push_str(_b.format($v)); $($crate::s!(@b $s; $($r)*);)? }};
    (@b $s:ident; float $v:expr $(, $($r:tt)*)?) => { $s.push_str(&$crate::modules::fstr::format_f64($v)); $($crate::s!(@b $s; $($r)*);)? };
    (@b $s:ident; char $v:expr $(, $($r:tt)*)?) => { $s.push($v); $($crate::s!(@b $s; $($r)*);)? };
    (@b $s:ident; bool $v:expr $(, $($r:tt)*)?) => { $s.push_str(if $v { "true" } else { "false" }); $($crate::s!(@b $s; $($r)*);)? };
    (cap: $c:expr; $($t:tt)*) => {{ let mut _s = alloc::string::String::with_capacity($c); $crate::s!(@b _s; $($t)*); _s }};
    ($($t:tt)*) => {{ let mut _s = alloc::string::String::new(); $crate::s!(@b _s; $($t)*); _s }};
}

/* Format little-endian base-10⁹ digit groups as a decimal string.
   Highest group is unpadded; the rest are zero-padded to width 9. */
pub fn format_dec_groups(groups: &[u32]) -> alloc::string::String {
    let mut out = alloc::string::String::new();
    for (i, &g) in groups.iter().rev().enumerate() {
        let mut b = itoa::Buffer::new();
        let s = b.format(g);
        if i == 0 { out.push_str(s); continue; }
        for _ in 0..9usize.saturating_sub(s.len()) { out.push('0'); }
        out.push_str(s);
    }
    out
}

pub enum E {
    Parse  { ctx: &'static str },
    Custom { msg: alloc::string::String },
}

impl E {
    pub fn message(&self) -> alloc::string::String {
        match self {
            Self::Parse { ctx } => s!("parse error: ", str ctx),
            Self::Custom { msg } => msg.clone(),
        }
    }
    #[inline] pub fn parse(ctx: &'static str) -> Self { Self::Parse { ctx } }
}

impl From<E> for alloc::string::String { fn from(e: E) -> Self { e.message() } }

#[macro_export]
macro_rules! err {
    ($($t:tt)*) => { $crate::modules::fstr::E::Custom { msg: $crate::s!($($t)*) } };
}
