use crate::s;

use super::Parser;
use super::types::OpCode;

use crate::modules::lexer::{Token, TokenType};

use alloc::{vec, vec::Vec, string::ToString};

impl<'src, I: Iterator<Item = Token>> Parser<'src, I> {

    /* if / elif / else with SSA Phi at branch joins. */

    pub(super) fn if_stmt(&mut self) {
        self.advance();
        self.enter_block();
        self.if_body();
        self.commit_block();
    }

    pub(super) fn if_body(&mut self) {
        self.expr();
        self.chunk.emit(OpCode::JumpIfFalse, 0);
        let jf = self.chunk.instructions.len() - 1;

        self.eat(TokenType::Colon);
        self.compile_block();

        match self.peek() {
            Some(TokenType::Elif) => {
                self.advance();
                self.chunk.emit(OpCode::Jump, 0);
                let jmp = self.chunk.instructions.len() - 1;
                self.mid_block();
                self.patch(jf);
                self.if_body();
                self.patch(jmp);
            }
            Some(TokenType::Else) => {
                self.advance();
                self.chunk.emit(OpCode::Jump, 0);
                let jmp = self.chunk.instructions.len() - 1;
                self.mid_block();
                self.patch(jf);
                self.eat(TokenType::Colon);
                self.compile_block();
                self.patch(jmp);
            }
            _ => {
                self.patch(jf);
            }
        }
    }

    /* match/case: full pattern matcher.

       Supports: literals, capture variables (`case x:`), `_` wildcard,
       OR patterns (`case 1 | 2 | 3:`), guards (`case x if x > 0:`), and
       sequence patterns (`case [a, b, *rest]:`). Mapping and class
       patterns are not implemented — use chained `if/elif` for those.

       Bytecode shape per case:
         LoadName subj          ; subject onto stack for the matcher
         <pattern emit>         ; consumes subject; on miss, jumps via fail_jumps
         <guard emit>           ; optional; on false, jumps via fail_jumps
         <body>
         Jump end
         <fail_jumps land here>
    */
    pub(super) fn match_stmt(&mut self) {
        self.advance();
        self.expr();

        let ver = self.increment_version("#match");
        let subj = self.chunk.push_name(&s!("#match", int ver));
        self.chunk.emit(OpCode::StoreName, subj);

        self.eat(TokenType::Colon);
        self.eat_if(TokenType::Indent);

        let mut end_jumps = Vec::new();

        while matches!(self.peek(), Some(TokenType::Case)) {
            self.advance();

            let mut fail_jumps: Vec<usize> = Vec::new();
            self.parse_pattern(subj, &mut fail_jumps);

            // Optional guard. The pattern's fail_jumps and the guard's jump
            // share the same landing pad: fall through to the next case.
            if self.eat_if(TokenType::If) {
                self.expr();
                self.chunk.emit(OpCode::JumpIfFalse, 0);
                fail_jumps.push(self.chunk.instructions.len() - 1);
            }

            self.eat(TokenType::Colon);
            self.compile_block();

            self.chunk.emit(OpCode::Jump, 0);
            end_jumps.push(self.chunk.instructions.len() - 1);

            for j in fail_jumps { self.patch(j); }
        }

        self.eat_if(TokenType::Dedent);

        for pos in end_jumps { self.patch(pos); }
    }

