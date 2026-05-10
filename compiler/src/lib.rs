#![cfg_attr(target_arch = "wasm32", no_std)]
#![allow(special_module_name)]

extern crate alloc;

pub mod abi;

#[cfg(target_arch = "wasm32")]
pub mod main;

/* Internal helpers shared across the compiler — not Edge Python language
   modules. Kept separate from `modules/` (which contains lexer/parser/vm/
   packages — runtime components) so contributors don't mistake utility
   code for built-in stdlib. */
pub mod util {
    pub mod fx;
    pub mod fstr;
    pub mod sha256;
}

pub mod modules {
    pub mod lexer;
    pub mod vm;
    pub mod parser;
    pub mod packages;
}