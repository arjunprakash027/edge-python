// Pure-Rust f64 math (no libm, works under no_std / WASM).

#[inline]
pub fn fpowi(mut base: f64, exp: i32) -> f64 {
    if exp == 0 { return 1.0; }
    let neg = exp < 0;
    let mut e = (exp as i64).unsigned_abs() as u32;
    let mut r = 1.0;
    while e > 0 { if e & 1 != 0 { r *= base; } base *= base; e >>= 1; }
    if neg { 1.0 / r } else { r }
}

#[inline]
pub fn fround(x: f64) -> f64 {
    let i = x as i64;
    let t = i as f64;
    let d = x - t;
    if d > 0.5 { t + 1.0 }
    else if d < -0.5 { t - 1.0 }
    else if d == 0.5 { if i % 2 == 0 { t } else { t + 1.0 } }
    else if d == -0.5 { if i % 2 == 0 { t } else { t - 1.0 } }
    else { t }
}

pub fn fln(x: f64) -> f64 {
    let bits = f64::to_bits(x);
    let exp = ((bits >> 52) & 0x7FF) as i64 - 1023;
    let m = f64::from_bits((bits & 0x000F_FFFF_FFFF_FFFF) | 0x3FF0_0000_0000_0000);
    let t = (m - 1.0) / (m + 1.0); let t2 = t * t;
    2.0 * t * (1.0 + t2 * (1.0/3.0 + t2 * (1.0/5.0 + t2 * (1.0/7.0 + t2 / 9.0)))) + exp as f64 * core::f64::consts::LN_2
}

pub fn fexp(x: f64) -> f64 {
    if x > 709.0 { return f64::INFINITY; }
    if x < -709.0 { return 0.0; }
    let k = (x * core::f64::consts::LOG2_E) as i64;
    let r = x - k as f64 * core::f64::consts::LN_2;
    let e = 1.0 + r * (1.0 + r * (0.5 + r * (1.0/6.0 + r * (1.0/24.0 + r * (1.0/120.0 + r / 720.0)))));
    f64::from_bits(((k + 1023) as u64) << 52) * e
}

#[inline]
pub fn fpowf(base: f64, exp: f64) -> f64 {
    let ei = exp as i32;
    if (ei as f64) == exp { return fpowi(base, ei); }
    if base <= 0.0 {
        if base == 0.0 { return if exp > 0.0 { 0.0 } else { f64::INFINITY }; }
        return f64::NAN;
    }
    fexp(exp * fln(base))
}

#[inline]
pub fn ffloor(x: f64) -> f64 {
    let i = x as i64 as f64;
    if x < i { i - 1.0 } else { i }
}

#[inline]
pub fn fabs(x: f64) -> f64 {
    f64::from_bits(f64::to_bits(x) & 0x7FFF_FFFF_FFFF_FFFF)
}

#[inline]
pub fn ftrunc(x: f64) -> f64 {
    if x >= 0.0 { ffloor(x) } else { -ffloor(-x) }
}

#[inline]
pub fn fsignum(x: f64) -> f64 {
    if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 }
}

#[inline]
pub fn flog10(x: f64) -> f64 { fln(x) / core::f64::consts::LN_10 }