    /* Recursive pattern parser. Emits matcher bytecode for one pattern,
       extending `fail_jumps` with every JumpIfFalse / Jump that should
       branch to the case-fail label. The subject is reloaded from `subj`
       at the top of the matcher; the matcher consumes it (success path)
       or jumps to fail (mismatch). */
    pub(super) fn parse_pattern(&mut self, subj: u16, fail_jumps: &mut Vec<usize>) {
        // OR pattern: parse one alternative, then while peek `|` parse more.
        // Each alt has its own success-jump that lands at the post-OR point.
        let alt_start = self.chunk.instructions.len();
        let _ = alt_start; // kept for symmetry; alts are linked via fail_jumps below

        let mut alts: Vec<Vec<usize>> = Vec::new();
        let mut succ_jumps: Vec<usize> = Vec::new();

        loop {
            let mut this_alt_fails: Vec<usize> = Vec::new();
            self.parse_simple_pattern(subj, &mut this_alt_fails);
            // Match: jump past remaining alts (ahead).
            self.chunk.emit(OpCode::Jump, 0);
            succ_jumps.push(self.chunk.instructions.len() - 1);
            // Mismatch: rewire this alt's fails to land at the next alt
            // (here). Don't propagate to the case-fail until the LAST alt.
            alts.push(this_alt_fails);
            if !matches!(self.peek(), Some(TokenType::Vbar)) {
                break;
            }
            // Land previous alt's fails here.
            let here = self.chunk.instructions.len();
            for j in alts.last_mut().unwrap().drain(..) {
                let target = here as u16;
                self.chunk.instructions[j].operand = target;
            }
            self.advance(); // consume |
        }

        // Last alt's fails are the case-fails (no further alts to try).
        if let Some(last) = alts.last_mut() {
            fail_jumps.extend(last.drain(..));
        }

        // Patch all success jumps to land here (post-OR).
        for j in succ_jumps { self.patch(j); }
    }

    /* One alternative within a pattern (no `|`). Dispatches by leading
       token kind. Captures bind via `StoreName` against the subject. */
    fn parse_simple_pattern(&mut self, subj: u16, fail_jumps: &mut Vec<usize>) {
        match self.peek() {
            // `_` wildcard — always succeeds, no binding.
            Some(TokenType::Underscore) => { self.advance(); }
            // `[ ... ]` sequence pattern.
            Some(TokenType::Lsqb) => { self.parse_sequence_pattern(subj, fail_jumps); }
            // Bare identifier — capture: bind subject to name, always succeed.
            Some(TokenType::Name) => {
                let t = self.advance();
                let name = self.lexeme(&t).to_string();
                self.chunk.emit(OpCode::LoadName, subj);
                let ver = self.increment_version(&name);
                let i = self.push_ssa_name(&name, ver);
                self.chunk.emit(OpCode::StoreName, i);
            }
            // Anything else (literal / parenthesised expr): equality test
            // against the subject. Use a precedence above bitwise-or so the
            // pattern `1 | 2 | 3` is consumed by the OR loop in
            // `parse_pattern` rather than as a single `1 | 2 | 3`
            // bitwise-or expression.
            _ => {
                self.chunk.emit(OpCode::LoadName, subj);
                self.expr_bp(11);
                self.chunk.emit(OpCode::Eq, 0);
                self.chunk.emit(OpCode::JumpIfFalse, 0);
                fail_jumps.push(self.chunk.instructions.len() - 1);
            }
        }
    }

