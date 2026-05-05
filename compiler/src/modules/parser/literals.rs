use crate::s;

use super::Parser;
use super::types::builtin;
use super::types::{OpCode, Value, SSAChunk, Instruction};

use crate::modules::lexer::{Token, TokenType, utf8_char_len};
use crate::modules::fx::FxHashMap as HashMap;

use alloc::{string::{String, ToString}, vec::Vec};

impl<'src, I: Iterator<Item = Token>> Parser<'src, I> {

    /* `{}`: dict literal, set literal, dict-comp, or set-comp.
       Always close via `eat(Rbrace)`: an unconditional `advance()` would
       silently swallow whatever's there if `}` is missing, hiding the
       unclosed-bracket error and leaving `bracket_stack` desynced. */
    pub(super) fn brace_literal(&mut self) {
        if matches!(self.peek(), Some(TokenType::Rbrace)) {
            self.advance();
            self.chunk.emit(OpCode::BuildDict, 0);
            return;
        }
        let key_start = self.chunk.instructions.len();
        self.expr();
        match self.peek() {
            Some(TokenType::Colon) => {
                self.advance();
                let val_start = self.chunk.instructions.len();
                self.expr();
                if matches!(self.peek(), Some(TokenType::For)) {
                    let versions_before = self.ssa_versions.clone();
                    let val_ins: Vec<Instruction> = self.chunk.instructions.drain(val_start..).collect();
                    let key_ins: Vec<Instruction> = self.chunk.instructions.drain(key_start..).collect();
                    self.chunk.emit(OpCode::BuildDict, 0);
                    self.comprehension_loop(&[key_ins, val_ins], OpCode::MapAdd, &versions_before);
                    self.eat(TokenType::Rbrace);
                } else {
                    let mut pairs = 1u16;
                    while self.eat_if(TokenType::Comma) {
                        if matches!(self.peek(), Some(TokenType::Rbrace)) { break; }
                        self.expr();
                        self.eat(TokenType::Colon);
                        self.expr();
                        pairs += 1;
                    }
                    self.eat(TokenType::Rbrace);
                    self.chunk.emit(OpCode::BuildDict, pairs);
                }
            }
            Some(TokenType::For) => {
                let versions_before = self.ssa_versions.clone();
                let elem_ins: Vec<Instruction> = self.chunk.instructions.drain(key_start..).collect();
                self.chunk.emit(OpCode::BuildSet, 0);
                self.comprehension_loop(&[elem_ins], OpCode::SetAdd, &versions_before);
                self.eat(TokenType::Rbrace);
            }
            _ => {
                let mut count = 1u16;
                while self.eat_if(TokenType::Comma) {
                    if matches!(self.peek(), Some(TokenType::Rbrace)) { break; }
                    self.expr();
                    count += 1;
                }
                self.eat(TokenType::Rbrace);
                self.chunk.emit(OpCode::BuildSet, count);
            }
        }
    }

    /* `[]`: list literal or list-comp. Same rationale as `brace_literal`:
       always close via `eat(Rsqb)`. */
    pub(super) fn list_literal(&mut self) {
        if matches!(self.peek(), Some(TokenType::Rsqb)) {
            self.advance();
            self.chunk.emit(OpCode::BuildList, 0);
            return;
        }
        let elem_start = self.chunk.instructions.len();
        self.expr();
        if matches!(self.peek(), Some(TokenType::For)) {
            let versions_before = self.ssa_versions.clone();
            let elem_ins: Vec<Instruction> = self.chunk.instructions.drain(elem_start..).collect();
            self.chunk.emit(OpCode::BuildList, 0);
            self.comprehension_loop(&[elem_ins], OpCode::ListAppend, &versions_before);
            self.eat(TokenType::Rsqb);
        } else {
            let mut count = 1u16;
            while self.eat_if(TokenType::Comma) {
                if matches!(self.peek(), Some(TokenType::Rsqb)) { break; }
                self.expr();
                count += 1;
            }
            self.eat(TokenType::Rsqb);
            self.chunk.emit(OpCode::BuildList, count);
        }
    }

