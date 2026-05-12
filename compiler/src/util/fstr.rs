/* f64 -> string: nan/±inf/±0.0/whole->".0" suffix/else default. */
pub fn format_f64(f: f64) -> alloc::string::String {
    // Lowercase tokens.
    if f.is_nan() { return alloc::string::String::from("nan"); }
    if f == f64::INFINITY { return alloc::string::String::from("inf"); }
    if f == f64::NEG_INFINITY { return alloc::string::String::from("-inf"); }
    if f == 0.0 {
        return if f.is_sign_negative() { alloc::string::String::from("-0.0") } else { alloc::string::String::from("0.0") };
    }

    // Whole float: itoa + ".0" avoids Rust's bare "1" for 1.0.
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

/* Default f64 format; 32-byte preallocation fits the common case without regrowth. */
fn format_general(f: f64) -> alloc::string::String {
    use core::fmt::Write;
    let mut out = alloc::string::String::with_capacity(32);
    /* core::fmt::Write::write_fmt is infallible for a String. */
    let _ = write!(&mut out, "{}", f);
    out
}

#[macro_export]
macro_rules! s {
    (@b $s:ident;) => {};
    (@b $s:ident; $l:literal $(, $($r:tt)*)?) => { $s.push_str($l); $($crate::s!(@b $s; $($r)*);)? };
    (@b $s:ident; str $v:expr $(, $($r:tt)*)?) => { $s.push_str($v); $($crate::s!(@b $s; $($r)*);)? };
    (@b $s:ident; int $v:expr $(, $($r:tt)*)?) => {{ let mut _b = itoa::Buffer::new(); $s.push_str(_b.format($v)); $($crate::s!(@b $s; $($r)*);)? }};
    (@b $s:ident; float $v:expr $(, $($r:tt)*)?) => { $s.push_str(&$crate::util::fstr::format_f64($v)); $($crate::s!(@b $s; $($r)*);)? };
    (@b $s:ident; char $v:expr $(, $($r:tt)*)?) => { $s.push($v); $($crate::s!(@b $s; $($r)*);)? };
    (@b $s:ident; bool $v:expr $(, $($r:tt)*)?) => { $s.push_str(if $v { "true" } else { "false" }); $($crate::s!(@b $s; $($r)*);)? };
    (cap: $c:expr; $($t:tt)*) => {{ let mut _s = alloc::string::String::with_capacity($c); $crate::s!(@b _s; $($t)*); _s }};
    ($($t:tt)*) => {{ let mut _s = alloc::string::String::new(); $crate::s!(@b _s; $($t)*); _s }};
}

/* Formats little-endian base-10^9 groups as decimal; highest unpadded, rest zero-padded to 9. */
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
    Parse { ctx: &'static str },
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
    ($($t:tt)*) => { $crate::util::fstr::E::Custom { msg: $crate::s!($($t)*) } };
}
