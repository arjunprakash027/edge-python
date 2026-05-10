//! `#[plugin_fn]` proc macro: turns a typed Rust function into a
//! wire-shape export consumable by the Edge Python wasm-abi v1.
//!
//! Generated wrapper signature (extern "C", no_mangle):
//!   fn <name>(argv: *const u32, argc: u32, out: *mut u32) -> i32
//!
//! The user's original function is renamed to `__edge_impl_<name>` so it
//! stays callable from inside the crate (tests, helpers) without name
//! collision with the wrapper.
//!
//! Internally the wrapper:
//!   1. Decodes argv[i] into the parameter types via FromValue.
//!   2. Calls the renamed user fn.
//!   3. If the user returned Result<T, Error>, unwraps Err -> stash + return 1.
//!   4. Encodes the success value via IntoValue, writes the handle to *out.
//!   5. Returns 0 on Ok, 1 on Err.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, FnArg, ItemFn, Pat, ReturnType};

#[proc_macro_attribute]
pub fn plugin_fn(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);
    let user_vis = &input.vis;
    let user_name = input.sig.ident.clone();
    let user_inputs = input.sig.inputs.clone();
    let user_output = input.sig.output.clone();
    let user_block = input.block.clone();

    // Rename the impl so we can take the original name for the wrapper.
    let impl_name = syn::Ident::new(
        &format!("__edge_impl_{}", user_name),
        proc_macro2::Span::call_site(),
    );

    // Collect (binding-name, type) pairs.
    let mut bindings: Vec<(syn::Ident, syn::Type)> = Vec::new();
    for (i, arg) in user_inputs.iter().enumerate() {
        match arg {
            FnArg::Typed(pat) => {
                let name = match &*pat.pat {
                    Pat::Ident(id) => id.ident.clone(),
                    _ => syn::Ident::new(
                        &format!("__arg{}", i),
                        proc_macro2::Span::call_site(),
                    ),
                };
                bindings.push((name, (*pat.ty).clone()));
            }
            FnArg::Receiver(_) => {
                return TokenStream::from(quote! {
                    compile_error!("#[plugin_fn] does not support methods (`self` parameter)");
                });
            }
        }
    }

    // Detect Result<T, _>. If present, the wrapper unwraps it; otherwise
    // it treats the value as direct.
    let return_ty: syn::Type = match &user_output {
        ReturnType::Default => syn::parse_quote!(()),
        ReturnType::Type(_, t) => (**t).clone(),
    };
    let is_result = matches!(&return_ty,
        syn::Type::Path(p) if p.path.segments.last()
            .map(|s| s.ident == "Result").unwrap_or(false));

    let argc_expected = bindings.len();
    let decodes: Vec<TokenStream2> = bindings.iter().enumerate().map(|(i, (name, ty))| {
        quote! {
            let h = unsafe { *argv.add(#i) };
            let #name: #ty = match <#ty as ::edge_pdk::FromValue>::from_handle(h) {
                Ok(v) => v,
                Err(e) => { ::edge_pdk::__internals::stash_error(e); return 1; }
            };
        }
    }).collect();
    let arg_names: Vec<&syn::Ident> = bindings.iter().map(|(n, _)| n).collect();

    let invoke = if is_result {
        quote! {
            match #impl_name(#(#arg_names),*) {
                Ok(v) => v,
                Err(e) => { ::edge_pdk::__internals::stash_error(e); return 1; }
            }
        }
    } else {
        quote! { #impl_name(#(#arg_names),*) }
    };

    let expanded = quote! {
        // The user's original function under a hidden name.
        #[doc(hidden)]
        #user_vis fn #impl_name(#user_inputs) #user_output #user_block

        // The wire-shape export takes the user's original name.
        #[doc(hidden)]
        #[unsafe(no_mangle)]
        #[allow(clippy::not_unsafe_ptr_arg_deref)]
        pub extern "C" fn #user_name(
            argv: *const u32,
            argc: u32,
            out: *mut u32,
        ) -> i32 {
            if (argc as usize) != #argc_expected {
                ::edge_pdk::__internals::stash_error(::edge_pdk::Error::Type(
                    ::alloc::format!(
                        "{} expects {} positional args, got {}",
                        stringify!(#user_name), #argc_expected, argc)));
                return 1;
            }
            #(#decodes)*
            let __value = { #invoke };
            // Encode the success value. We call IntoValue through a fully
            // qualified path so the user doesn't need to bring the trait
            // into scope.
            match ::edge_pdk::IntoValue::into_handle(__value) {
                Ok(h) => {
                    unsafe { *out = h.into_raw(); }
                    0
                }
                Err(e) => { ::edge_pdk::__internals::stash_error(e); 1 }
            }
        }
    };

    expanded.into()
}