    /* Build the for/if scaffolding for a comprehension and reinject the
       element body, rewriting SSA slot indices to the loop-bound versions. */
    pub(super) fn comprehension_loop(&mut self, elem_bodies: &[Vec<Instruction>], append_op: OpCode, versions_before: &HashMap<String, u32>) {
        let mut loop_starts: Vec<u16> = Vec::new();
        let mut for_iters: Vec<usize> = Vec::new();
        let mut all_vars: Vec<String> = Vec::new();

        while self.eat_if(TokenType::For) {
            let mut vars: Vec<String> = Vec::new();
            loop {
                vars.push(self.advance_text());
                if !self.eat_if(TokenType::Comma) { break; }
                if matches!(self.peek(), Some(TokenType::In)) { break; }
            }

            self.eat(TokenType::In);
            self.expr_bp(1);
            self.chunk.emit(OpCode::GetIter, 0);

            let ls = self.chunk.instructions.len() as u16;
            self.chunk.emit(OpCode::ForIter, 0);
            let fi = self.chunk.instructions.len() - 1;

            if vars.len() == 1 {
                self.store_name(vars[0].clone());
            } else {
                self.chunk.emit(OpCode::UnpackSequence, vars.len() as u16);
                for var in &vars {
                    self.store_name(var.clone());
                }
            }
            for v in &vars { all_vars.push(v.clone()); }

            while self.eat_if(TokenType::If) {
                self.expr_bp(1);
                self.chunk.emit(OpCode::JumpIfFalse, ls);
            }

            loop_starts.push(ls);
            for_iters.push(fi);
        }

        // Tiny mapping (typical size 1-5); linear scan beats HashMap and
        // avoids monomorphizing a hash table for u16 keys.
        let mut var_map: Vec<(u16, u16)> = Vec::new();
        for var in &all_vars {
            let old_ver = versions_before.get(var).copied().unwrap_or(0);
            let new_ver = self.current_version(var);
            if old_ver == new_ver { continue; }
            let mut ob = [0u8; 128];
            let old_name = Self::ssa_name(var, old_ver, &mut ob);
            let Some(&old_slot) = self.chunk.name_index.get(old_name) else { continue };
            let mut nb = [0u8; 128];
            let new_slot = self.chunk.push_name(Self::ssa_name(var, new_ver, &mut nb));
            var_map.push((old_slot, new_slot));
        }

        for body in elem_bodies {
            for ins in body {
                let operand = if matches!(ins.opcode, OpCode::LoadName | OpCode::StoreName) {
                    var_map.iter().find(|(k, _)| *k == ins.operand).map(|(_, v)| *v).unwrap_or(ins.operand)
                } else {
                    ins.operand
                };
                self.chunk.instructions.push(Instruction { opcode: ins.opcode, operand });
            }
        }
        self.chunk.emit(append_op, 0);

        for i in (0..for_iters.len()).rev() {
            self.chunk.emit(OpCode::Jump, loop_starts[i]);
            self.patch(for_iters[i]);
        }
    }

