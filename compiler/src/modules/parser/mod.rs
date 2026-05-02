pub(super) mod types;

mod stmt;
mod control;
mod expr;
mod literals;

pub use types::*;

use crate::s;
use crate::modules::lexer::{Token, TokenType};
use crate::modules::fx::FxHashMap as HashMap;

use alloc::{string::{String, ToString}, vec::Vec};
use core::iter::Peekable;

// Bracket diagnostic helpers — keep human-readable strings out of the hot path.
#[inline]
pub(super) const fn open_str(k: TokenType) -> &'static str {
    match k {
        TokenType::Lpar => "'('",
        TokenType::Lsqb => "'['",
        TokenType::Lbrace => "'{'",
        _ => "bracket",
    }
}
#[inline]
pub(super) const fn close_str(k: TokenType) -> &'static str {
    match k {
        TokenType::Rpar => "')'",
        TokenType::Rsqb => "']'",
        TokenType::Rbrace => "'}'",
        _ => "bracket",
    }
}
#[inline]
pub(super) const fn match_close_str(open: TokenType) -> &'static str {
    match open {
        TokenType::Lpar => "')'",
        TokenType::Lsqb => "']'",
        TokenType::Lbrace => "'}'",
        _ => "bracket",
    }
}

pub struct Parser<'src, I: Iterator<Item = Token>> {
    pub(super) source: &'src str,
    pub(super) tokens: Peekable<I>,
    pub(super) chunk: SSAChunk,
    pub(super) ssa_versions: HashMap<String, u32>,
    pub(super) join_stack: Vec<JoinNode>,
    pub(super) loop_starts: Vec<u16>,
    pub(super) last_line: usize,
    /* End offset of the last advance()'d token. Lets diagnostics anchor at
       end-of-previous-line when the next significant token lives on a later
       line — the common "missing `:` at end of header" case where peek()
       has already skipped a Newline so the offending position is no longer
       in the token stream. */
    pub(super) last_end: usize,
    pub(super) loop_breaks: Vec<Vec<usize>>,
    // Parallel to loop_starts/loop_breaks: true for `for` loops (which push
    // an iter on iter_stack), false for `while`. Lets `break` emit PopIter
    // only when escaping a for-loop, so nested for/while combinations work.
    pub(super) loop_kinds: Vec<bool>,
    pub(super) expr_depth: usize,
    pub(super) saw_newline: bool,
    /* Every `(`, `[`, `{` consumed-but-not-yet-closed, with the error count at
       the time it opened. Lets us anchor "X was never closed" diagnostics at
       the opener (instead of at EOF where the cascade lands), and drop the
       in-bracket cascade since those errors are downstream consequences. */
    pub(super) bracket_stack: Vec<(TokenType, usize, usize, usize)>,
    pub errors: Vec<Diagnostic>,
}

// SSA versioning: track and emit version-suffixed names.

impl<'src, I: Iterator<Item = Token>> Parser<'src, I> {
    pub(super) fn current_version(&self, name: &str) -> u32 {
        self.ssa_versions.get(name).copied().unwrap_or(0)
    }

