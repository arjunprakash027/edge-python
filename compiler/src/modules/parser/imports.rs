/* Compile-time import resolution.

   Both `import X` and `from X import names` enter here. For Native modules,
   bindings are appended to chunk.extern_table; the call site emits CallExtern.
   For Code modules, requested functions and their same-module dependencies
   are inlined as MakeFunction + StoreName pairs in the parent chunk. */

use crate::s;

use super::Parser;
use super::types::{Diagnostic, ImportEntry, ImportKind, OpCode, SSAChunk, parse_string, ssa_strip};
use crate::modules::lexer::{Token, TokenType, lex};
use crate::modules::packages::{Resolved, binding_to_extern, parse_integrity};
use crate::modules::sha256::{sha256, hex_encode};
use crate::modules::fx::FxHashSet;

use alloc::{string::{String, ToString}, vec::Vec};

impl<'src, I: Iterator<Item = Token>> Parser<'src, I> {

    /* `import name [as alias][, ...]`. Bare `import` only — no dotted module
       paths and no string-form spec (those belong on `from`). The module is
       materialised as a `HeapObj::Module` value bound under the alias, so
       `name.attr` and `name.attr(...)` resolve at runtime. */
    pub(super) fn do_import_stmt(&mut self) {
        self.advance(); // 'import'
        loop {
            let (spec, span) = self.read_module_spec();
            let alias = if self.eat_if(TokenType::As) {
                self.advance_text()
            } else {
                spec.split('.').next().unwrap_or(&spec).to_string()
            };
            self.resolve_and_bind_all(&spec, span, &alias);
            if !self.eat_if(TokenType::Comma) { break; }
        }
    }

    /* `from <spec> import <name>[ as <alias>][, ...]` or `from <spec>
       import *`. The spec is either a quoted string ("https://...",
       "./utils.py") or a bare identifier (resolver-looked-up). Star binds
       every export of the module under its bare name, mirroring CPython. */
    pub(super) fn do_from_stmt(&mut self) {
        self.advance(); // 'from'
        let (spec, spec_span) = self.read_module_spec();
        self.eat(TokenType::Import);

        if self.eat_if(TokenType::Star) {
            self.resolve_and_bind_star(&spec, spec_span);
            return;
        }

        let mut names: Vec<(String, String)> = Vec::new();
        loop {
            let name = self.advance_text();
            let alias = if self.eat_if(TokenType::As) { self.advance_text() } else { name.clone() };
            names.push((name, alias));
            if !self.eat_if(TokenType::Comma) { break; }
        }

        self.resolve_and_bind_named(&spec, spec_span, names);
    }

    /* Read a module spec: either a quoted string literal (URL / path) or a
       dotted bare name. Returns `(spec, span)` so callers can attach
       diagnostics to the spec's source position. */
    fn read_module_spec(&mut self) -> (String, (usize, usize)) {
        if matches!(self.peek(), Some(TokenType::String)) {
            let t = self.advance();
            let raw = self.lexeme(&t).to_string();
            let unquoted = parse_string(&raw);
            (unquoted, (t.start, t.end))
        } else {
            let first = self.advance();
            let mut name = self.lexeme(&first).to_string();
            let mut span = (first.start, first.end);
            while self.eat_if(TokenType::Dot) {
                let next = self.advance();
                name.push('.');
                name.push_str(self.lexeme(&next));
                span.1 = next.end;
            }
            (name, span)
        }
    }

    /* Verify the spec's `#sha256-...` fragment (if any) and resolve. The
       parser is the trust boundary for module bytes: every URL import that
       declares an integrity hash is checked HERE before the resolver hands
       back a `Resolved`, so a host that lies about hashes can't sneak past.
       Returns the URL stripped of the fragment so downstream uses
       (`child()`, error messages, module names) see clean URLs. */
    fn resolve_with_integrity(&mut self, spec: &str) -> Result<(String, Resolved), String> {
        let (url, expected) = parse_integrity(spec)?;
        if let Some(hash) = expected {
            // Pass the expected hash to the host so its lockfile / cache
            // can verify on its side too. The parser still re-hashes the
            // returned bytes as a defence-in-depth check — a misbehaving
            // host that returns wrong content gets caught here.
            let bytes = self.resolver.fetch_bytes(url, Some(hash))?;
            let computed = sha256(&bytes);
            if computed != hash {
                return Err(s!(
                    "integrity check failed for '", str url,
                    "'\n  expected sha256-", str &hex_encode(&hash),
                    "\n  got      sha256-", str &hex_encode(&computed)));
            }
        }
        let resolved = self.resolver.resolve(url)?;
        Ok((url.to_string(), resolved))
    }

