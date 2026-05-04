/* Compile-time import resolution.

   Both `import X` and `from X import names` enter here. For Native modules,
   bindings are appended to chunk.extern_table; the call site emits CallExtern.
   For Code modules, requested functions and their same-module dependencies
   are inlined as MakeFunction + StoreName pairs in the parent chunk. */

use crate::s;

use super::Parser;
use super::types::{OpCode, SSAChunk, parse_string, ssa_strip};
use crate::modules::lexer::{Token, TokenType, lex};
use crate::modules::packages::{Resolved, NoopResolver, binding_to_extern};
use crate::modules::fx::FxHashSet;

use alloc::{boxed::Box, string::{String, ToString}, vec::Vec};

impl<'src, I: Iterator<Item = Token>> Parser<'src, I> {

    /* `import name [as alias][, ...]`. Bare `import` only — no dotted module
       paths and no string-form spec (those belong on `from`). Each name is
       treated as if the user had written `from <name> import <name>`: the
       resolver is asked for `<name>`, and every export it returns is bound
       in the local scope. Useful when a host wants to expose a small module
       as a flat set of functions. */
    pub(super) fn do_import_stmt(&mut self) {
        self.advance(); // 'import'
        loop {
            let (spec, span) = self.read_module_spec();
            let alias = if self.eat_if(TokenType::As) {
                self.advance_text()
            } else {
                spec.split('.').next().unwrap_or(&spec).to_string()
            };
            // For now: only natives are supported via plain `import`. Code
            // modules require `from X import names` because we don't have a
            // namespace-object opcode to hold module attributes.
            self.resolve_and_bind_all(&spec, span, Some(alias));
            if !self.eat_if(TokenType::Comma) { break; }
        }
    }

    /* `from <spec> import <name>[ as <alias>][, ...]`. The spec is either a
       quoted string ("https://...", "./utils.py") or a bare identifier
       (which the resolver looks up in the host's import map). */
    pub(super) fn do_from_stmt(&mut self) {
        self.advance(); // 'from'
        let (spec, spec_span) = self.read_module_spec();
        self.eat(TokenType::Import);

        if self.eat_if(TokenType::Star) {
            self.error_at(spec_span.0, spec_span.1,
                "'from X import *' is not supported");
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

    /* `import X` — resolve X, bind every export under the (single) alias. For
       natives, every binding is exposed directly. Code modules aren't
       supported via plain `import` (no namespace object); the resolver's
       Code variant is rejected with a clear diagnostic. */
    fn resolve_and_bind_all(&mut self, spec: &str, span: (usize, usize), _alias: Option<String>) {
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
            Resolved::Code(_) => {
                self.error_at(span.0, span.1,
                    "code modules require 'from X import names'; plain 'import X' is native-only");
            }
        }
    }

    /* Splice a code module's `def` definitions into the current chunk.

       For each requested name we recursively pull in same-module helpers it
       references (LoadName operands inside the body that resolve to a sibling
       def). The `inlined` set deduplicates and breaks cycles. Helpers come
       in under their bare name; the user's chosen alias only applies to the
       name they explicitly imported. */
    fn inline_code_module(
        &mut self,
        spec: &str,
        span: (usize, usize),
        src: &str,
        names: &[(String, String)],
    ) {
        let (tokens, _lex_errs) = lex(src);
        let owned_src = src.to_string();
        let (sub_chunk, errs) = Parser::with_resolver(
            &owned_src, tokens.into_iter(), Box::new(NoopResolver)
        ).parse();
        if !errs.is_empty() {
            self.error_at(span.0, span.1,
                &s!("module '", str spec, "' parse error: ", str &errs[0].msg));
            return;
        }

        let mut inlined: FxHashSet<String> = FxHashSet::default();
        for (name, alias) in names {
            if find_fn(&sub_chunk, name).is_none() {
                self.error_at(span.0, span.1,
                    &s!("module '", str spec, "' has no function '", str name, "'"));
                continue;
            }
            self.inline_with_deps(&sub_chunk, name, Some(alias.as_str()), &mut inlined);
        }
    }

    /* Inline `bare_name` from `sub_chunk`, recursing into same-module helpers
       it calls. Bound under `alias` if given, else under `bare_name`. Marks
       names in `inlined` BEFORE recursing so mutual recursion (a → b → a)
       terminates and shared helpers aren't duplicated. */
    fn inline_with_deps(
        &mut self,
        sub_chunk: &SSAChunk,
        bare_name: &str,
        alias: Option<&str>,
        inlined: &mut FxHashSet<String>,
    ) {
        if !inlined.insert(bare_name.to_string()) { return; }
        let Some(fi_in_sub) = find_fn(sub_chunk, bare_name) else { return; };
        let (params, body, defaults, _) = sub_chunk.functions[fi_in_sub].clone();

        // Walk the body for LoadName operands that match a sibling def.
        // Local variables and parameters also use LoadName, but their stripped
        // names won't appear in the sub-module's def table — so the lookup
        // skips them naturally.
        for ins in &body.instructions {
            if ins.opcode != OpCode::LoadName { continue; }
            let Some(loaded) = body.names.get(ins.operand as usize) else { continue };
            let bare = ssa_strip(loaded);
            if bare != bare_name && find_fn(sub_chunk, bare).is_some() {
                self.inline_with_deps(sub_chunk, bare, None, inlined);
            }
        }

        let bind = alias.unwrap_or(bare_name);
        let parent_ver = self.increment_version(bind);
        let parent_slot = self.push_ssa_name(bind, parent_ver);
        let new_fi = self.chunk.functions.len() as u16;
        self.chunk.functions.push((params, body, defaults, parent_slot));
        self.chunk.emit(OpCode::MakeFunction, new_fi);
        self.chunk.emit(OpCode::StoreName, parent_slot);
    }
}

/* Find a top-level `def` in a sub-chunk by its bare (SSA-stripped) name.
   Returns its index in `sub_chunk.functions`. */
fn find_fn(sub_chunk: &SSAChunk, bare_name: &str) -> Option<usize> {
    sub_chunk.functions.iter().position(|(_, _, _, name_slot)| {
        sub_chunk.names.get(*name_slot as usize)
            .map(|n| ssa_strip(n) == bare_name)
            .unwrap_or(false)
    })
}