    /* F-string: alternates literal middles with `{expr[:spec]}` chunks
       until FstringEnd; emits BuildString to concatenate. `fs_start`/`fs_end`
       point at the opening `f"`/`f'` so we can anchor "f-string was never
       closed" if the lexer hit EOF before producing FstringEnd. */
    pub(super) fn fstring(&mut self, fs_start: usize, fs_end: usize) {
        let mut parts = 0u16;
        let mut got_end = false;
        if matches!(self.peek(), Some(TokenType::FstringEnd)) {
            self.advance();
            self.emit_const(Value::Str(String::new()));
            return;
        }
        loop {
            match self.peek() {
            Some(TokenType::FstringMiddle) => {
                let t = self.advance();
                let raw = self.lexeme(&t);
                let mut unescaped = String::with_capacity(raw.len());
                let bytes = raw.as_bytes();
                let mut i = 0;
                while i < bytes.len() {
                    if (bytes[i] == b'{' && bytes.get(i + 1) == Some(&b'{'))
                        || (bytes[i] == b'}' && bytes.get(i + 1) == Some(&b'}'))
                    {
                        unescaped.push(bytes[i] as char);
                        i += 2;
                    } else {
                        let ch_len = if bytes[i] < 0x80 { 1 } else { utf8_char_len(bytes[i]) };
                        unescaped.push_str(&raw[i..i + ch_len]);
                        i += ch_len;
                    }
                }
                self.emit_const(Value::Str(unescaped));
                parts += 1;
            }
                Some(TokenType::Lbrace) => {
                    self.advance();
                    self.expr();
                    /* Encoding for FormatValue operand:
                         bit 0       — has format-spec on stack
                         bits 1..=2  — conversion: 0 none, 1 !r, 2 !s, 3 !a
                       Mirrors CPython's `flags` byte in `FORMAT_VALUE`. Zero
                       operand still means "plain str()" — backwards-compatible. */
                    let mut flags = 0u16;
                    if matches!(self.peek(), Some(TokenType::Exclamation)) {
                        let bang = self.advance();
                        let conv_tok = self.advance();
                        let conv = self.lexeme(&conv_tok);
                        flags |= match conv {
                            "r" => 1 << 1,
                            "s" => 2 << 1,
                            "a" => 3 << 1,
                            _ => {
                                self.error_at(bang.start, conv_tok.end,
                                    "invalid f-string conversion (expected !r, !s, or !a)");
                                0
                            }
                        };
                    }
                    if matches!(self.peek(), Some(TokenType::Colon)) {
                        let colon = self.advance();
                        let spec_start = colon.end;
                        loop {
                            match self.tokens.peek().map(|t| t.kind) {
                                Some(TokenType::Rbrace) | None => break,
                                _ => { self.tokens.next(); }
                            }
                        }
                        let spec_end = self.tokens.peek().map(|t| t.start).unwrap_or(spec_start);
                        let spec = self.source[spec_start..spec_end].to_string();
                        let idx = self.chunk.push_const(Value::Str(spec));
                        self.chunk.emit(OpCode::LoadConst, idx);
                        flags |= 1;
                    }
                    self.chunk.emit(OpCode::FormatValue, flags);
                    parts += 1;
                    if matches!(self.peek(), Some(TokenType::Rbrace)) {
                        self.advance();
                    }
                }
                Some(TokenType::FstringEnd) => {
                    self.advance();
                    got_end = true;
                    break;
                }
                _ => break
            }
        }
        if !got_end {
            self.error_at(fs_start, fs_end, "f-string was never closed");
        }
        if parts > 0 {
            self.chunk.emit(OpCode::BuildString, parts);
        }
    }

    /* Dispatches `name(args)`: print/range get dedicated opcodes; other
       builtins map via the `builtin()` table; imported native bindings emit
       `CallExtern` (operand = (extern_idx << 8) | argc); rest fall through to
       the generic `LoadName + Call`. */
    pub(super) fn call(&mut self, name: String) -> bool {
        if name == "print" {
            let (pos, kw) = self.parse_args();
            self.chunk.emit(OpCode::CallPrint, pos + kw);
            return false;
        }

        if name == "range" {
            self.call_range();
            return true;
        }

        if let Some((op, leaves_value)) = builtin(name.as_str()) {
            let (pos, kw) = self.parse_args();
            self.chunk.emit(op, pos + kw);
            return leaves_value;
        }

        // Imported native binding: skip the heap allocation that LoadName/Call
        // would do and emit a direct CallExtern. The 16-bit operand packs
        // (extern_idx << 8) | argc — 256 externs per chunk and 256 positional
        // args, both well above realistic ceilings.
        if let Some(&extern_idx) = self.chunk.extern_index.get(&name) {
            let (pos, _kw) = self.parse_args();
            let encoded = (extern_idx << 8) | (pos & 0xFF);
            self.chunk.emit(OpCode::CallExtern, encoded);
            return true;
        }

        let i = self.push_ssa_name(&name, self.current_version(&name));
        self.chunk.emit(OpCode::LoadName, i);
        let (pos, kw) = self.parse_args();
        let encoded = ((kw & 0xFF) << 8) | (pos & 0xFF);
        self.chunk.emit(OpCode::Call, encoded);
        true
    }

    pub(super) fn call_range(&mut self) {
        self.advance();
        let mut argc = 0u16;
        while !matches!(self.peek(), Some(TokenType::Rpar) | None) {
            self.expr();
            argc += 1;
            self.eat_if(TokenType::Comma);
        }
        self.eat(TokenType::Rpar);
        self.chunk.emit(OpCode::CallRange, argc);
    }