    /* Parse a code module's source if not already cached, return the
       shared `Rc<SSAChunk>`. Only path-shaped specs (containing `/` or
       `://`) are cached: their canonical form equals the spec string,
       so the cache key is unambiguous. Bare names like `db` resolve
       differently depending on the importer's nested `packages.json`
       — caching by `db` would conflate two unrelated modules. The VM
       still treats the resulting chunks as singletons (same Rc on both
       cache hit and miss; module_table dedupes by canonical spec from
       `chunk.imports`). */
    fn parse_or_get_cached(
        &mut self, spec: &str, src: &str, span: (usize, usize),
    ) -> Option<alloc::rc::Rc<SSAChunk>> {
        let cache_safe = spec.contains('/') || spec.contains("://");
        if cache_safe
            && let Some(cached) = self.module_cache.borrow().get(spec).cloned()
        {
            return Some(cached);
        }
        let owned = src.to_string();
        let (tokens, lex_errs) = lex(&owned);
        let mut sub_parser = Parser::with_shared_cache(
            &owned, tokens.into_iter(),
            self.resolver.child(spec),
            self.module_cache.clone(),
        );
        for e in lex_errs {
            sub_parser.errors.push(Diagnostic {
                start: e.start, end: e.end, msg: e.msg.to_string(),
            });
        }
        let (sub, errs) = sub_parser.parse();
        if !errs.is_empty() {
            self.error_at(span.0, span.1,
                &s!("module '", str spec, "' parse error: ", str &errs[0].msg));
            return None;
        }
        let rc = alloc::rc::Rc::new(sub);
        if cache_safe {
            self.module_cache.borrow_mut().insert(spec.to_string(), rc.clone());
        }
        Some(rc)
    }

    /* Add an import to this chunk's import list, deduplicating by spec.
       Returns the index — the operand for `OpCode::LoadModule`. The VM
       walks the union of every chunk's imports at run() start and inits
       each unique spec exactly once, so a redundant entry is just a
       redundant index, not a redundant init. */
    fn register_import(&mut self, spec: &str, kind: ImportKind) -> u16 {
        if let Some(i) = self.chunk.imports.iter().position(|e| e.spec == spec) {
            return i as u16;
        }
        let i = self.chunk.imports.len() as u16;
        self.chunk.imports.push(ImportEntry {
            spec: spec.to_string(),
            kind,
        });
        i
    }

    /* Walk a parsed module's top-level instructions and collect every
       public bare name it stores (StoreName, MakeFunction's name slot).
       Used by `from M import *` to enumerate exports at compile time. */
    fn module_public_exports(sub: &SSAChunk) -> Vec<String> {
        let mut exports: Vec<String> = Vec::new();
        let mut seen: FxHashSet<String> = FxHashSet::default();
        for ins in &sub.instructions {
            let slot_idx = match ins.opcode {
                OpCode::StoreName => Some(ins.operand as usize),
                OpCode::MakeFunction | OpCode::MakeCoroutine => sub.functions
                    .get(ins.operand as usize)
                    .map(|f| f.3 as usize),
                _ => None,
            };
            let Some(s) = slot_idx else { continue };
            let Some(name) = sub.names.get(s) else { continue };
            let bare = ssa_strip(name).to_string();
            if bare.starts_with('_') { continue; }
            if seen.insert(bare.clone()) { exports.push(bare); }
        }
        exports
    }

