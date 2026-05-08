#![cfg_attr(target_arch = "wasm32", no_std)]
#![allow(special_module_name)]

extern crate alloc;

pub mod abi;

#[cfg(target_arch = "wasm32")]
pub mod main;

pub mod modules {
    pub mod fx;
    pub mod lexer;
    pub mod vm;
    pub mod parser;
    pub mod packages;
    pub mod fstr;
    pub mod sha256;
}