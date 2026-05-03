extern crate alloc;

use compiler_lib::modules::{lexer::lex, parser::{Parser, Diagnostic}, vm::{VM, Limits}};
use std::{env, fs, process::exit};
use compiler_lib::s;

const HELP: &str = "
usage: edge [options] <file>
       edge -c <code>

options:
  -c <code>    run inline code
  -d           debug output (verbosity level 1)
  -dd          debug output (verbosity level 2)
  -q           suppress info logs
  --sandbox    enable limits
  -h           show this help
";

#[inline]
fn eprint_msg(msg: &str) {
    use std::io::Write;
    let mut e = std::io::stderr().lock();
    let _ = e.write_all(msg.as_bytes());
    let _ = e.write_all(b"\n");
}

// Diagnostics go to stderr so `edge file.py > out.txt` captures only program output.
#[inline]
fn print_msg(level: &str, msg: &str) {
    use std::io::Write;
    let mut e = std::io::stderr().lock();
    let _ = e.write_all(b"[");
    let _ = e.write_all(level.as_bytes());
    let _ = e.write_all(b"] ");
    let _ = e.write_all(msg.as_bytes());
    let _ = e.write_all(b"\n");
}

// VM print() sink: streams each line to stdout as it executes (mirrors wasm.rs's js_print).
fn stream_print(s: &str) {
    use std::io::Write;
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(s.as_bytes());
    let _ = o.write_all(b"\n");
}

fn parse_args() -> (String, usize, bool, bool) {
    let args: Vec<_> = env::args().skip(1).collect();
    // GNU convention: explicit -h is requested output (stdout, exit 0); missing args is a usage error (stderr, exit 1).
    if args.iter().any(|a| a == "-h") {
        use std::io::Write;
        let _ = std::io::stdout().lock().write_all(HELP.as_bytes());
        exit(0);
    }
    if args.is_empty() {
        eprint_msg("usage: edge [options] <file>  (try `edge -h`)");
        exit(1);
    }
    let q = args.iter().any(|a| a == "-q");
    let sandbox = args.iter().any(|a| a == "--sandbox");
    let v = args.iter().filter(|&a| a == "-dd").count() * 2 + args.iter().filter(|&a| a == "-d").count();

    if let Some(pos) = args.iter().position(|a| a == "-c") {
        let code = args.get(pos + 1).cloned().unwrap_or_default();
        return (code, v, q, sandbox);
    }
    let p = args.iter().find(|&a| !a.starts_with('-')).cloned().unwrap_or_else(|| {
        eprint_msg("abort: no input file specified");
        exit(1);
    });
    (p, v, q, sandbox)
}

fn run(path: &str, sandbox: bool, verbosity: usize, quiet: bool) -> Result<(), String> {
    let is_file = path.ends_with(".py");
    let src = if is_file {
        fs::read_to_string(path).map_err(|_| s!("io: cannot access '", str path, "'"))?
    } else {
        path.to_string()
    };
    let diag_path = if is_file { Some(path) } else { None };

    let (tokens, lex_errs) = lex(&src);
    let mut p = Parser::new(&src, tokens.into_iter());
    for e in lex_errs {
        p.errors.push(Diagnostic { start: e.start, end: e.end, msg: e.msg.to_string() });
    }
    let (mut chunk, errs) = p.parse();
    if !errs.is_empty() {
        for e in &errs {
            eprint_msg(&e.render(&src, diag_path));
        }
        exit(1);
    }
    compiler_lib::modules::vm::optimizer::constant_fold(&mut chunk);


    if !quiet {
        print_msg("info", &s!(
            "emit: snapshot created [ops=", int chunk.instructions.len(), " consts=", int chunk.constants.len(), "]"));
    }

    let limits = if sandbox { Limits::sandbox() } else { Limits::none() };
    let mut vm = VM::with_limits(&chunk, limits);
    vm.print_hook = Some(stream_print);
    let exec_result = vm.run();

    if let Err(e) = exec_result {
        return Err(e.render_at(&src, vm.error_pos(), diag_path));
    }

    if verbosity >= 1 {
        let (sp, tot) = vm.cache_stats();
        print_msg("debug", &s!(
            "vm: specialization_ratio=", int sp, "/", int tot, " [heap_footprint=", int vm.heap_usage(), "b]"));
    }

    Ok(())
}

fn main() {
    let (p, v, q, sandbox) = parse_args();
    if let Err(e) = run(&p, sandbox, v, q) {
        eprint_msg(&e);
        exit(1);
    }
}