//! Canonical reference module for the Edge Python SDK. The integration test
//! in `compiler/tests/packages.rs` builds this exact file to wasm32 and loads
//! it via the test loader — single source of truth between the docs and the
//! code that actually runs.
//!
//! Build:
//!     cargo build --release --target wasm32-unknown-unknown --example reference
//!
//! Use from a script:
//!     from "./reference.wasm" import add, square
//!     print(add(2, square(3)))   # 11
//!
//! `crate-type = ["cdylib"]`, so on the host triple this builds an empty
//! cdylib (no host entry point). Edge Python's loaders only consume the
//! wasm32 build.

#![cfg_attr(target_arch = "wasm32", no_std, no_main)]

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

#[cfg(target_arch = "wasm32")]
mod m {
    use edge_sdk::edge_export;

    edge_export! {
        pub fn add(a: i64, b: i64) -> i64 {
            a + b
        }
    }

    edge_export! {
        pub fn square(x: i64) -> i64 {
            x * x
        }
    }

    edge_export! {
        pub fn area(r: f64) -> f64 {
            core::f64::consts::PI * r * r
        }
    }

    edge_export! {
        pub fn even(n: i64) -> bool {
            n % 2 == 0
        }
    }

    edge_export! {
        pub fn pick(flag: bool, lo: i64, hi: i64) -> i64 {
            if flag { hi } else { lo }
        }
    }
}