    /* `[a, b, c]` and `[a, *rest, c]` sequence patterns. Each item is a
       full pattern (literal, capture, _, OR, ...). Length-checks the
       subject, then materialises subj[i] into a fresh slot so the item
       pattern can recurse via parse_simple_pattern.

       Star patterns capture the middle slice into a list and may name it
       (`*rest`) or wildcard (`*_`). At most one star per sequence. */
    fn parse_sequence_pattern(&mut self, subj: u16, fail_jumps: &mut Vec<usize>) {
        self.advance(); // [
        // Each item is a (star?, start_token_position) — the actual pattern
        // bytecode is emitted lazily after we know the indices.
        let item_positions: Vec<bool> = Vec::new(); // star flag per item
        // Phase 1: count the items + locate the star (if any) by peeking at
        // the token stream and skipping past each item via parse_simple_pattern
        // is wrong (it would emit bytecode). Instead, parse the patterns
        // into temporary slot expressions by recording where each item begins
        // in the source and re-parsing later. Simpler: emit the equality /
        // capture inline against subj[i] in a single forward pass.
        let _ = item_positions;

        // Track items as we go: emit length check first, then per-item
        // bytecode that reads subj[i] (or the star slice) into a sub-subject
        // and recursively runs the item pattern. To keep things linear we
        // actually do TWO passes by buffering the parser position.
        //
        // Implementation: count items in a token-only pass (no bytecode),
        // then walk again emitting the per-index bytecode. We achieve the
        // first pass by peeking: lookahead until matching `]`, counting
        // commas at depth 0 and detecting one `*`.

        // First pass: scan ahead for item count + star index, save the
        // tokens we consumed so we can re-feed them.
        let mut buffered: Vec<crate::modules::lexer::Token> = Vec::new();
        let mut depth: i32 = 0;
        let mut commas = 0;
        let mut empty = true;
        let mut star_count = 0;
        loop {
            match self.peek() {
                Some(TokenType::Rsqb) if depth == 0 => break,
                None => break,
                Some(TokenType::Lpar | TokenType::Lsqb | TokenType::Lbrace) => {
                    depth += 1; empty = false;
                    buffered.push(self.advance());
                }
                Some(TokenType::Rpar | TokenType::Rsqb | TokenType::Rbrace) => {
                    depth -= 1;
                    buffered.push(self.advance());
                }
                Some(TokenType::Comma) if depth == 0 => {
                    commas += 1; empty = false;
                    buffered.push(self.advance());
                }
                Some(TokenType::Star) if depth == 0 => {
                    star_count += 1; empty = false;
                    buffered.push(self.advance());
                }
                _ => { empty = false; buffered.push(self.advance()); }
            }
        }
        let item_count = if empty { 0 } else { commas + 1 };
        self.eat(TokenType::Rsqb);

        if star_count > 1 {
            self.error_at(buffered[0].start, buffered.last().unwrap().end,
                "multiple stars in sequence pattern");
        }

        // Length check: == item_count if no star, >= item_count - 1 if star.
        let len_min = if star_count > 0 { item_count - 1 } else { item_count };
        self.chunk.emit(OpCode::LoadName, subj);
        self.chunk.emit(OpCode::CallLen, 0);
        let ci = self.chunk.push_const(super::types::Value::Int(len_min as i64));
        self.chunk.emit(OpCode::LoadConst, ci);
        let cmp = if star_count > 0 { OpCode::GtEq } else { OpCode::Eq };
        self.chunk.emit(cmp, 0);
        self.chunk.emit(OpCode::JumpIfFalse, 0);
        fail_jumps.push(self.chunk.instructions.len() - 1);

        // Phase 2: re-feed buffered tokens through a sub-parser-like state.
        // Easiest: create an iterator from buffered and swap with self.tokens
        // for the duration of the item walk. The lexer Token type is plain
        // data so this is mechanical.
        let saved: Vec<crate::modules::lexer::Token> = buffered;
        // Iterator over saved tokens that we can `peek/advance` against.
        let mut idx = 0usize;
        let total = saved.len();

        // Fresh slot to use as the per-item sub-subject.
        let item_ver = self.increment_version("#match_item");
        let item_subj = self.chunk.push_name(&s!("#match_item", int item_ver));

        // Walk items: each item ends at the next top-level Comma (or EOF).
        let mut item_idx: i64 = 0;
        let mut star_idx_seen: Option<i64> = None;
        while idx < total {
            // Detect star prefix.
            let is_star = matches!(saved.get(idx), Some(t) if t.kind == TokenType::Star);
            if is_star { idx += 1; }
            // Find end of this item (next Comma at depth 0 or EOF).
            let item_start = idx;
            let mut d: i32 = 0;
            while idx < total {
                let k = saved[idx].kind;
                if d == 0 && k == TokenType::Comma { break; }
                if matches!(k, TokenType::Lpar | TokenType::Lsqb | TokenType::Lbrace) { d += 1; }
                if matches!(k, TokenType::Rpar | TokenType::Rsqb | TokenType::Rbrace) { d -= 1; }
                idx += 1;
            }
            let item_end = idx;
            // Skip the comma if any.
            if idx < total && saved[idx].kind == TokenType::Comma { idx += 1; }

            // Compute the source-side index for this element. Without a star,
            // it's simply `item_idx`. With a star, prefix items use positive
            // indices, the star itself binds a slice, suffix items use
            // negative indices.
            if is_star {
                star_idx_seen = Some(item_idx);
                // Build subj[item_idx : len(subj) - (item_count - item_idx - 1)]
                let suffix = (item_count as i64) - item_idx - 1;
                self.chunk.emit(OpCode::LoadName, subj);
                let cs = self.chunk.push_const(super::types::Value::Int(item_idx));
                self.chunk.emit(OpCode::LoadConst, cs);
                self.chunk.emit(OpCode::LoadName, subj);
                self.chunk.emit(OpCode::CallLen, 0);
                let cend = self.chunk.push_const(super::types::Value::Int(suffix));
                self.chunk.emit(OpCode::LoadConst, cend);
                self.chunk.emit(OpCode::Sub, 0);
                self.chunk.emit(OpCode::LoadNone, 0);
                self.chunk.emit(OpCode::BuildSlice, 3);
                self.chunk.emit(OpCode::GetItem, 0);
                self.chunk.emit(OpCode::CallList, 0);
                self.chunk.emit(OpCode::StoreName, item_subj);
            } else {
                // Source index: positive before star, negative after.
                let physical_idx: i64 = if star_idx_seen.is_some() {
                    -((item_count as i64) - item_idx)
                } else {
                    item_idx
                };
                self.chunk.emit(OpCode::LoadName, subj);
                let cidx = self.chunk.push_const(super::types::Value::Int(physical_idx));
                self.chunk.emit(OpCode::LoadConst, cidx);
                self.chunk.emit(OpCode::GetItem, 0);
                self.chunk.emit(OpCode::StoreName, item_subj);
            }

            // Item pattern dispatch by token shape: wildcard, capture, or
            // literal-equality. Nested sequence/OR patterns aren't supported
            // inside another sequence — keeps the matcher linear and
            // sidesteps having to swap parser token streams. Use chained
            // ifs/match for those rare nested cases.
            let toks = &saved[item_start..item_end];
            if toks.is_empty() || (toks.len() == 1 && toks[0].kind == TokenType::Underscore) {
                // wildcard — drop the item_subj we just stored.
            } else if toks.len() == 1 && toks[0].kind == TokenType::Name {
                let name = self.source[toks[0].start..toks[0].end].to_string();
                if name != "_" {
                    self.chunk.emit(OpCode::LoadName, item_subj);
                    let ver = self.increment_version(&name);
                    let ni = self.push_ssa_name(&name, ver);
                    self.chunk.emit(OpCode::StoreName, ni);
                }
            } else {
                // Literal (or compound expression). Emit by replaying the
                // tokens through a small expr scanner. Supported: Int,
                // Float, Str, True, False, None, Minus+Number, parenthesised
                // expressions of the same. Anything else lands in the
                // generic `unsupported pattern` error path.
                let mut pos = 0;
                let mut neg = false;
                if pos < toks.len() && toks[pos].kind == TokenType::Minus { neg = true; pos += 1; }
                if pos < toks.len() {
                    let t = &toks[pos];
                    let raw = &self.source[t.start..t.end];
                    let val = match t.kind {
                        TokenType::Int => raw.replace('_', "").parse::<i64>().ok().map(super::types::Value::Int),
                        TokenType::Float => raw.replace('_', "").parse::<f64>().ok().map(super::types::Value::Float),
                        TokenType::String => Some(super::types::Value::Str(super::types::parse_string(raw))),
                        TokenType::True => Some(super::types::Value::Bool(true)),
                        TokenType::False => Some(super::types::Value::Bool(false)),
                        TokenType::None => Some(super::types::Value::None),
                        _ => None,
                    };
                    if let Some(mut v) = val {
                        if neg {
                            v = match v {
                                super::types::Value::Int(i) => super::types::Value::Int(-i),
                                super::types::Value::Float(f) => super::types::Value::Float(-f),
                                other => other,
                            };
                        }
                        self.chunk.emit(OpCode::LoadName, item_subj);
                        let ci = self.chunk.push_const(v);
                        self.chunk.emit(OpCode::LoadConst, ci);
                        self.chunk.emit(OpCode::Eq, 0);
                        self.chunk.emit(OpCode::JumpIfFalse, 0);
                        fail_jumps.push(self.chunk.instructions.len() - 1);
                    } else {
                        self.error_at(toks[0].start, toks.last().unwrap().end,
                            "unsupported sub-pattern in sequence (use literals, names, or _)");
                    }
                }
            }
            item_idx += 1;
        }
    }

