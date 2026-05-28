/*
Interactive REPL: a persistent engine session driven by rustyline; multi-line blocks via the `:` heuristic.
*/

use anyhow::Result;
use rustyline::{error::ReadlineError, DefaultEditor};
use std::path::Path;

use crate::engine::Session;
use crate::pkg::Manifest;

const PROMPT: &str = ">>> ";
const CONT: &str = "... ";

pub fn run(manifest_path: &Path) -> Result<()> {
    let manifest = Manifest::load(manifest_path)?;
    let mut session = Session::open(&manifest)?;
    println!("Edge Python {}  ·  type .exit to quit", env!("CARGO_PKG_VERSION"));

    let mut rl = DefaultEditor::new()?;
    loop {
        let first = match rl.readline(PROMPT) {
            Ok(s) => s,
            Err(ReadlineError::Interrupted) => continue, // Ctrl-C cancels the current input
            Err(ReadlineError::Eof) => break,            // Ctrl-D exits
            Err(e) => { eprintln!("repl error: {e}"); break; }
        };
        let _ = rl.add_history_entry(first.as_str());
        let block = match read_block(&mut rl, first)? {
            Some(b) => b,
            None => continue, // Ctrl-C inside a continuation discards the block
        };

        let trimmed = block.trim();
        if trimmed.is_empty() { continue; }
        match trimmed {
            ".exit" => break,
            ".reset" => {
                // Drop and reopen the session to wipe runtime state.
                session = Session::open(&manifest)?;
                continue;
            }
            _ => {}
        }

        let outcome = session.eval(&block, |line| println!("{line}"))?;
        if let Some(err) = outcome.err {
            crate::ui::traceback(&err);
        }
    }
    Ok(())
}

/// Collect a multi-line block when the first line ends with `:`; an empty line closes it.
fn read_block(rl: &mut DefaultEditor, first: String) -> Result<Option<String>> {
    if !first.trim_end().ends_with(':') {
        return Ok(Some(first));
    }
    let mut block = first;
    loop {
        match rl.readline(CONT) {
            Ok(line) if line.is_empty() => return Ok(Some(block)),
            Ok(line) => {
                let _ = rl.add_history_entry(line.as_str());
                block.push('\n');
                block.push_str(&line);
            }
            Err(ReadlineError::Interrupted) => return Ok(None),
            Err(ReadlineError::Eof) => return Ok(Some(block)),
            Err(e) => { eprintln!("repl error: {e}"); return Ok(None); }
        }
    }
}
