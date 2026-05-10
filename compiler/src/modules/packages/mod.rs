/*
Parse-time import: resolver returns Code (spliced .py) or Native (extern_table); VM sees Call/CallExtern.
*/

use alloc::{boxed::Box, string::{String, ToString}, sync::Arc, vec::Vec};

use crate::s;
use crate::modules::vm::types::{HeapPool, Val, VmErr};

pub mod manifest;
pub use manifest::{Manifest, parse_manifest, walk_up_dirs, dir_of, join_relative};

/* Arc-wrapped callable for EdgePython natives; supports stateful loaders like wasmtime stores. */
pub type ExternFnPtr = Arc<dyn Fn(&mut HeapPool, &[Val]) -> Result<Val, VmErr> + Send + Sync>;

/* Named native binding with purity flag; pure=true enables VM memoization, false for I/O or side-effects. */
#[derive(Clone)]
pub struct NativeBinding {
    pub name: String,
    pub func: ExternFnPtr,
    pub pure: bool,
}

impl NativeBinding {
    /* Convenience constructor wrapping a plain fn pointer into an Arc for hand-written Rust natives. */
    pub fn from_fn(name: impl Into<String>, func: fn(&mut HeapPool, &[Val]) -> Result<Val, VmErr>, pure: bool) -> Self {
        Self { name: name.into(), func: Arc::new(func), pure }
    }
}

/* Resolver result: Code splices .py defs; Native registers extern bindings. canonical dedupes diamond imports by resolved path. */
#[derive(Clone)]
pub enum Resolved {
    Code { src: alloc::string::String, canonical: alloc::string::String },
    Native { bindings: Vec<NativeBinding>, canonical: alloc::string::String },
}

/* Host-injected trait; resolve called once per import statement. &mut self allows internal caching. */
pub trait Resolver {
    fn resolve(&mut self, spec: &str) -> Result<Resolved, String>;

    /* Returns a child resolver rescoped to the imported file's directory for transitive imports; default is NoopResolver. */
    fn child(&self, _spec: &str) -> Box<dyn Resolver> {
        Box::new(NoopResolver)
    }

    /* Returns raw bytes for integrity verification and manifest walk-up; Err signals absent file. expected_hash must match. */
    fn fetch_bytes(&mut self, _spec: &str, _expected_hash: Option<[u8; 32]>) -> Result<alloc::vec::Vec<u8>, String> {
        Err(s!("module '", str _spec, "' integrity verification not supported by this resolver"))
    }
}

/* Splits spec into (url, Option<[u8;32]>); errors on malformed #sha256- fragment; decodes hex once. */
pub fn parse_integrity(spec: &str) -> Result<(&str, Option<[u8; 32]>), String> {
    let Some((url, frag)) = spec.split_once('#') else {
        return Ok((spec, None));
    };
    let Some(hex) = frag.strip_prefix("sha256-") else {
        return Err(s!(
            "unrecognized integrity fragment in '", str spec,
            "'; expected '#sha256-<64 hex chars>'"));
    };
    if hex.len() != 64 {
        return Err(s!(
            "sha256 fragment must be 64 hex chars in '", str spec,
            "'; got ", int hex.len() as i64));
    }
    let hash = crate::util::sha256::hex_decode_32(hex).ok_or_else(|| s!(
        "invalid hex in sha256 fragment of '", str spec, "'"))?;
    Ok((url, Some(hash)))
}

/* Default resolver rejecting all specs with a diagnostic; used when no resolver is configured. */
pub struct NoopResolver;

impl Resolver for NoopResolver {
    fn resolve(&mut self, spec: &str) -> Result<Resolved, String> {
        Err(s!("module '", str spec, "' not found (no resolver configured)"))
    }
}

/* Boxes a concrete Resolver into Box<dyn Resolver>, removing boilerplate casts at call sites. */
pub fn boxed<R: Resolver + 'static>(r: R) -> Box<dyn Resolver> {
    Box::new(r)
}

impl Default for Box<dyn Resolver> {
    fn default() -> Self { Box::new(NoopResolver) }
}

/* Re-exports core types; hosts get trait, enums, binding, and default resolver via glob import. */
pub use NativeBinding as Binding;
pub use Resolved as ResolvedModule;
pub use NoopResolver as Default_;

/* Converts public NativeBinding into internal ExternFn; two structs separate host API from VM storage. */
pub(crate) fn binding_to_extern(b: &NativeBinding) -> crate::modules::vm::types::ExternFn {
    crate::modules::vm::types::ExternFn {
        name: b.name.clone(),
        func: b.func.clone(),
        pure: b.pure,
    }
}

/* Scans source for quoted from-import specs; WASM host uses results to pre-fetch URLs before compile. */
pub fn scan_string_imports(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        if !t.starts_with("from ") { continue; }
        let rest = &t[5..].trim_start();
        let bytes = rest.as_bytes();
        if bytes.is_empty() || bytes[0] != b'"' { continue; }
        let mut end = 1;
        while end < bytes.len() && bytes[end] != b'"' { end += 1; }
        if end < bytes.len() {
            out.push(rest[1..end].to_string());
        }
    }
    out
}
