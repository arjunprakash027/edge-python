/*
Reference `wasm-pdk` module showcasing #[plugin_class]. Build with `cargo build --release --target wasm32-unknown-unknown -p slugify-mod`.
*/

#![no_std]
#![no_main]

extern crate alloc;

use alloc::{string::{String, ToString}, vec::Vec};
use wasm_pdk::*;

#[global_allocator]
static A: lol_alloc::LeakingPageAllocator = lol_alloc::LeakingPageAllocator;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

/// Accumulates slug parts across calls; exercises mutable state and Option/Result returns.
#[plugin_class]
pub struct Slugger {
    parts: Vec<String>,
}

#[plugin_methods]
impl Slugger {
    #[plugin_ctor]
    pub fn new() -> Self {
        Self { parts: Vec::new() }
    }

    /// Lowercase, replace non-alphanumeric with '-', drop empty splits, append to internal buffer.
    pub fn add(&mut self, s: String) {
        let normalized: String = s.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        for part in normalized.split('-').filter(|p| !p.is_empty()) {
            self.parts.push(part.to_string());
        }
    }

    /// Joins accumulated parts with '-' into the final slug.
    pub fn build(&self) -> String {
        self.parts.join("-")
    }

    /// Returns the joined slug uppercased with an exclamation suffix.
    pub fn shout(&self) -> String {
        let mut out = self.parts.join("-").to_uppercase();
        out.push('!');
        out
    }

    /// Returns the joined slug repeated n times; rejects negative counts with ValueError.
    pub fn repeat(&self, n: i64) -> Result<String> {
        if n < 0 {
            return Err(Error::Value("repeat count must be non-negative".to_string()));
        }
        Ok(self.parts.join("-").repeat(n as usize))
    }

    /// Sums the byte lengths of accumulated parts; demonstrates &self read-only reduction.
    pub fn total_len(&self) -> i64 {
        self.parts.iter().map(|p| p.len() as i64).sum()
    }

    /// Pops and returns the last part, or None when the buffer is empty.
    pub fn pop(&mut self) -> Option<String> {
        self.parts.pop()
    }
}