    /* while: cond + body + back-edge; optional else when cond falsifies. */

    pub(super) fn while_stmt(&mut self) {
        self.advance();
        self.enter_block();

        let loop_start = self.chunk.instructions.len() as u16;
        self.loop_starts.push(loop_start);
        self.loop_breaks.push(vec![]);
        self.loop_kinds.push(false);

        self.expr();
        self.chunk.emit(OpCode::JumpIfFalse, 0);
        let jf = self.chunk.instructions.len() - 1;

        self.eat(TokenType::Colon);
        self.compile_block();

        self.chunk.emit(OpCode::Jump, loop_start);
        self.patch(jf);

        if self.eat_if(TokenType::Else) {
            self.eat(TokenType::Colon);
            self.compile_block();
        }

        self.loop_starts.pop();
        self.loop_kinds.pop();
        for pos in self.loop_breaks.pop().unwrap_or_default() {
            self.patch(pos);
        }

        self.commit_block();
    }

    /* for / async for, with optional tuple/star unpacking. */

    pub(super) fn for_stmt_inner(&mut self, is_async: bool) {
        self.advance();

        let parens = self.eat_if(TokenType::Lpar);
        let mut vars = Vec::new();
        let mut star_pos: Option<usize> = None;
        loop {
            if self.eat_if(TokenType::Star) { star_pos = Some(vars.len()); }
            vars.push(self.advance_text());
            if !self.eat_if(TokenType::Comma) { break; }
            if matches!(self.peek(), Some(TokenType::In | TokenType::Rpar)) { break; }
        }
        if parens {
            self.eat(TokenType::Rpar);
        }

        self.eat(TokenType::In);
        self.expr();
        self.chunk.emit(OpCode::GetIter, is_async as u16);

        self.enter_block();

        let loop_start = self.chunk.instructions.len() as u16;
        self.loop_starts.push(loop_start);
        self.loop_breaks.push(vec![]);
        self.loop_kinds.push(true);

        self.chunk.emit(OpCode::ForIter, 0);
        let fi = self.chunk.instructions.len() - 1;

        if vars.len() == 1 && star_pos.is_none() {
            self.store_name(vars[0].clone());
        } else {
            if let Some(sp) = star_pos {
                let before = sp as u16;
                let after = (vars.len() - sp - 1) as u16;
                self.chunk.emit(OpCode::UnpackEx, (before << 8) | after);
            } else {
                self.chunk.emit(OpCode::UnpackSequence, vars.len() as u16);
            }
            for var in &vars { self.store_name(var.clone()); }
        }

        self.eat(TokenType::Colon);
        self.compile_block();

        self.chunk.emit(OpCode::Jump, loop_start);
        self.patch(fi);

        if !is_async && self.eat_if(TokenType::Else) {
            self.eat(TokenType::Colon);
            self.compile_block();
        }

        self.loop_starts.pop();
        self.loop_kinds.pop();
        for pos in self.loop_breaks.pop().unwrap_or_default() {
            self.patch(pos);
        }

        self.commit_block();
    }