    pub(super) fn ssa_name<'a>(name: &str, ver: u32, buf: &'a mut [u8; 128]) -> &'a str {
        let name_bytes = name.as_bytes();
        let cap = buf.len();
        let mut n = name_bytes.len().min(cap);
        buf[..n].copy_from_slice(&name_bytes[..n]);
        if n < cap {
            buf[n] = b'_';
            n += 1;
        }
        let mut tmp = itoa::Buffer::new();
        let s = tmp.format(ver).as_bytes();
        let take = s.len().min(cap - n);
        buf[n..n + take].copy_from_slice(&s[..take]);
        n += take;
        unsafe { core::str::from_utf8_unchecked(&buf[..n]) }
    }

    pub(super) fn increment_version(&mut self, name: &str) -> u32 {
        let cur = self.current_version(name);
        let new = cur + 1;
        self.ssa_versions.insert(name.to_string(), new);
        new
    }

    pub(super) fn push_ssa_name(&mut self, name: &str, ver: u32) -> u16 {
        let mut buf = [0u8; 128];
        self.chunk.push_name(Self::ssa_name(name, ver, &mut buf))
    }

    pub(super) fn emit_load_ssa(&mut self, name: String) {
        let i = self.push_ssa_name(&name, self.current_version(&name));
        self.chunk.emit(OpCode::LoadName, i);
    }

    pub(super) fn emit_const(&mut self, v: Value) {
        let i = self.chunk.push_const(v);
        self.chunk.emit(OpCode::LoadConst, i);
    }

    pub(super) fn store_name(&mut self, name: String) {
        let ver = self.increment_version(&name);
        let i = self.push_ssa_name(&name, ver);
        self.chunk.emit(OpCode::StoreName, i);
    }

    pub(super) fn with_fresh_chunk(&mut self, f: impl FnOnce(&mut Self)) -> SSAChunk {
        let saved_chunk = core::mem::take(&mut self.chunk);
        let saved_ver = self.ssa_versions.clone();
        f(self);
        let body = core::mem::take(&mut self.chunk);
        self.chunk = saved_chunk;
        self.ssa_versions = saved_ver;
        body
    }
}

// SSA join points: enter/mid/commit emit Phi at control-flow merges.

impl<'src, I: Iterator<Item = Token>> Parser<'src, I> {
    pub(super) fn enter_block(&mut self) {
        self.join_stack.push(JoinNode {
            backup: self.ssa_versions.clone(),
            then: None,
        });
    }

    pub(super) fn mid_block(&mut self) {
        let Some(j) = self.join_stack.last_mut() else { return };
        // Snapshot then-branch before overwriting with else baseline.
        j.then = Some(self.ssa_versions.clone());
        let mut restored = j.backup.clone();
        for (name, &v) in &self.ssa_versions {
            let e = restored.entry(name.clone()).or_insert(0);
            *e = (*e).max(v);
        }
        self.ssa_versions = restored;
    }

    pub(super) fn commit_block(&mut self) {
        let Some(j) = self.join_stack.pop() else { return };
        let post = self.ssa_versions.clone();

        let (a, b) = match j.then {
            Some(t) => (t, post),
            None => (post, j.backup.clone()),
        };

        let mut divergent: Vec<&String> = a
            .keys()
            .chain(b.keys())
            .filter(|name| a.get(*name).unwrap_or(&0) != b.get(*name).unwrap_or(&0))
            .collect();
        // sort: deterministic Phi order regardless of HashMap iteration.
        // dedup: chain() may yield duplicates if both branches define the same var.
        divergent.sort();
        divergent.dedup();

        for name in divergent {
            let va = *a.get(name).unwrap_or(&0);
            let vb = *b.get(name).unwrap_or(&0);
            let mut ba = [0u8; 128];
            let mut bb = [0u8; 128];
            let mut bx = [0u8; 128];
            let ia = self.chunk.push_name(Self::ssa_name(name, va, &mut ba));
            let ib = self.chunk.push_name(Self::ssa_name(name, vb, &mut bb));
            let v = self.increment_version(name);
            let ix = self.chunk.push_name(Self::ssa_name(name, v, &mut bx));

            self.chunk.phi_sources.push((ia, ib));
            self.chunk.emit(OpCode::Phi, ix);
        }
    }
}

// Token-stream utilities: advance/peek/eat + diagnostics.

impl<'src, I: Iterator<Item = Token>> Parser<'src, I> {
    /* Raw advance: same fallback to Endmarker, but bypasses bracket_stack
       tracking. Reserved for recovery-style consumers (drain_annotation,
       panic-mode sync, etc.) that have their own bracket-balance bookkeeping
       and shouldn't pollute the parser's structural stack. */
    pub(super) fn advance_raw(&mut self) -> Token {
        let tok = self.tokens.next().unwrap_or(Token {
            kind: TokenType::Endmarker,
            line: 0, start: 0, end: 0,
        });
        self.last_line = tok.line;
        if tok.end > 0 { self.last_end = tok.end; }
        tok
    }

