pub mod tables;
pub use tables::utf8_char_len;

mod scan;

use scan::Scanner;
use alloc::vec::Vec;

const MAX_SOURCE_SIZE: usize = 10 * 1024 * 1024;

#[derive(Debug)]
pub struct Token {
    pub kind: TokenType,
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

/* Lex-time diagnostic. Carries a `&'static str` message — every lexer error
   is one of a small fixed set, so heap-allocating strings would be wasteful.
   Callers convert to a richer Diagnostic at the parser boundary. */
#[derive(Debug)]
pub struct LexError {
    pub start: usize,
    pub end: usize,
    pub msg: &'static str,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum TokenType {
    // Keywords
    False, None, True, And, As, Assert, Async, Await, Break, Class, Continue, Def, Del,
    Elif, Else, Except, Finally, For, From, Global, If, Import, In, Is, Lambda, Nonlocal,
    Not, Or, Pass, Raise, Return, Try, While, With, Yield,
    // Soft keywords
    Case, Match, Type, Underscore,
    // Operators (3-char)
    DoubleStarEqual, DoubleSlashEqual, LeftShiftEqual, RightShiftEqual,
    // Operators (2-char)
    NotEqual, PercentEqual, AmperEqual, DoubleStar, StarEqual, PlusEqual, MinEqual,
    Rarrow, Ellipsis, DoubleSlash, SlashEqual, ColonEqual, LeftShift, LessEqual,
    EqEqual, GreaterEqual, RightShift, AtEqual, CircumflexEqual, VbarEqual,
    // Operators (1-char)
    Exclamation, Percent, Amper, Star, Plus, Minus, Dot, Slash, Less, Equal, Greater,
    At, Circumflex, Vbar, Tilde, Comma, Colon, Semi,
    // Delimiters
    Lpar, Rpar, Lsqb, Rsqb, Lbrace, Rbrace,
    // Literals
    Name, Complex, Float, Int, String,
    // F-string
    FstringStart, FstringMiddle, FstringEnd,
    // Whitespace and structure
    Comment, Newline, Indent, Dedent, Nl, Endmarker,
}

/* Parser-ready token list with indentation handled and soft keywords
   (match/case/type) demoted to Name when followed by a token that can't
   begin a soft-keyword construct. Returns the token vector alongside any
   lex-time diagnostics so the caller can fold them into the parser's
   error stream. */
pub fn lex(source: &str) -> (Vec<Token>, Vec<LexError>) {
    let bytes = source.as_bytes();
    let len = source.len();
    let mut scanner = Scanner::new(bytes);

    if len > MAX_SOURCE_SIZE {
        scanner.errors.push(LexError {
            start: 0, end: 0,
            msg: "source file exceeds maximum size (10 MiB)",
        });
        return (
            alloc::vec![Token { kind: TokenType::Endmarker, line: 0, start: len, end: len }],
            scanner.errors,
        );
    }

    let mut raw: Vec<(TokenType, usize, usize, usize)> = Vec::new();
    while let Some(t) = scanner.next_token() {
        raw.push(t);
    }
    raw.push((TokenType::Endmarker, scanner.line, len, len));

    let mut tokens = Vec::with_capacity(raw.len());
    let mut ended = false;
    for i in 0..raw.len() {
        let (tok, line, start, end) = raw[i];
        if ended { break; }
        if tok == TokenType::Endmarker { ended = true; }

        let is_soft = matches!(tok, TokenType::Match | TokenType::Case | TokenType::Type);
        let next_demotes = match raw.get(i + 1) {
            Some(&(
                TokenType::Lpar
                | TokenType::Colon
                | TokenType::Equal
                | TokenType::Comma
                | TokenType::Rpar
                | TokenType::Rsqb
                | TokenType::Newline,
                _, _, _,
            )) => true,
            None => true,
            _ => false,
        };
        let kind = if is_soft && next_demotes { TokenType::Name } else { tok };
        tokens.push(Token { kind, line, start, end });
    }
    (tokens, scanner.errors)
}

impl TokenType {
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        tables::token_to_str(self)
    }
}