#![cfg_attr(target_arch = "wasm32", no_std)]

extern crate alloc;

#[cfg(any(target_arch = "wasm32", test))]
pub mod wasm;

pub mod modules {
    pub mod fx;
    pub mod lexer;
    pub mod vm;
    pub mod parser;
    pub mod packages;
    pub mod fstr;
}