    pub(super) fn advance(&mut self) -> Token {
        let tok = self.tokens.next().unwrap_or(Token {
            kind: TokenType::Endmarker,
            line: 0, start: 0, end: 0,
        });
        self.last_line = tok.line;
        if tok.end > 0 { self.last_end = tok.end; }
        match tok.kind {
            TokenType::Lpar | TokenType::Lsqb | TokenType::Lbrace => {
                self.bracket_stack.push((tok.kind, tok.start, tok.end, self.errors.len()));
            }
            TokenType::Rpar | TokenType::Rsqb | TokenType::Rbrace => {
                let want_open = match tok.kind {
                    TokenType::Rpar => TokenType::Lpar,
                    TokenType::Rsqb => TokenType::Lsqb,
                    _ => TokenType::Lbrace,
                };
                /* Walk the stack to find the matching opener. Anything sitting
                   above it is unclosed — surface those with their own
                   "X was never closed" anchored at the offender, then pop the
                   matching opener. If no match is in the stack at all, the
                   closer mismatches the innermost opener (or is orphan) and
                   we report accordingly without desyncing further. */
                if let Some(idx) = self.bracket_stack.iter().rposition(|&(k, _, _, _)| k == want_open) {
                    while self.bracket_stack.len() > idx + 1 {
                        let (k, st, en, ov) = self.bracket_stack.pop().unwrap();
                        self.errors.truncate(ov);
                        self.errors.push(Diagnostic {
                            start: st, end: en,
                            msg: s!(str open_str(k), " was never closed"),
                        });
                    }
                    self.bracket_stack.pop();
                } else if let Some(&(top_k, _, _, _)) = self.bracket_stack.last() {
                    self.errors.push(Diagnostic {
                        start: tok.start, end: tok.end,
                        msg: s!(str close_str(tok.kind), " does not match ",
                                str open_str(top_k), ", expected ",
                                str match_close_str(top_k)),
                    });
                    self.bracket_stack.pop();
                } else {
                    self.errors.push(Diagnostic {
                        start: tok.start, end: tok.end,
                        msg: s!("unexpected ", str close_str(tok.kind), ", no matching opener"),
                    });
                }
            }
            _ => {}
        }
        tok
    }

    /* Push a Diagnostic at peek's position WITHOUT panic-mode sync. For
       use when the caller wants to report a missing/wrong token but keep
       the surrounding parser flow intact (e.g. `class :` where a synthetic
       name lets `eat(Colon)` and the body parse normally). Anchors at
       end-of-prev-line when peek crossed a newline, mirroring `eat()`'s
       behavior. */
    pub(super) fn diag_at_peek(&mut self, msg: &str) {
        let n = self.source.len();
        let (start, end) = match self.tokens.peek() {
            Some(t) if t.line > self.last_line && self.last_end > 0 => (self.last_end, self.last_end),
            Some(t) => (t.start, t.end),
            None => (n, n),
        };
        self.errors.push(Diagnostic { start, end, msg: msg.to_string() });
    }

    /* Push a Diagnostic anchored at the next token's span (or at end-of-source
       if we ran past EOF) and panic-mode sync to the next statement boundary
       so we can keep reporting downstream errors. */
    pub(super) fn error(&mut self, msg: &str) {
        let n = self.source.len();
        let (start, end) = self.tokens.peek()
            .map(|t| (t.start, t.end))
            .unwrap_or((n, n));
        self.error_at(start, end, msg);
    }