    pub(super) fn parse_args(&mut self) -> (u16, u16) {
        self.advance();
        let mut pos = 0u16;
        let mut kw = 0u16;
        while !matches!(self.peek(), Some(TokenType::Rpar) | None) {
            let unpack = if self.eat_if(TokenType::DoubleStar) { Some(2u16) }
                         else if self.eat_if(TokenType::Star) { Some(1u16) }
                         else { None };
            if let Some(kind) = unpack {
                self.expr();
                self.chunk.emit(OpCode::UnpackArgs, kind);
                pos += 1;
            } else if matches!(self.peek(), Some(TokenType::Name)) {
                let t = self.advance();
                if matches!(self.peek(), Some(TokenType::Equal)) {
                    let kw_name = self.lexeme(&t).to_string();
                    self.advance();
                    let i = self.chunk.push_const(Value::Str(kw_name));
                    self.chunk.emit(OpCode::LoadConst, i);
                    self.expr();
                    kw += 1;
                } else {
                    let elem_start = self.chunk.instructions.len();
                    self.name(t);
                    self.infix_bp(0);
                    if matches!(self.peek(), Some(TokenType::For)) {
                        let versions_before = self.ssa_versions.clone();
                        let elem_ins: Vec<Instruction> = self.chunk.instructions.drain(elem_start..).collect();
                        self.chunk.emit(OpCode::BuildList, 0);
                        self.comprehension_loop(&[elem_ins], OpCode::ListAppend, &versions_before);
                    }
                    pos += 1;
                }
            } else {
                let elem_start = self.chunk.instructions.len();
                self.expr();
                if matches!(self.peek(), Some(TokenType::For)) {
                    let versions_before = self.ssa_versions.clone();
                    let elem_ins: Vec<Instruction> = self.chunk.instructions.drain(elem_start..).collect();
                    self.chunk.emit(OpCode::BuildList, 0);
                    self.comprehension_loop(&[elem_ins], OpCode::ListAppend, &versions_before);
                }
                pos += 1;
            }
            self.eat_if(TokenType::Comma);
        }
        self.eat(TokenType::Rpar);
        (pos, kw)
    }

    /* Class header + body in a fresh chunk + MakeClass + StoreName. */
    pub(super) fn class_def(&mut self) {
        // `class :` / `class (Foo):` etc. — push a non-syncing diagnostic
        // (the regular `eat(Colon)` below will pick up where we left off)
        // and synthesize a name so the body still parses. Without this,
        // advance_text would swallow the colon as the class name and produce
        // a misleading cascade.
        let cname = if matches!(self.peek(), Some(TokenType::Name)) {
            self.advance_text()
        } else {
            self.diag_at_peek("expected class name");
            "<missing>".to_string()
        };

        if self.eat_if(TokenType::Lpar) {
            while !matches!(self.peek(), Some(TokenType::Rpar) | None) {
                self.expr();
                if !self.eat_if(TokenType::Comma) { break; }
            }
            self.eat(TokenType::Rpar);
        }

        self.eat(TokenType::Colon);

        let body = self.with_fresh_chunk(|s| s.compile_block());

        let ci = self.chunk.classes.len() as u16;
        self.chunk.classes.push(body);
        self.chunk.emit(OpCode::MakeClass, ci);

        let ver = self.increment_version(&cname);
        let i = self.push_ssa_name(&cname, ver);
        self.chunk.emit(OpCode::StoreName, i);
    }

    /* `def` (sync or async): parse signature, compile body, emit
       MakeFunction / MakeCoroutine, apply decorators, then StoreName. */
    pub(super) fn func_def_inner(&mut self, decorators: u16, is_async: bool) {
        // `def (...)` / `def :` — same treatment as class_def: non-syncing
        // diagnostic + synthetic name so signature + body still parse.
        let fname = if matches!(self.peek(), Some(TokenType::Name)) {
            self.advance_text()
        } else {
            self.diag_at_peek("expected function name");
            "<missing>".to_string()
        };
        let (params, defaults) = self.parse_params();
        let body = self.compile_body(&params);

        let fi = self.chunk.functions.len() as u16;
        let name_slot = self.push_ssa_name(&fname, self.current_version(&fname) + 1);
        self.chunk.functions.push((params, body, defaults, name_slot));
        self.chunk.emit(if is_async { OpCode::MakeCoroutine } else { OpCode::MakeFunction }, fi);

        for _ in 0..decorators {
            self.chunk.emit(OpCode::Call, 1);
        }

        let ver = self.increment_version(&fname);
        let i = self.push_ssa_name(&fname, ver);
        self.chunk.emit(OpCode::StoreName, i);
    }

