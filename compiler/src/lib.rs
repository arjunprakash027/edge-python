#![cfg_attr(target_arch = "wasm32", no_std)]

extern crate alloc;

#[cfg(target_arch = "wasm32")]
pub mod bridge;

pub mod modules {
    pub mod fx;
    pub mod lexer;
    pub mod vm;
    pub mod parser;
    pub mod packages;
    pub mod fstr;
    pub mod sha256;
}