/* Module / package resolution layer.

   `from <spec> import <names>` is a parse-time concept in EdgePython: the parser
   asks a host-injected `Resolver` to materialise the module, then either inlines
   .py source as functions in the parent chunk (Code) or registers native function
   pointers in the chunk's extern_table (Native). The VM never learns what a module
   is — it only sees the existing `Call` opcode (for inlined code) or the new
   `CallExtern` opcode (for natives).

   Hosts implement `Resolver` to plug their own resolution strategy:
     * Browser / WASM: pre-fetch URLs in JS, resolve from an in-memory map.
     * CLI: read from local FS or fetch + cache.
     * Tests: a small struct with a HashMap of fixture modules.

   The default `NoopResolver` rejects all imports — the parser stays usable
   without any host wiring (existing tests that don't touch imports keep working). */

use alloc::{boxed::Box, string::{String, ToString}, sync::Arc, vec::Vec};

use crate::s;
use crate::modules::vm::types::{HeapPool, Val, VmErr};

pub mod manifest;
pub use manifest::{Manifest, parse_manifest, walk_up_dirs, dir_of, join_relative};

/* Callable signature for a native exposed to EdgePython. Receives the heap
   (so the function can allocate strings, lists, etc.) and a slice of its
   positional arguments; returns a `Val` or a `VmErr`. No kwargs (kwargs are a
   parse-time concept tied to user-defined functions).

   `Arc<dyn Fn>` rather than a plain `fn` pointer so loaders can wrap stateful
   instances (a wasmtime `Store + Instance`, a libloading-loaded dyn-lib
   handle, etc.) in a closure. Plain `fn` pointers wrap into Arc cheaply via
   the `NativeBinding::from_fn` constructor. */
pub type ExternFnPtr = Arc<dyn Fn(&mut HeapPool, &[Val]) -> Result<Val, VmErr> + Send + Sync>;

/* One importable name from a native module: the bare identifier, the
   callable, and a purity flag. `pure = true` allows the VM to memoize the
   result (template cache) and avoids tainting enclosing user functions with
   impurity. Set `pure = false` for anything that performs I/O, mutates
   external state, or reads non-deterministic input. */
#[derive(Clone)]
pub struct NativeBinding {
    pub name: String,
    pub func: ExternFnPtr,
    pub pure: bool,
}

impl NativeBinding {
    /* Convenience for hosts that have a plain `fn` pointer in hand (the
       common case for hand-written Rust natives compiled into the host). */
    pub fn from_fn(
        name: impl Into<String>,
        func: fn(&mut HeapPool, &[Val]) -> Result<Val, VmErr>,
        pure: bool,
    ) -> Self {
        Self { name: name.into(), func: Arc::new(func), pure }
    }
}

/* What the resolver returned for a given module spec.
   * Code: a `.py` source string — the parser will lex/parse it and splice its
     `def` definitions into the importing chunk.
   * Native: a list of pre-built bindings — the parser will register them in
     the importing chunk's extern_table.

   Cloneable so resolvers can cache results across diamond imports without
   re-fetching. NativeBinding's `func` is an Arc, so the clone is shallow. */
#[derive(Clone)]
pub enum Resolved {
    Code(String),
    Native(Vec<NativeBinding>),
}

/* Host-injected lookup. Implementations should be cheap to call: the parser
   may invoke `resolve` once per `from <spec> ...` statement during compilation.
   The `&mut self` allows resolvers to maintain caches or counters; stateless
   implementations should ignore it. */
pub trait Resolver {
    fn resolve(&mut self, spec: &str) -> Result<Resolved, String>;

    /* Sub-resolver for the imported file's transitive imports. Called by
       the splicer after `resolve(spec)` returned `Resolved::Code` so the
       inner parser can resolve the imported file's own `from ...`
       statements.

       The returned resolver should share module-cache state with `self`
       (so diamond imports dedupe) and rescope its current directory to the
       imported file's location. The directory rescope serves two ends:

         1. Quoted relative paths (`./helpers.py`) inside the imported file
            resolve against the imported file's directory, not the entry
            script's.
         2. Bare-name imports inside the imported file walk up from that
            directory looking for `packages.json`. A sub-package can carry
            its own manifest and its bare names are decided locally
            (hermetic by default; `extends` opts into inheriting from a
            parent manifest's directory).

       The default impl returns `NoopResolver`, which preserves the
       original behavior of rejecting transitive imports. WASM and
       embedder hosts override this to rescope and thread the resolver
       through. */
    fn child(&self, _spec: &str) -> Box<dyn Resolver> {
        Box::new(NoopResolver)
    }

