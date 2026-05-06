/* Compile-time import resolution.

   Both `import X` and `from X import names` enter here. For Native modules,
   bindings are appended to chunk.extern_table; the call site emits CallExtern.
   For Code modules, requested functions and their same-module dependencies
   are inlined as MakeFunction + StoreName pairs in the parent chunk. */

use crate::s;

use super::Parser;
use super::types::{Diagnostic, OpCode, SSAChunk, Value, parse_string, ssa_strip};
use crate::modules::lexer::{Token, TokenType, lex};
use crate::modules::packages::{Resolved, binding_to_extern};
use crate::modules::fx::{FxHashMap, FxHashSet};

use alloc::{string::{String, ToString}, vec::Vec};
use core::sync::atomic::{AtomicU32, Ordering};

/* Process-wide unique splice ID. Each code-module splice grabs a fresh ID
   and uses it to build a name prefix `__edge_mod_N__`. The prefix isolates
   that module's top-level names in the parent chunk so two modules with
   identically-named private helpers don't clobber each other (the `bound`
   map and runtime `body_free_loads` resolution both key by bare name).
   Atomic so cross-thread compilation in test harnesses stays consistent. */
static SPLICE_ID: AtomicU32 = AtomicU32::new(0);

#[inline]
fn alloc_splice_prefix() -> String {
    let id = SPLICE_ID.fetch_add(1, Ordering::Relaxed);
    s!("__edge_mod_", int id, "__")
}

/* Bare names that must NOT be mangled even when stored at module top level.
   `__name__` is owned by the parser via `enter_imported_scope`/`leave_imported_scope`
   and by the VM init; mangling it would desync the parent's namespace.
   Other dunders (`__doc__`, `__all__`, `__file__`, ...) follow Python's
   reserved-attribute convention — leave them alone for the same reason
   nothing inside the inliner should reinterpret host-reserved names. */
#[inline]
fn is_reserved_bare(bare: &str) -> bool {
    bare.starts_with("__edge_mod_")
        || (bare.starts_with("__") && bare.ends_with("__") && bare.len() >= 4)
}

/* Walk a sub-chunk's top-level instructions to collect every bare name it
   stores (StoreName, Phi destinations, MakeFunction/MakeCoroutine name slots).
   These are exactly the names that need a module-private prefix to avoid
   colliding with same-named stores from sibling imports in the parent chunk.
   Reserved names (see `is_reserved_bare`) and names already prefixed by a
   nested splice are filtered out so mangling is idempotent across transitive
   imports. */
fn collect_module_stores(sub: &SSAChunk) -> FxHashSet<String> {
    let mut stores: FxHashSet<String> = FxHashSet::default();
    for ins in &sub.instructions {
        let slot_idx = match ins.opcode {
            OpCode::StoreName | OpCode::Phi => Some(ins.operand as usize),
            OpCode::MakeFunction | OpCode::MakeCoroutine => sub.functions
                .get(ins.operand as usize)
                .map(|f| f.3 as usize),
            _ => None,
        };
        let Some(s) = slot_idx else { continue };
        let Some(name) = sub.names.get(s) else { continue };
        let bare = ssa_strip(name);
        if is_reserved_bare(bare) { continue; }
        if !stores.contains(bare) { stores.insert(bare.to_string()); }
    }
    stores
}

/* Recursively rewrite SSA names in `chunk` and every nested function/class
   body, prepending `prefix` to any name whose bare form is in `stores`.
   The rewrite is purely string-level — slot indices, instruction operands,
   `prev_slots`, `alias_groups`, and `phi_sources` all reference slots by
   index and stay valid. `name_index` is rebuilt to keep deduping correct
   if the chunk is later mutated (the splicer never re-pushes onto a sub
   chunk, but rebuilding is cheap and prevents stale entries leaking out). */
fn rewrite_chunk_names(chunk: &mut SSAChunk, stores: &FxHashSet<String>, prefix: &str) {
    for n in chunk.names.iter_mut() {
        let bare = ssa_strip(n);
        if !stores.contains(bare) { continue; }
        let suffix_start = bare.len();
        let suffix = if suffix_start < n.len() { &n[suffix_start..] } else { "" };
        *n = s!(str prefix, str bare, str suffix);
    }
    chunk.name_index.clear();
    for (i, n) in chunk.names.iter().enumerate() {
        chunk.name_index.insert(n.clone(), i as u16);
    }
    for (_, body, _, _) in chunk.functions.iter_mut() {
        rewrite_chunk_names(body, stores, prefix);
    }
    for body in chunk.classes.iter_mut() {
        rewrite_chunk_names(body, stores, prefix);
    }
}

