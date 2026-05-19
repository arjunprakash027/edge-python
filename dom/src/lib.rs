#![no_std]
#![no_main]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

// Concatenated at build time from `src/bridge/*.js` — see `build.rs`.
const BRIDGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bridge.js"));

#[unsafe(no_mangle)]
pub extern "C" fn edge_capability_bridge_ptr() -> *const u8 { BRIDGE.as_ptr() }

#[unsafe(no_mangle)]
pub extern "C" fn edge_capability_bridge_len() -> u32 { BRIDGE.len() as u32 }
