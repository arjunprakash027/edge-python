// Concatenates `src/bridge/*.js` fragments into OUT_DIR/bridge.js in `PARTS` order.

use std::env;
use std::fs;
use std::path::PathBuf;

// state.js must come first (defines `makeState`); bridge.js must come last (the composer).
const PARTS: &[&str] = &[
    "state.js",
    "tree.js",
    "style.js",
    "events.js",
    "forms.js",
    "observers.js",
    "animations.js",
    "media.js",
    "platform.js",
    "bridge.js",
];

fn main() {
    let bridge_dir = PathBuf::from("src/bridge");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));

    let mut combined = String::new();
    for part in PARTS {
        let path = bridge_dir.join(part);
        let content = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
        combined.push_str(&content);
        combined.push('\n');
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-changed=build.rs");

    let dest = out_dir.join("bridge.js");
    fs::write(&dest, combined).expect("failed to write concatenated bridge.js");
}
