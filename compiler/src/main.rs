extern crate alloc;

use compiler_lib::modules::{lexer::lex, parser::{Parser, Diagnostic}, vm::{VM, Limits}};
use compiler_lib::modules::packages::{Resolver, Resolved, NativeBinding, load_wasm_bindings};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

modules:
  Imports resolve via three forms. Bare names (`from json import x`) lookup
  in `packages.json` next to the script:

    {
      \"imports\": {
        \"json\":  \"./vendor/json.wasm\",
        \"utils\": \"./lib/utils.py\",
        \"web\":   \"https://example.com/lib.wasm\"
      }
    }

  Spec resolution:
    ./ ../  /         local file relative to script (or absolute)
    http(s)://...     fetched at compile time (no cache yet)

  Recognized module formats:
    *.py    EdgePython source, inlined as functions
    *.wasm  Native WASM module, dispatched via the i64 ABI
            (build modules with the `edge-sdk` crate)
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

/* Default Resolver for the `edge` CLI.

   Resolves three import shapes:
     * Quoted relative path  `from "./utils.py" import x`
     * Quoted absolute path  `from "/srv/lib/x.wasm" import x`
     * Bare name             `from json import x`
       — looked up in packages.json's `imports` map, then re-resolved as a path.

   Path-form imports infer module type from extension:
     *.py    →  Resolved::Code (the file's source)
     *.wasm  →  Resolved::Native (load_wasm_bindings, via wasmtime)

   URL-form (`http://`, `https://`) imports are not supported by the CLI yet
   — those need a fetcher + cache layer that's planned but not implemented.

   `base_dir` is the directory of the entry script so relative paths resolve
   against the script's location, not the user's CWD. For `-c <code>`, it
   defaults to the CWD. */
struct CliResolver {
    base_dir: PathBuf,
    imports:  HashMap<String, String>,
}

impl Resolver for CliResolver {
    fn resolve(&mut self, spec: &str) -> Result<Resolved, String> {
        // 1. Alias lookup: bare names (no leading ./, /, http) hit packages.json.
        let resolved_spec = if spec.starts_with("./") || spec.starts_with("../")
            || spec.starts_with('/')
            || spec.starts_with("http://") || spec.starts_with("https://")
        {
            spec.to_string()
        } else {
            self.imports.get(spec).cloned().ok_or_else(||
                format!("module '{}' has no entry in packages.json's 'imports'", spec)
            )?
        };

        // 2. Read bytes — either fetched over HTTP(S) or read from local FS.
        //    URL fetches are blocking (matches the sync Resolver contract).
        //    No cache layer yet — every compile re-fetches; intended for
        //    development. For deploys, mirror to local files.
        let bytes: Vec<u8> = if resolved_spec.starts_with("http://")
            || resolved_spec.starts_with("https://")
        {
            fetch_url(&resolved_spec)
                .map_err(|e| format!("fetching module '{}': {}", spec, e))?
        } else {
            let path = if resolved_spec.starts_with('/') {
                PathBuf::from(&resolved_spec)
            } else {
                self.base_dir.join(&resolved_spec)
            };
            fs::read(&path).map_err(|e|
                format!("cannot read module '{}' at {}: {}", spec, path.display(), e))?
        };

        // 3. Dispatch on URL/path extension. Strip query strings for the
        //    extension check so `?v=1` doesn't break the match.
        let path_part = resolved_spec.split('?').next().unwrap_or(&resolved_spec);
        if path_part.ends_with(".py") {
            let src = String::from_utf8(bytes).map_err(|_|
                format!("module '{}' is not valid UTF-8", spec))?;
            Ok(Resolved::Code(src))
        } else if path_part.ends_with(".wasm") {
            let bindings: Vec<NativeBinding> = load_wasm_bindings(&bytes).map_err(|e|
                format!("loading WASM module '{}': {}", spec, e))?;
            Ok(Resolved::Native(bindings))
        } else {
            Err(format!(
                "module '{}' has unrecognized extension; expected .py or .wasm", spec))
        }
    }
}

/* Synchronous HTTP/HTTPS fetch via ureq. Returns the response body bytes,
   or an error string suitable for surfacing in a parser Diagnostic. Rejects
   non-2xx responses with the status code so the user can see what went wrong. */
fn fetch_url(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url).call()
        .map_err(|e| format!("{}", e))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    let mut body = response.into_body();
    body.read_to_vec().map_err(|e| format!("read body: {}", e))
}

/* Read packages.json from the script's directory if present. Missing file
   or parse errors yield an empty import map (the user can use full paths). */
fn read_packages_json(dir: &Path) -> HashMap<String, String> {
    #[derive(serde::Deserialize, Default)]
    struct Pkg {
        #[serde(default)]
        imports: HashMap<String, String>,
    }
    let path = dir.join("packages.json");
    let Ok(text) = fs::read_to_string(&path) else { return HashMap::new(); };
    serde_json::from_str::<Pkg>(&text).map(|p| p.imports).unwrap_or_default()
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

    // Module resolution base = script's directory (or CWD for `-c <code>`).
    let base_dir = if is_file {
        Path::new(path).parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
    } else {
        env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };
    let imports = read_packages_json(&base_dir);
    let resolver = Box::new(CliResolver { base_dir, imports });

    let (tokens, lex_errs) = lex(&src);
    let mut p = Parser::with_resolver(&src, tokens.into_iter(), resolver);
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