    /* Return the raw bytes that back `spec`. Called by the parser in two
       situations:

         1. Integrity verification: when a URL import carries a
            `#sha256-...` fragment, the parser hashes the bytes returned
            here against the declared digest before invoking `resolve`. A
            host that can't supply the bytes returns Err and the parser
            surfaces a clean "not supported" diagnostic rather than
            silently bypassing the check.

         2. Nested-manifest walk-up: when a bare name is encountered, the
            host's `resolve` impl typically asks `fetch_bytes` for
            `<dir>/packages.json` at each parent directory. Returning Err
            for an absent manifest is the documented signal to "keep
            walking up"; the parser only fails if the walk exhausts.

       `expected_hash` is **mandatory** integrity contract: when `Some`,
       the host MUST return bytes whose sha-256 digest equals the
       expected value, OR return Err. Returning bytes with a different
       digest is a contract violation — the parser trusts the host's
       answer here for cross-host reproducibility. When `None`, the host
       is free to fetch fresh content; the parser will hash it itself if
       a `#sha256-...` fragment is in the spec.

       The default impl returns Err. The same `spec` (already stripped of
       any `#sha256-...` fragment by the parser) will be passed to
       `resolve` next, so the host is free to cache the bytes between the
       two calls. */
    fn fetch_bytes(
        &mut self,
        _spec: &str,
        _expected_hash: Option<[u8; 32]>,
    ) -> Result<alloc::vec::Vec<u8>, String> {
        Err(s!("module '", str _spec, "' integrity verification not supported by this resolver"))
    }
}

/* Split a URL spec into `(canonical_url, optional_sha256_hash)`. Returns an
   `Err` if the fragment exists but is malformed (anything other than a
   well-formed `sha256-<64 hex chars>`). A spec with no fragment returns the
   spec unchanged and `None` for the hash.

   The hash is decoded once here; the parser only sees a raw `[u8; 32]` and
   compares bytes, so the rest of the codebase never touches hex parsing. */
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
    let hash = crate::modules::sha256::hex_decode_32(hex).ok_or_else(|| s!(
        "invalid hex in sha256 fragment of '", str spec, "'"))?;
    Ok((url, Some(hash)))
}

/* Default resolver: rejects every spec with a clear message. Used when the
   parser is constructed via `Parser::new` (no resolver explicitly provided),
   so existing call sites don't need to change. Any `from X import ...` against
   `NoopResolver` produces a parse-time diagnostic instead of silent acceptance. */
pub struct NoopResolver;

impl Resolver for NoopResolver {
    fn resolve(&mut self, spec: &str) -> Result<Resolved, String> {
        Err(s!("module '", str spec, "' not found (no resolver configured)"))
    }
}

/* Convenience for hosts that want to box up a concrete resolver to hand to
   `Parser::with_resolver`. Avoids forcing every call site to write the
   `Box::new(...) as Box<dyn Resolver>` cast themselves. */
pub fn boxed<R: Resolver + 'static>(r: R) -> Box<dyn Resolver> {
    Box::new(r)
}

impl Default for Box<dyn Resolver> {
    fn default() -> Self { Box::new(NoopResolver) }
}

/* Re-export the types most hosts will need. Test/CLI/WASM crates can write
   `use compiler_lib::modules::packages::*;` and get the trait, the enums, the
   binding struct, and the default resolver in one line. */
pub use NativeBinding as Binding;
pub use Resolved as ResolvedModule;
pub use NoopResolver as Default_;

/* Convert an external NativeBinding into the chunk-internal ExternFn shape.
   Two structs exist because NativeBinding is the public host-facing API
   (lives in `packages`) and ExternFn is the chunk-internal storage shape
   (lives in `vm::types`, where Val/HeapPool/VmErr are defined). The host can
   pass either a plain `fn` pointer (wrapped in an Arc here) or a closure
   (e.g. a wasmtime-bound dispatcher), and ExternFn carries it uniformly. */
pub(crate) fn binding_to_extern(b: &NativeBinding) -> crate::modules::vm::types::ExternFn {
    crate::modules::vm::types::ExternFn {
        name: b.name.clone(),
        func: b.func.clone(),
        pure: b.pure,
    }
}

/* Light pre-scan for the WASM/JS host: walks the source line-by-line and
   collects every quoted module spec (the `"..."` after a `from`). Returns one
   spec per line. The host uses this to pre-fetch all URL-form imports in
   parallel before invoking the synchronous compile.

   Bare-name imports (`from json import x`) aren't included — those resolve via
   the host's import map, which lives outside the parser. */
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