    /* Same as `error` but anchored at the caller-provided span. Use when
       a parser has already consumed the offending token and wants the
       diagnostic to point at it (not at whatever comes next).

       Panic-mode sync: stop at statement boundaries (Newline/Dedent/Endmarker).
       When inside one or more open brackets, also stop at Comma or the
       matching close bracket of our current nesting level so we can resume
       parsing the next argument/element. We track nested openers/closers
       internally so a `)` inside `range(epochs)` doesn't terminate sync that
       started inside an outer unclosed `(`. */
    pub(super) fn error_at(&mut self, start: usize, end: usize, msg: &str) {
        self.errors.push(Diagnostic { start, end, msg: msg.to_string() });
        let in_brackets = !self.bracket_stack.is_empty();
        let mut depth: i32 = 0;
        loop {
            let kind = self.tokens.peek().map(|t| t.kind);
            match kind {
                None | Some(TokenType::Newline | TokenType::Dedent | TokenType::Endmarker) => break,
                Some(TokenType::Comma) if in_brackets && depth == 0 => break,
                Some(TokenType::Rpar | TokenType::Rsqb | TokenType::Rbrace) => {
                    if in_brackets && depth == 0 { break; }
                    depth -= 1;
                    self.tokens.next();
                }
                Some(TokenType::Lpar | TokenType::Lsqb | TokenType::Lbrace) => {
                    depth += 1;
                    self.tokens.next();
                }
                _ => { self.tokens.next(); }
            }
        }
    }

    pub(super) fn at_end(&mut self) -> bool { self.peek().is_none() }

    pub(super) fn lexeme(&self, t: &Token) -> &'src str { &self.source[t.start..t.end] }

    /* Consume the next token and return its source text as a String. */
    pub(super) fn advance_text(&mut self) -> String {
        let t = self.advance();
        self.lexeme(&t).to_string()
    }

    /* Surface user-visible tokens. Skips Newline (latching `saw_newline`),
       Nl, Comment. Treats Endmarker as None so `at_end()` and "loop while
       not closer/None" patterns terminate cleanly without explicit Endmarker
       checks at every site. The raw iterator (`self.tokens.peek()`) still
       sees Endmarker for diagnostic anchoring in `error()` / `eat()`. */
    pub(super) fn peek(&mut self) -> Option<TokenType> {
        loop {
            match self.tokens.peek().map(|t| t.kind) {
                Some(TokenType::Newline) => {
                    self.saw_newline = true;
                    self.tokens.next();
                }
                Some(TokenType::Nl | TokenType::Comment) => { self.tokens.next(); }
                Some(TokenType::Endmarker) | None => return None,
                Some(k) => return Some(k),
            }
        }
    }

    pub(super) fn patch(&mut self, pos: usize) {
        self.chunk.instructions[pos].operand = self.chunk.instructions.len() as u16;
    }

