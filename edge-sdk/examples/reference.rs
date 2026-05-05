//! Canonical reference module for the Edge Python SDK. The integration test
//! in `compiler/tests/packages.rs` builds this exact file to wasm32 and loads
//! it via the production loader — single source of truth between the docs and
//! the code that actually runs.
//!
//! Build:
//!     cargo build --release --target wasm32-unknown-unknown --example reference
//!
//! Use from a script:
//!     from "./reference.wasm" import add, square
//!     print(add(2, square(3)))   # 11
//!
//! Compiles to a `cdylib` only on wasm32. On the host architecture it becomes
//! an empty crate so `cargo check` passes without a `#[panic_handler]`.

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
            3.141592653589793 * r * r
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

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("Build for wasm32: cargo build --release --target wasm32-unknown-unknown --example reference");
}