    /* try / except / else / finally with exception arm chaining. */

    pub(super) fn try_stmt(&mut self) {
        self.advance();
        self.eat(TokenType::Colon);

        self.chunk.emit(OpCode::SetupExcept, 0);
        let setup = self.chunk.instructions.len() - 1;

        self.enter_block();
        self.compile_block();

        self.chunk.emit(OpCode::PopExcept, 0);
        self.chunk.emit(OpCode::Jump, 0);
        let success_jump = self.chunk.instructions.len() - 1;

        self.mid_block();

        self.patch(setup);

        let mut end_jumps: Vec<usize> = Vec::new();
        let mut next_arm_jump: Option<usize> = None;
        let mut had_bare = false;

        while self.eat_if(TokenType::Except) {
            if let Some(j) = next_arm_jump.take() { self.patch(j); }
            if had_bare {
                self.error("default 'except:' must be last");
                break;
            }

            if matches!(self.peek(), Some(TokenType::Colon)) {
                had_bare = true;
                self.chunk.emit(OpCode::PopTop, 0);
            } else {
                self.chunk.emit(OpCode::Dup, 0);
                self.expr();
                let isinst_pos = self.last_end as u32;
                self.chunk.emit(OpCode::CallIsInstance, 0);
                self.chunk.record_call_pos(isinst_pos);
                self.chunk.emit(OpCode::JumpIfFalse, 0);
                next_arm_jump = Some(self.chunk.instructions.len() - 1);

                if self.eat_if(TokenType::As) {
                    let n = self.advance_text();
                    self.store_name(n);
                } else { self.chunk.emit(OpCode::PopTop, 0); }
            }
            self.eat(TokenType::Colon);
            self.compile_block();

            let more = matches!(
                self.peek(),
                Some(TokenType::Except | TokenType::Else | TokenType::Finally)
            );
            if !had_bare || more {
                self.chunk.emit(OpCode::Jump, 0);
                end_jumps.push(self.chunk.instructions.len() - 1);
            }
        }

        if let Some(j) = next_arm_jump {
            self.patch(j);
            self.chunk.emit(OpCode::Raise, 0);
        }

        self.patch(success_jump);
        for j in end_jumps {
            self.patch(j);
        }

        if self.eat_if(TokenType::Else) {
            self.eat(TokenType::Colon);
            self.compile_block();
        }

        if self.eat_if(TokenType::Finally) {
            self.eat(TokenType::Colon);
            self.compile_block();
        }

        self.commit_block();
    }