    pub(super) fn parse_params(&mut self) -> (Vec<String>, u16) {
        // Bail out early on `def name:` (no parens). The synthetic-name path
        // in `func_def_inner` lands here with peek() == Colon; without this
        // guard we'd swallow the colon as the `(` and then chase a phantom
        // `)` to EOF. Eat the trailing `:` ourselves so compile_body's
        // block lexer starts at Indent — otherwise the body parses the
        // colon as a statement and emits "expected expression".
        if !matches!(self.peek(), Some(TokenType::Lpar)) {
            self.diag_at_peek("expected '('");
            self.eat_if(TokenType::Colon);
            return (Vec::new(), 0);
        }
        self.advance();
        let mut params = Vec::new();
        let mut defaults = 0u16;
        // Also break on `Rarrow`: drain_annotation surfaces it when it ran past
        // a malformed annotation, and from here it means "params are over,
        // return type follows" — eat(Rpar) below will anchor at the unclosed `(`.
        while !matches!(self.peek(), Some(TokenType::Rpar | TokenType::Rarrow) | None) {
            if self.eat_if(TokenType::Slash) {
                self.eat_if(TokenType::Comma);
                continue;
            }
            let prefix = if self.eat_if(TokenType::DoubleStar) { "**" }
                         else if self.eat_if(TokenType::Star) { "*" }
                         else { "" };
            let nm = self.advance_text();
            params.push(if prefix.is_empty() { nm } else { s!(str prefix, str &nm) });
            self.drain_annotation();
            if prefix.is_empty() && self.eat_if(TokenType::Equal) {
                self.expr();
                defaults += 1;
            }
            self.eat_if(TokenType::Comma);
        }
        self.eat(TokenType::Rpar);
        if self.eat_if(TokenType::Rarrow) {
            while !matches!(self.peek(), Some(TokenType::Colon) | None) { self.advance(); }
        }
        if matches!(self.peek(), Some(TokenType::Colon)) { self.advance(); }
        (params, defaults)
    }

    /* Eat a parameter / variable annotation. Tracks its OWN bracket depth and
       uses `advance_raw` so the parser's structural `bracket_stack` stays
       clean — otherwise an unclosed annotation like `tuple[float | int,` would
       leave a stray `Lsqb` on the stack and confuse downstream `eat(Rpar)`.
       Breaks unconditionally on `Rarrow`: return-type arrow can't appear
       inside an annotation, so it reliably signals "we drifted past the
       annotation"; without this guard, depth would pin at 1 forever and the
       drain would silently consume the rest of the module. */
    pub(super) fn drain_annotation(&mut self) {
        if self.eat_if(TokenType::Colon) {
            let mut depth = 0u32;
            loop {
                match self.peek() {
                    None => break,
                    Some(TokenType::Rarrow) => break,
                    Some(TokenType::Lsqb | TokenType::Lpar | TokenType::Lbrace) => {
                        depth += 1;
                        self.advance_raw();
                    }
                    Some(TokenType::Rsqb | TokenType::Rpar | TokenType::Rbrace) => {
                        if depth == 0 { break; }
                        depth -= 1;
                        self.advance_raw();
                    }
                    Some(TokenType::Equal | TokenType::Comma) if depth == 0 => break,
                    _ => { self.advance_raw(); }
                }
            }
        }
    }

    pub(super) fn compile_body(&mut self, params: &[String]) -> SSAChunk {
        let mut body = self.with_fresh_chunk(|s| {
            for p in params {
                s.ssa_versions.insert(p.clone(), 0);
                let _ = s.push_ssa_name(p.trim_start_matches('*'), 0);
            }
            s.compile_block_body();
        });
        body.is_pure = !body.instructions.iter().any(|i| matches!(
            i.opcode,
            OpCode::CallPrint
            | OpCode::StoreItem
            | OpCode::DelItem
            | OpCode::StoreAttr
            | OpCode::CallInput
            | OpCode::Global
            | OpCode::Nonlocal
            | OpCode::LoadAttr
            | OpCode::Raise
            | OpCode::RaiseFrom
            | OpCode::Yield
            | OpCode::YieldFrom
        ));
        // Pre-compute is_generator once so exec_call avoids an O(n_instructions)
        // scan per call.
        body.is_generator = body.instructions.iter().any(|i| matches!(
            i.opcode,
            OpCode::Yield | OpCode::YieldFrom
        ));
        body
    }
}