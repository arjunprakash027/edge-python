/*
DOM capability module for Edge Python (Path D: self-contained `.wasm` carrying
its own JS bridge).

This crate produces `edge_python_dom.wasm`, a minimal artifact that embeds
`bridge.js` as static data and exposes two exports the host loader uses to
extract it. The loader (vendored in `runtime/src/edge.js`) detects this module
shape, instantiates it, reads the bridge source, evaluates it to obtain DOM
handlers, and registers them as a synthetic native module against vanilla
`compiler_lib.wasm` — no custom embedder, no client-side DOM code.

No allocator, no panics, no std. Everything beyond `core` lives in `bridge.js`.
*/

#![no_std]
#![no_main]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

const BRIDGE: &[u8] = include_bytes!("bridge.js");

#[unsafe(no_mangle)]
pub extern "C" fn edge_capability_bridge_ptr() -> *const u8 { BRIDGE.as_ptr() }

#[unsafe(no_mangle)]
pub extern "C" fn edge_capability_bridge_len() -> u32 { BRIDGE.len() as u32 }
