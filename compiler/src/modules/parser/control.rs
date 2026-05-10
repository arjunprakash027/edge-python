use crate::s;

use super::Parser;
use super::types::OpCode;

use crate::modules::lexer::{Token, TokenType};

use alloc::{vec, vec::Vec, string::ToString};

impl<'src, I: Iterator<Item = Token>> Parser<'src, I> {

    /* if/elif/else compiler; emits JumpIfFalse/Jump and patches branch join targets */

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

    /* match/case: literals, captures, wildcards, OR, guards, sequences; emits subject-load + pattern + guard + Jump-end. */
    pub(super) fn match_stmt(&mut self) {
        self.advance();
        self.expr();

        let ver = self.increment_version(super::SSA_TMP_MATCH);
        let subj = self.chunk.push_name(&s!(str super::SSA_TMP_MATCH, int ver));
        self.chunk.emit(OpCode::StoreName, subj);

        self.eat(TokenType::Colon);
        self.eat_if(TokenType::Indent);

        let mut end_jumps = Vec::new();

        while matches!(self.peek(), Some(TokenType::Case)) {
            self.advance();

            let mut fail_jumps: Vec<usize> = Vec::new();
            self.parse_pattern(subj, &mut fail_jumps);

            // Guard fail joins pattern fails; both land at the next case.
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

    /* Emits bytecode for one pattern; appends case-fail jumps to `fail_jumps`; reloads subject from subj. */
    pub(super) fn parse_pattern(&mut self, subj: u16, fail_jumps: &mut Vec<usize>) {
        // OR pattern: each alt gets a success-jump landing past all alts.
        let alt_start = self.chunk.instructions.len();
        let _ = alt_start; // unused; alts link via `fail_jumps`.

        let mut alts: Vec<Vec<usize>> = Vec::new();
        let mut succ_jumps: Vec<usize> = Vec::new();

        loop {
            let mut this_alt_fails: Vec<usize> = Vec::new();
            self.parse_simple_pattern(subj, &mut this_alt_fails);
            // On match: jump past remaining alts.
            self.chunk.emit(OpCode::Jump, 0);
            succ_jumps.push(self.chunk.instructions.len() - 1);
            // On mismatch: redirect fails to next alt; only last alt propagates to case-fail.
            alts.push(this_alt_fails);
            if !matches!(self.peek(), Some(TokenType::Vbar)) {
                break;
            }
            // Previous alt's fails land at next alt entry.
            let here = self.chunk.instructions.len();
            for j in alts.last_mut().unwrap().drain(..) {
                let target = here as u16;
                self.chunk.instructions[j].operand = target;
            }
            self.advance(); // consume `|`
        }

        // Last alt's fails become the case-fail exits.
        if let Some(last) = alts.last_mut() {
            fail_jumps.append(last);
        }

        // All success jumps land here, past the OR.
        for j in succ_jumps { self.patch(j); }
    }

    /* Dispatches single pattern alternative by token: wildcard, capture (StoreName), or literal equality. */
    fn parse_simple_pattern(&mut self, subj: u16, fail_jumps: &mut Vec<usize>) {
        match self.peek() {
            // Wildcard: always succeeds, no binding.
            Some(TokenType::Underscore) => { self.advance(); }
            // Sequence pattern.
            Some(TokenType::Lsqb) => { self.parse_sequence_pattern(subj, fail_jumps); }
            // Capture: bind subject to name, always succeeds.
            Some(TokenType::Name) => {
                let t = self.advance();
                let name = self.lexeme(&t).to_string();
                self.chunk.emit(OpCode::LoadName, subj);
                let ver = self.increment_version(&name);
                let i = self.push_ssa_name(&name, ver);
                self.chunk.emit(OpCode::StoreName, i);
            }
            // Literal/expr: equality-test against subject; precedence > bitwise-or keeps `1|2|3` as OR pattern.
            _ => {
                self.chunk.emit(OpCode::LoadName, subj);
                self.expr_bp(11);
                self.chunk.emit(OpCode::Eq, 0);
                self.chunk.emit(OpCode::JumpIfFalse, 0);
                fail_jumps.push(self.chunk.instructions.len() - 1);
            }
        }
    }

    /* Sequence pattern: length-checks subject, indexes items into fresh slots; star captures middle slice. */
    fn parse_sequence_pattern(&mut self, subj: u16, fail_jumps: &mut Vec<usize>) {
        self.advance(); // consume `[`

        // Two-pass: scan counts items/star; second pass emits bytecode.
        let _ = Vec::<bool>::new(); // remove `item_positions` entirely, it's dead

        // Pass 1: buffer tokens to count items and locate the star.
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

        // Length check: exact without star, >= (count-1) with star.
        let len_min = if star_count > 0 { item_count - 1 } else { item_count };
        self.chunk.emit(OpCode::LoadName, subj);
        self.chunk.emit(OpCode::CallLen, 0);
        let ci = self.chunk.push_const(super::types::Value::Int(len_min as i64));
        self.chunk.emit(OpCode::LoadConst, ci);
        let cmp = if star_count > 0 { OpCode::GtEq } else { OpCode::Eq };
        self.chunk.emit(cmp, 0);
        self.chunk.emit(OpCode::JumpIfFalse, 0);
        fail_jumps.push(self.chunk.instructions.len() - 1);

        // Pass 2: walk buffered tokens, emitting per-item bytecode.
        let saved: Vec<crate::modules::lexer::Token> = buffered;
        let mut idx = 0usize;
        let total = saved.len();

        // Fresh slot to use as the per-item sub-subject.
        let item_ver = self.increment_version(super::SSA_TMP_MATCH_ITEM);
        let item_subj = self.chunk.push_name(&s!(str super::SSA_TMP_MATCH_ITEM, int item_ver));

        // Walk items; split on top-level commas.
        let mut item_idx: i64 = 0;
        let mut star_idx_seen: Option<i64> = None;
        while idx < total {
            // Check for star prefix.
            let is_star = matches!(saved.get(idx), Some(t) if t.kind == TokenType::Star);
            if is_star { idx += 1; }
            // Find item end: next depth-0 comma or EOF.
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
            // Consume trailing comma.
            if idx < total && saved[idx].kind == TokenType::Comma { idx += 1; }

            // Index: positive before star, star gets slice, negative after.
            if is_star {
                star_idx_seen = Some(item_idx);
                // Slice: subj[item_idx : len-suffix]
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
                // Negative index for items after the star.
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

            // Dispatch: wildcard, capture, or literal. No nested sequences; use if/match instead.
            let toks = &saved[item_start..item_end];
            if toks.is_empty() || (toks.len() == 1 && toks[0].kind == TokenType::Underscore) {
                // Wildcard: discard stored `item_subj`.
            } else if toks.len() == 1 && toks[0].kind == TokenType::Name {
                let name = self.source[toks[0].start..toks[0].end].to_string();
                if name != "_" {
                    self.chunk.emit(OpCode::LoadName, item_subj);
                    let ver = self.increment_version(&name);
                    let ni = self.push_ssa_name(&name, ver);
                    self.chunk.emit(OpCode::StoreName, ni);
                }
            } else {
                // Literal: replay tokens; supports Int/Float/Str/Bool/None/negation; else error.
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
        // Paired ExitWith for each SetupWith.
        for _ in 0..cm_count {
            self.chunk.emit(OpCode::ExitWith, operand);
        }
    }

    /* Delegates to imports.rs; compile-time only — no import opcodes reach the VM. */

    pub(super) fn import_stmt(&mut self) {
        self.do_import_stmt();
    }

    pub(super) fn parse_from_stmt(&mut self) {
        self.do_from_stmt();
    }

}
