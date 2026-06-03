use afl::fuzz;

use compiler::modules::lexer::lex;
use compiler::modules::parser::Parser;
use compiler::modules::vm::{Limits, VM};

fn main() {
    fuzz!(|data: &[u8]| {
        // Source is text; reject non-UTF-8 rather than counting it as coverage.
        let Ok(src) = core::str::from_utf8(data) else { return };

        let (tokens, _lex_errs) = lex(src);
        let (chunk, parse_errs) = Parser::new(src, tokens.into_iter()).parse();

        // Only valid programs reach the VM; the chunk is unreliable after a parse error.
        if !parse_errs.is_empty() {
            return;
        }

        // Bounded budget turns runaway loops and allocations into VmErr, not hangs.
        let _ = VM::with_limits(&chunk, Limits::sandbox()).run();
    });
}