    /* `from X import a, b, c` — register X in this chunk's imports and
       emit `LoadModule + LoadAttr + StoreName` per requested name. For
       Native modules, the bindings are also pushed into `extern_table`
       so call sites can keep using the fast `CallExtern` path. */
    fn resolve_and_bind_named(&mut self, spec: &str, span: (usize, usize), names: Vec<(String, String)>) {
        let (_url, resolved) = match self.resolve_with_integrity(spec) {
            Ok(r) => r,
            Err(msg) => { self.error_at(span.0, span.1, &msg); return; }
        };
        let url = match &resolved {
            Resolved::Code { canonical, .. } => canonical.clone(),
            Resolved::Native { canonical, .. } => canonical.clone(),
        };
        match resolved {
            Resolved::Native { bindings, .. } => {
                // Validate every requested name exists; populate fast-path
                // extern table for direct CallExtern dispatch.
                for (name, alias) in &names {
                    let Some(b) = bindings.iter().find(|b| b.name == *name) else {
                        self.error_at(span.0, span.1,
                            &s!("module '", str &url, "' has no export '", str name, "'"));
                        continue;
                    };
                    let idx = self.chunk.extern_table.len() as u16;
                    self.chunk.extern_table.push(binding_to_extern(b));
                    self.chunk.extern_index.insert(alias.clone(), idx);
                }
                // Also register the module so `import_module()` and any
                // first-class reference to the module value can resolve.
                let externs: Vec<crate::modules::vm::types::ExternFn> =
                    bindings.iter().map(binding_to_extern).collect();
                let _ = self.register_import(&url, ImportKind::Native(externs));
            }
            Resolved::Code { src, canonical } => {
                let Some(sub) = self.parse_or_get_cached(&canonical, &src, span) else { return; };
                let exports = Self::module_public_exports(&sub);
                let import_idx = self.register_import(&canonical, ImportKind::Code(sub));
                for (name, alias) in &names {
                    if !exports.iter().any(|e| e == name) {
                        let _ = url;
                        self.error_at(span.0, span.1,
                            &s!("module '", str &canonical, "' has no export '", str name, "'"));
                        continue;
                    }
                    self.chunk.emit(OpCode::LoadModule, import_idx);
                    let attr_idx = self.chunk.push_name(name);
                    self.chunk.emit(OpCode::LoadAttr, attr_idx);
                    let alias_ver = self.increment_version(alias);
                    let alias_slot = self.push_ssa_name(alias, alias_ver);
                    self.chunk.emit(OpCode::StoreName, alias_slot);
                }
            }
        }
    }

    /* `import X` — register X in this chunk's imports and emit
       `LoadModule + StoreName alias`. The Module Val itself is built
       once at VM init from the registered chunk; every importer that
       does `import X` ends up with the SAME Val (true singleton). */
    fn resolve_and_bind_all(&mut self, spec: &str, span: (usize, usize), alias: &str) {
        let (_url, resolved) = match self.resolve_with_integrity(spec) {
            Ok(r) => r,
            Err(msg) => { self.error_at(span.0, span.1, &msg); return; }
        };
        let import_idx = match resolved {
            Resolved::Native { bindings, canonical } => {
                let externs: Vec<crate::modules::vm::types::ExternFn> =
                    bindings.iter().map(binding_to_extern).collect();
                self.register_import(&canonical, ImportKind::Native(externs))
            }
            Resolved::Code { src, canonical } => {
                let Some(sub) = self.parse_or_get_cached(&canonical, &src, span) else { return; };
                self.register_import(&canonical, ImportKind::Code(sub))
            }
        };
        self.chunk.emit(OpCode::LoadModule, import_idx);
        let alias_ver = self.increment_version(alias);
        let alias_slot = self.push_ssa_name(alias, alias_ver);
        self.chunk.emit(OpCode::StoreName, alias_slot);
    }

    /* `from X import *` — bind every public export under its bare name.
       For Native: pushes each binding into `extern_index` for fast call.
       For Code: scans the module's parsed top-level for stored public
       names and emits LoadModule + LoadAttr + StoreName per name. */
    fn resolve_and_bind_star(&mut self, spec: &str, span: (usize, usize)) {
        let (_url, resolved) = match self.resolve_with_integrity(spec) {
            Ok(r) => r,
            Err(msg) => { self.error_at(span.0, span.1, &msg); return; }
        };
        match resolved {
            Resolved::Native { bindings, canonical } => {
                for b in &bindings {
                    let idx = self.chunk.extern_table.len() as u16;
                    self.chunk.extern_table.push(binding_to_extern(b));
                    self.chunk.extern_index.insert(b.name.clone(), idx);
                }
                let externs: Vec<crate::modules::vm::types::ExternFn> =
                    bindings.iter().map(binding_to_extern).collect();
                let _ = self.register_import(&canonical, ImportKind::Native(externs));
            }
            Resolved::Code { src, canonical } => {
                let Some(sub) = self.parse_or_get_cached(&canonical, &src, span) else { return; };
                let exports = Self::module_public_exports(&sub);
                let import_idx = self.register_import(&canonical, ImportKind::Code(sub));
                for name in &exports {
                    self.chunk.emit(OpCode::LoadModule, import_idx);
                    let attr_idx = self.chunk.push_name(name);
                    self.chunk.emit(OpCode::LoadAttr, attr_idx);
                    let v = self.increment_version(name);
                    let s = self.push_ssa_name(name, v);
                    self.chunk.emit(OpCode::StoreName, s);
                }
            }
        }
    }

}

