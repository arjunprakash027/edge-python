//! Reference Edge Python native module: a few text utilities.
//!
//! Build:
//!   cargo build --release --target wasm32-unknown-unknown -p slugify-mod
//!
//! The artifact is at:
//!   target/wasm32-unknown-unknown/release/slugify_mod.wasm
//!
//! Use it from a script:
//!   from "./slugify_mod.wasm" import slugify, shout, repeat_n, sum_ints
//!   print(slugify("Hello World"))    # -> hello-world
//!   print(shout("ok"))               # -> OK!
//!   print(repeat_n("ha", 3))         # -> hahaha
//!   print(sum_ints([1, 2, 3, 4]))    # -> 10

#![no_std]
#![no_main]

extern crate alloc;

use alloc::{string::{String, ToString}, vec::Vec};
use wasm_pdk::*;

#[global_allocator]
static A: lol_alloc::LeakingPageAllocator = lol_alloc::LeakingPageAllocator;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

#[plugin_fn]
fn slugify(s: String) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[plugin_fn]
fn shout(s: String) -> String {
    let mut out = s.to_uppercase();
    out.push('!');
    out
}

#[plugin_fn]
fn repeat_n(s: String, n: i64) -> Result<String> {
    if n < 0 {
        return Err(Error::Value("repeat count must be non-negative".to_string()));
    }
    Ok(s.repeat(n as usize))
}

/// Universal-dispatch demo: takes a list handle, sums its int items.
#[plugin_fn]
fn sum_ints(items: Handle) -> Result<i64> {
    let n = items.len()? as u32;
    let mut total: i64 = 0;
    for i in 0..n {
        let item = items.get_item(i)?;
        let v = i64::from_handle(item.raw())?;
        total += v;
    }
    Ok(total)
}

// FromValue for i64 / Handle helpers come in via `use wasm_pdk::*;`.