    /* Consume `kind` or push a diagnostic with a friendly description of
       what was actually found (lexeme for normal tokens, kind label for
       synthetic Endmarker / structural tokens, and "EOF" past end).

       Special path for close brackets at EOF: anchor at the matching opener
       on `bracket_stack` and drop any cascade errors that piled up inside
       the unclosed bracket — those are downstream consequences of the same
       unclosed-bracket bug and only confuse the user. */
    pub(super) fn eat(&mut self, kind: TokenType) {
        if matches!(self.peek(), Some(k) if k == kind) {
            self.advance();
            return;
        }
        if matches!(kind, TokenType::Rpar | TokenType::Rsqb | TokenType::Rbrace) {
            let peeked = self.tokens.peek().map(|t| t.kind);
            // Different closer in stream — silently bail; advance() will fire a
            // "X does not match Y" / "Y was never closed" when the wrong closer
            // is consumed up the call chain. Avoids a redundant generic error.
            if matches!(peeked, Some(TokenType::Rpar | TokenType::Rsqb | TokenType::Rbrace))
                && peeked != Some(kind)
            {
                return;
            }
            // Whenever the matching opener sits at the top of the stack and we
            // can't produce its closer, anchor at the opener — this catches
            // both EOF and mid-stream cases (e.g. a `[` followed by a newline
            // is "lost" because Nl is suppressed inside brackets, so the
            // generic fallback would point at the next-line token instead).
            let want_open = match kind {
                TokenType::Rpar => TokenType::Lpar,
                TokenType::Rsqb => TokenType::Lsqb,
                _ => TokenType::Lbrace,
            };
            if let Some(&(opener_kind, start, end, errors_at_open)) = self.bracket_stack.last()
                && opener_kind == want_open
            {
                self.errors.truncate(errors_at_open);
                self.errors.push(Diagnostic {
                    start, end,
                    msg: s!(str open_str(opener_kind), " was never closed"),
                });
                self.bracket_stack.pop();
                return;
            }
        }
        let label: alloc::string::String = match self.tokens.peek() {
            Some(t) if t.kind == TokenType::Endmarker => "EOF".to_string(),
            Some(t) if t.kind == TokenType::Newline || t.kind == TokenType::Nl => "newline".to_string(),
            Some(t) if t.kind == TokenType::Indent => "indent".to_string(),
            Some(t) if t.kind == TokenType::Dedent => "dedent".to_string(),
            Some(t) if t.start == t.end => t.kind.as_str().to_string(),
            Some(t) => {
                let mut s = alloc::string::String::with_capacity(t.end - t.start + 2);
                s.push('\''); s.push_str(&self.source[t.start..t.end]); s.push('\''); s
            }
            None => "EOF".to_string(),
        };
        let msg = s!("expected ", str kind.as_str(), ", got ", str &label);
        // Anchor at end-of-previous-line when the next significant token is
        // on a later line than the last consumed one. peek() has already
        // swallowed the Newline, so the offending position no longer exists
        // in the token stream — without this, the caret lands on the next
        // line's first token (e.g. `Indent`) which is misleading for the
        // user. Most common case: missing `:` at the end of an `if/for/def`
        // header.
        if let Some(t) = self.tokens.peek()
            && t.line > self.last_line
            && self.last_end > 0
        {
            let p = self.last_end;
            self.error_at(p, p, &msg);
        } else {
            self.error(&msg);
        }
    }

    pub(super) fn eat_if(&mut self, kind: TokenType) -> bool {
        if matches!(self.peek(), Some(k) if k == kind) {
            self.advance();
            true
        } else {
            false
        }
    }
}

// Constructor and entry point.

impl<'src, I: Iterator<Item = Token>> Parser<'src, I> {
    pub fn new(source: &'src str, iter: I) -> Self {
        Self {
            source,
            tokens: iter.peekable(),
            chunk: SSAChunk::default(),
            ssa_versions: HashMap::default(),
            join_stack: Vec::new(),
            loop_starts: Vec::new(),
            loop_breaks: Vec::new(),
            loop_kinds: Vec::new(),
            saw_newline: false,
            expr_depth: 0,
            last_line: 0,
            last_end: 0,
            bracket_stack: Vec::new(),
            errors: Vec::new(),
        }
    }

    pub fn parse(mut self) -> (SSAChunk, Vec<Diagnostic>) {
        while !self.at_end() {
            while self.eat_if(TokenType::Semi) {}
            if self.at_end() { break; }

            let produced_value = self.stmt();
            // Always pop expression-statement results: the implicit ReturnValue
            // at chunk end returns Val::none() if the stack is empty.
            if produced_value { self.chunk.emit(OpCode::PopTop, 0); }
        }

        if self.chunk.overflow {
            let n = self.source.len();
            self.errors.push(Diagnostic {
                start: n, end: n,
                msg: "program too large: exceeded maximum instruction limit".to_string()
            });
        }

        if !self.errors.is_empty() {
            // Wipe ALL bytecode side-state so finalize_prev_slots doesn't
            // index `canonical` (built from `names`) with stale phi sources.
            self.chunk.instructions.clear();
            self.chunk.constants.clear();
            self.chunk.names.clear();
            self.chunk.phi_sources.clear();
            self.chunk.phi_map.clear();
            self.chunk.functions.clear();
            self.chunk.classes.clear();
            self.chunk.name_index.clear();
            self.chunk.nonlocals.clear();
            self.chunk.annotations.clear();
            self.loop_kinds.clear();
        }

        self.chunk.emit(OpCode::ReturnValue, 0);
        self.chunk.finalize_prev_slots();
        (self.chunk, self.errors)
    }
}