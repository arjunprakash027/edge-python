/*
Fetches vanilla `compiler_lib.wasm` from the upstream Edge Python release
into `runtime/`. Skipped if the file is already there (delete it to force
re-download).
*/

use std::{path::PathBuf, process::Command};

const UPSTREAM_REPO: &str = "https://github.com/dylan-sutton-chavez/edge-python";
const UPSTREAM_VERSION: &str = "v0.1.0";

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let dst = PathBuf::from(&manifest).join("runtime").join("compiler_lib.wasm");
    if dst.exists() { return; }

    let url = format!("{UPSTREAM_REPO}/releases/download/{UPSTREAM_VERSION}/compiler_lib.wasm");
    let status = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&dst)
        .arg(&url)
        .status()
        .expect("`curl` required on PATH to fetch compiler_lib.wasm");

    if !status.success() {
        let _ = std::fs::remove_file(&dst);
        panic!(
            "failed to download compiler_lib.wasm from {url}\n\
             hint: ensure release {UPSTREAM_VERSION} exists with `compiler_lib.wasm` as an asset, \
             or pre-place it at {}",
            dst.display()
        );
    }
}
