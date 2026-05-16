/* Copies upstream's prefetched `compiler_lib.wasm` into `runtime/` for the JS host. */

fn main() {
    let src = std::env::var("DEP_COMPILER_LIB_WASM").expect("upstream `edge-python` must set `links`");
    std::fs::copy(src, "runtime/compiler_lib.wasm").unwrap();
    println!("cargo::rerun-if-changed=build.rs");
}