    /* with / async with: SetupWith per CM, ExitWith 1:1 on unwind. */

    pub(super) fn with_stmt_inner(&mut self, is_async: bool) {
        self.advance();
        let operand = is_async as u16;
        let mut cm_count: u16 = 0;
        loop {
            self.expr();
            self.chunk.emit(OpCode::SetupWith, operand);
            cm_count += 1;
            if self.eat_if(TokenType::As) {
                let name = self.advance_text();
                self.store_name(name);
            }
            if !self.eat_if(TokenType::Comma) { break; }
        }
        self.eat(TokenType::Colon);
        self.compile_block();
        // One ExitWith per SetupWith — paired 1:1 on unwind.
        for _ in 0..cm_count {
            self.chunk.emit(OpCode::ExitWith, operand);
        }
    }

    /* `import name` and `from <spec> import names` — both delegate to
       `imports.rs`, which calls the injected Resolver and either inlines code
       module functions or registers natives in the chunk's extern_table.
       Module resolution is compile-time only: no Import/ImportFrom opcodes
       reach the VM. See `imports.rs` for the resolution logic and
       `crate::modules::packages` for the public Resolver API. */

    pub(super) fn import_stmt(&mut self) {
        self.do_import_stmt();
    }

    pub(super) fn parse_from_stmt(&mut self) {
        self.do_from_stmt();
    }

}