/* Mangle a parsed sub-chunk so its top-level names get a module-private
   prefix. Returns the set of un-prefixed bare names that were mangled —
   the call site uses this to know which user-requested imports correspond
   to a mangled key in the splicer's `bound` map. */
fn mangle_module(sub: &mut SSAChunk, prefix: &str) -> FxHashSet<String> {
    let stores = collect_module_stores(sub);
    rewrite_chunk_names(sub, &stores, prefix);
    stores
}

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

    /* `from X import a, b, c` — resolve X, then bind the named exports. */
    fn resolve_and_bind_named(&mut self, spec: &str, span: (usize, usize), names: Vec<(String, String)>) {
        let resolved = match self.resolver.resolve(spec) {
            Ok(r) => r,
            Err(msg) => { self.error_at(span.0, span.1, &msg); return; }
        };
        match resolved {
            Resolved::Native(bindings) => {
                for (name, alias) in names {
                    let Some(b) = bindings.iter().find(|b| b.name == name) else {
                        self.error_at(span.0, span.1,
                            &s!("module '", str spec, "' has no export '", str &name, "'"));
                        continue;
                    };
                    let idx = self.chunk.extern_table.len() as u16;
                    self.chunk.extern_table.push(binding_to_extern(b));
                    self.chunk.extern_index.insert(alias, idx);
                }
            }
            Resolved::Code(src) => {
                self.inline_code_module(spec, span, &src, &names);
            }
        }
    }

    /* `import X` — resolve X, build a `HeapObj::Module` containing every
       export, and store it under `alias`. Native exports are also recorded
       in `extern_index` under their bare name so the parser can still emit
       a direct CallExtern when the user calls `alias.name(...)` (the runtime
       Module dispatch is the fallback for "stash a module value as a
       first-class object" cases). */
    fn resolve_and_bind_all(&mut self, spec: &str, span: (usize, usize), alias: &str) {
        let resolved = match self.resolver.resolve(spec) {
            Ok(r) => r,
            Err(msg) => { self.error_at(span.0, span.1, &msg); return; }
        };
        match resolved {
            Resolved::Native(bindings) => {
                self.emit_native_module(spec, &bindings);
            }
            Resolved::Code(src) => {
                self.emit_code_module(spec, span, &src);
            }
        }
        let alias_ver = self.increment_version(alias);
        let alias_slot = self.push_ssa_name(alias, alias_ver);
        self.chunk.emit(OpCode::StoreName, alias_slot);
    }

    /* `from X import *` — resolve X, bind every export under its bare name
       in the importer's scope. Like Python's star-import: side-effects of the
       module's top level still run, and each public name becomes locally
       visible. */
    fn resolve_and_bind_star(&mut self, spec: &str, span: (usize, usize)) {
        let resolved = match self.resolver.resolve(spec) {
            Ok(r) => r,
            Err(msg) => { self.error_at(span.0, span.1, &msg); return; }
        };
        match resolved {
            Resolved::Native(bindings) => {
                for b in &bindings {
                    let idx = self.chunk.extern_table.len() as u16;
                    self.chunk.extern_table.push(binding_to_extern(b));
                    self.chunk.extern_index.insert(b.name.clone(), idx);
                }
            }
            Resolved::Code(src) => {
                let (tokens, lex_errs) = lex(&src);
                let owned = src.clone();
                let mut sub_parser = Parser::with_resolver(
                    &owned, tokens.into_iter(), self.resolver.child(spec)
                );
                for e in lex_errs {
                    sub_parser.errors.push(Diagnostic {
                        start: e.start, end: e.end, msg: e.msg.to_string(),
                    });
                }
                let (mut sub, errs) = sub_parser.parse();
                if !errs.is_empty() {
                    self.error_at(span.0, span.1,
                        &s!("module '", str spec, "' parse error: ", str &errs[0].msg));
                    return;
                }
                let prefix = alloc_splice_prefix();
                let _exports = mangle_module(&mut sub, &prefix);
                self.enter_imported_scope(spec);
                let bound = self.splice_top_level(&sub);
                self.leave_imported_scope();
                /* Star-import: bind every export under its public bare name
                   in the importer's scope. Mangling moved the splicer's stores
                   under prefixed keys, so without this rebind the bare names
                   would be invisible — defeating the point of `import *`. */
                let mut entries: Vec<(String, u16)> = bound.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                for (k, slot) in &entries {
                    let public = k.strip_prefix(prefix.as_str()).unwrap_or(k.as_str());
                    self.chunk.emit(OpCode::LoadName, *slot);
                    let v = self.increment_version(public);
                    let s = self.push_ssa_name(public, v);
                    self.chunk.emit(OpCode::StoreName, s);
                }
            }
        }
    }

    /* Emit the bytecode that, at runtime, builds a `HeapObj::Module` from
       a native module's bindings and leaves it on the stack ready for
       StoreName. Natives are also added to `extern_table` so direct
       call-site fusion (`alias.name(...)` → CallExtern) can short-circuit
       the runtime module attribute lookup when desirable. */
    fn emit_native_module(&mut self, spec: &str, bindings: &[crate::modules::packages::NativeBinding]) {
        for b in bindings {
            let idx = self.chunk.extern_table.len() as u16;
            self.chunk.extern_table.push(binding_to_extern(b));
            // No extern_index insert: the bindings live under the module's
            // attr lookup at runtime, not under flat names in this scope.
            let name_const = self.chunk.push_const(Value::Str(b.name.clone()));
            self.chunk.emit(OpCode::LoadConst, name_const);
            self.chunk.emit(OpCode::LoadExtern, idx);
        }
        let mod_name = self.chunk.push_const(Value::Str(spec.to_string()));
        self.chunk.emit(OpCode::LoadConst, mod_name);
        self.chunk.emit(OpCode::BuildModule, bindings.len() as u16);
    }

    /* Splice the code module's top level (so its side-effects run and its
       names become available locally), then read each top-level binding
       back via LoadName and weave them into a `HeapObj::Module`. */
    fn emit_code_module(&mut self, spec: &str, span: (usize, usize), src: &str) {
        let (tokens, lex_errs) = lex(src);
        let owned = src.to_string();
        let mut sub_parser = Parser::with_resolver(
            &owned, tokens.into_iter(), self.resolver.child(spec)
        );
        for e in lex_errs {
            sub_parser.errors.push(Diagnostic {
                start: e.start, end: e.end, msg: e.msg.to_string(),
            });
        }
        let (mut sub, errs) = sub_parser.parse();
        if !errs.is_empty() {
            self.error_at(span.0, span.1,
                &s!("module '", str spec, "' parse error: ", str &errs[0].msg));
            return;
        }
        let prefix = alloc_splice_prefix();
        let _exports = mangle_module(&mut sub, &prefix);
        self.enter_imported_scope(spec);
        let bound = self.splice_top_level(&sub);
        self.leave_imported_scope();
        // Strip the splice prefix off each bound key to recover the public
        // attribute name the module should expose. Reserved names (dunders)
        // were never prefixed and pass through unchanged.
        let mut entries: Vec<(String, u16)> = bound.into_iter().map(|(k, v)| {
            let public = k.strip_prefix(prefix.as_str())
                .map(|s| s.to_string())
                .unwrap_or(k);
            (public, v)
        }).collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, slot) in &entries {
            let name_const = self.chunk.push_const(Value::Str(name.clone()));
            self.chunk.emit(OpCode::LoadConst, name_const);
            self.chunk.emit(OpCode::LoadName, *slot);
        }
        let mod_name = self.chunk.push_const(Value::Str(spec.to_string()));
        self.chunk.emit(OpCode::LoadConst, mod_name);
        self.chunk.emit(OpCode::BuildModule, entries.len() as u16);
    }

    /* Splice a code module's top level into the current chunk.

       Python's `from m import names` runs the module body and then binds the
       requested names locally. We mirror that: every top-level statement
       (assignments, classes, defs, expression statements) gets transcribed
       into the parent chunk with operand indices remapped. Then for each
       requested name we look up its bound parent slot and, if the user
       supplied an alias, add a LoadName/StoreName pair under the alias.

       Operand remapping is opcode-driven (see `splice_top_level`). The sub
       chunk is parsed with a child resolver derived from `self.resolver`, so
       its transitive imports resolve through the same root `packages.json`
       and module cache as the entry script. */
    fn inline_code_module(
        &mut self,
        spec: &str,
        span: (usize, usize),
        src: &str,
        names: &[(String, String)],
    ) {
        let (tokens, lex_errs) = lex(src);
        let owned_src = src.to_string();
        let mut sub_parser = Parser::with_resolver(
            &owned_src, tokens.into_iter(), self.resolver.child(spec)
        );
        for e in lex_errs {
            sub_parser.errors.push(Diagnostic {
                start: e.start, end: e.end, msg: e.msg.to_string(),
            });
        }
        let (mut sub_chunk, errs) = sub_parser.parse();
        if !errs.is_empty() {
            self.error_at(span.0, span.1,
                &s!("module '", str spec, "' parse error: ", str &errs[0].msg));
            return;
        }

        // Mangle the sub-chunk's top-level names with a unique prefix so its
        // private helpers can't collide with sibling modules' helpers in the
        // parent chunk's namespace. The exports set tells us which requested
        // names need the prefix when looking up `bound`.
        let prefix = alloc_splice_prefix();
        let exports = mangle_module(&mut sub_chunk, &prefix);

        self.enter_imported_scope(spec);
        let bound = self.splice_top_level(&sub_chunk);
        self.leave_imported_scope();

        for (name, alias) in names {
            // Reserved names (dunders) skip mangling and live under their bare
            // form in `bound`; everything else lives under the mangled key.
            let key: String = if exports.contains(name.as_str()) {
                s!(str &prefix, str name)
            } else {
                name.clone()
            };
            let Some(&src_slot) = bound.get(key.as_str()) else {
                self.error_at(span.0, span.1,
                    &s!("module '", str spec, "' has no export '", str name, "'"));
                continue;
            };
            // Always rebind under the importer-visible name (bare or alias).
            // Without mangling the splicer left bare names directly in scope;
            // with mangling the bare name only exists if we emit this pair.
            self.chunk.emit(OpCode::LoadName, src_slot);
            let alias_ver = self.increment_version(alias);
            let alias_slot = self.push_ssa_name(alias, alias_ver);
            self.chunk.emit(OpCode::StoreName, alias_slot);
        }
    }

    /* Rebind `__name__` to the module spec for the duration of a code-module
       splice. CPython convention: a module's top-level body sees its own
       name in `__name__`, not "__main__"; that's what makes the
       `if __name__ == "__main__":` guard skip when the module is imported.
       Pushes the parent's current `__name__` value first so `leave_imported_scope`
       can restore it without hard-coding "__main__" — preserves any earlier
       user reassignment. */
    fn enter_imported_scope(&mut self, spec: &str) {
        let save_slot = self.push_ssa_name("__name__", self.current_version("__name__"));
        self.chunk.emit(OpCode::LoadName, save_slot);
        let spec_const = self.chunk.push_const(Value::Str(spec.to_string()));
        self.chunk.emit(OpCode::LoadConst, spec_const);
        let new_ver = self.increment_version("__name__");
        let new_slot = self.push_ssa_name("__name__", new_ver);
        self.chunk.emit(OpCode::StoreName, new_slot);
    }

    /* Pop the saved `__name__` (pushed by `enter_imported_scope`) into a fresh
       SSA version, so post-import code reads the parent's original value
       again. */
    fn leave_imported_scope(&mut self) {
        let new_ver = self.increment_version("__name__");
        let new_slot = self.push_ssa_name("__name__", new_ver);
        self.chunk.emit(OpCode::StoreName, new_slot);
    }

    /* Walk a sub-chunk's top-level instructions and re-emit them into the
       caller's chunk, remapping every operand that indexes into a per-chunk
       table (constants, names, functions, classes, jump targets, phi
       sources). Returns a map of bare-name → parent slot for every name the
       sub bound at top level (so the importer can wire up aliases without
       a second pass). */
    fn splice_top_level(&mut self, sub: &SSAChunk) -> FxHashMap<String, u16> {
        // sub_slot → parent_slot for slots that the sub stored into. LoadName
        // entries that aren't here are assumed to refer to parent's existing
        // scope (builtins / module globals seeded before the import).
        let mut slot_map: FxHashMap<u16, u16> = FxHashMap::default();
        // sub_const → parent_const, lazily filled.
        let mut const_map: FxHashMap<u16, u16> = FxHashMap::default();
        // Latest parent slot for a given bare name (the value the importer's
        // alias rebind uses).
        let mut bound: FxHashMap<String, u16> = FxHashMap::default();

        let base = self.chunk.instructions.len() as u16;
        let mut phi_seen: usize = 0;

        /* `Parser::parse` always appends a trailing `ReturnValue` to mark
           end-of-chunk; splicing it would cut the parent module short. */
        let last = sub.instructions.len().saturating_sub(1);
        let body_end = if sub.instructions.get(last).map(|i| i.opcode) == Some(OpCode::ReturnValue) {
            last
        } else {
            sub.instructions.len()
        };

        for ins in &sub.instructions[..body_end] {
            let new_op = match ins.opcode {
                OpCode::LoadConst => {
                    *const_map.entry(ins.operand).or_insert_with(|| {
                        self.chunk.push_const(sub.constants[ins.operand as usize].clone())
                    })
                }
                OpCode::LoadName => {
                    self.remap_load_slot(sub, ins.operand, &slot_map)
                }
                OpCode::StoreName => {
                    self.remap_store_slot(sub, ins.operand, &mut slot_map, &mut bound)
                }
                OpCode::Del => {
                    self.remap_load_slot(sub, ins.operand, &slot_map)
                }
                OpCode::Phi => {
                    let (sa, sb) = sub.phi_sources[phi_seen];
                    phi_seen += 1;
                    let pa = self.remap_load_slot(sub, sa, &slot_map);
                    let pb = self.remap_load_slot(sub, sb, &slot_map);
                    self.chunk.phi_sources.push((pa, pb));
                    self.remap_store_slot(sub, ins.operand, &mut slot_map, &mut bound)
                }
                OpCode::MakeFunction | OpCode::MakeCoroutine => {
                    let (params, body, defaults, name_slot) = sub.functions[ins.operand as usize].clone();
                    let bare = ssa_strip(&sub.names[name_slot as usize]).to_string();
                    let parent_ver = self.current_version(&bare) + 1;
                    let parent_name_slot = self.push_ssa_name(&bare, parent_ver);
                    let new_fi = self.chunk.functions.len() as u16;
                    self.chunk.functions.push((params, body, defaults, parent_name_slot));
                    new_fi
                }
                OpCode::MakeClass => {
                    let body = sub.classes[ins.operand as usize].clone();
                    let new_ci = self.chunk.classes.len() as u16;
                    self.chunk.classes.push(body);
                    new_ci
                }
                OpCode::Jump | OpCode::JumpIfFalse
                | OpCode::JumpIfFalseOrPop | OpCode::JumpIfTrueOrPop
                | OpCode::ForIter | OpCode::SetupExcept => {
                    ins.operand.checked_add(base)
                        .unwrap_or_else(|| { self.chunk.overflow = true; 0 })
                }
                _ => ins.operand,
            };
            self.chunk.emit(ins.opcode, new_op);
        }
        bound
    }

    /* Map a sub-chunk slot used as a load (operand in LoadName, Del, or a
       Phi source). If the sub stored into this slot we already have a parent
       counterpart in `slot_map`; otherwise the load resolves to the bare
       name in parent's current SSA frame (e.g., a builtin like `print`). */
    fn remap_load_slot(
        &mut self,
        sub: &SSAChunk,
        sub_slot: u16,
        slot_map: &FxHashMap<u16, u16>,
    ) -> u16 {
        if let Some(&p) = slot_map.get(&sub_slot) { return p; }
        let bare = ssa_strip(&sub.names[sub_slot as usize]).to_string();
        let ver = self.current_version(&bare);
        self.push_ssa_name(&bare, ver)
    }

    /* Map a sub-chunk slot used as a store target (operand in StoreName or
       Phi destination). Allocates a fresh parent SSA version for the bare
       name, records the sub→parent slot mapping for later loads, and tracks
       the most-recent slot so the importer can wire up aliases by name. */
    fn remap_store_slot(
        &mut self,
        sub: &SSAChunk,
        sub_slot: u16,
        slot_map: &mut FxHashMap<u16, u16>,
        bound: &mut FxHashMap<String, u16>,
    ) -> u16 {
        let bare = ssa_strip(&sub.names[sub_slot as usize]).to_string();
        let ver = self.increment_version(&bare);
        let parent_slot = self.push_ssa_name(&bare, ver);
        slot_map.insert(sub_slot, parent_slot);
        bound.insert(bare, parent_slot);
        parent_slot
    }
}

