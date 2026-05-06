extern crate alloc;

use compiler_lib::modules::{lexer::lex, parser::{Parser, Diagnostic}, vm::{VM, Limits}};
use compiler_lib::modules::packages::{Resolver, Resolved, NativeBinding, load_wasm_bindings};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
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
     * Quoted relative path  `from "./utils.py" import x`  (against `current_dir`)
     * Quoted absolute path  `from "/srv/lib/x.wasm" import x`
     * Bare name             `from json import x`
       — looked up in the root packages.json's `imports` map, then re-resolved.

   Path-form imports infer module type from extension:
     *.py    →  Resolved::Code (the file's source)
     *.wasm  →  Resolved::Native (load_wasm_bindings, via wasmtime)

   URL-form (`http://`, `https://`) imports are fetched synchronously via
   `ureq` + `rustls`. There is no on-disk cache yet; each compile re-fetches.

   Entry-point semantics: `root_dir` is the directory containing the entry
   script's packages.json (or the script's own directory if no packages.json
   exists). It is fixed for the whole compilation. `current_dir` is the
   directory of the file currently being resolved — updated when the parser
   descends into transitive imports so a deeper file's `./helper.py`
   resolves against ITS directory. Any packages.json sitting next to a
   sub-module is silently ignored: only the entry's import map is read,
   matching how Cargo.toml at the workspace root drives all dependencies. */
struct CliResolverState {
    root_dir: PathBuf,
    imports: HashMap<String, String>,
    /* canonical spec → already-resolved module. Hits skip both the fetch
       and the WASM loader on diamond imports (main → a → c, main → b → c
       loads c once). Canonical = absolute path or full URL. */
    cache: HashMap<String, Resolved>,
    /* canonical specs whose parse is in flight. Adding via `child()`,
       removing via the child resolver's `Drop`. Lets us catch a → b → a
       cycles that would otherwise infinite-loop the splicer. */
    in_flight: HashSet<String>,
}

struct CliResolver {
    state: Rc<RefCell<CliResolverState>>,
    current_dir: PathBuf,
    /* Set when this resolver was minted by `child()` for an in-flight
       import: on Drop, the canonical spec is removed from `in_flight`. The
       root resolver leaves this `None` (it's never the "current" parse
       target — main script isn't routed through resolve()). */
    in_flight_marker: Option<String>,
}

impl Drop for CliResolver {
    fn drop(&mut self) {
        if let Some(canon) = self.in_flight_marker.take() {
            self.state.borrow_mut().in_flight.remove(&canon);
        }
    }
}

impl CliResolver {
    fn new(root_dir: PathBuf, imports: HashMap<String, String>) -> Self {
        let current_dir = root_dir.clone();
        Self {
            state: Rc::new(RefCell::new(CliResolverState {
                root_dir,
                imports,
                cache: HashMap::new(),
                in_flight: HashSet::new(),
            })),
            current_dir,
            in_flight_marker: None,
        }
    }

    /* Canonicalize a user-facing spec to a stable key for cache / cycle
       detection. URLs are kept verbatim. Paths become absolute, joining
       relative paths against `current_dir` and bare names through the
       root's `imports` map (which yields a path relative to `root_dir`). */
    fn canonicalize(&self, spec: &str) -> Result<String, String> {
        if spec.starts_with("http://") || spec.starts_with("https://") {
            return Ok(spec.to_string());
        }
        let st = self.state.borrow();
        if spec.starts_with("./") || spec.starts_with("../") {
            let joined = self.current_dir.join(spec);
            return Ok(absolute(&joined).to_string_lossy().into_owned());
        }
        if spec.starts_with('/') {
            return Ok(spec.to_string());
        }
        // Bare name: look up in the root's import map. The mapped target may
        // be a URL, an absolute path, or a path relative to root_dir.
        let target = st.imports.get(spec).cloned().ok_or_else(|| format!(
            "module '{}' has no entry in packages.json's 'imports'", spec))?;
        if target.starts_with("http://") || target.starts_with("https://")
            || target.starts_with('/') {
            Ok(target)
        } else {
            let joined = st.root_dir.join(&target);
            Ok(absolute(&joined).to_string_lossy().into_owned())
        }
    }

    fn fetch_and_dispatch(&self, canonical: &str, spec: &str) -> Result<Resolved, String> {
        let bytes: Vec<u8> = if canonical.starts_with("http://") || canonical.starts_with("https://") {
            fetch_url(canonical)
                .map_err(|e| format!("fetching module '{}': {}", spec, e))?
        } else {
            fs::read(canonical).map_err(|e|
                format!("cannot read module '{}' at {}: {}", spec, canonical, e))?
        };

        let path_part = canonical.split('?').next().unwrap_or(canonical);
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

impl Resolver for CliResolver {
    fn resolve(&mut self, spec: &str) -> Result<Resolved, String> {
        let canonical = self.canonicalize(spec)?;

        if self.state.borrow().in_flight.contains(&canonical) {
            return Err(format!("circular import: '{}'", spec));
        }
        if let Some(r) = self.state.borrow().cache.get(&canonical) {
            return Ok(r.clone());
        }

        let resolved = self.fetch_and_dispatch(&canonical, spec)?;
        self.state.borrow_mut().cache.insert(canonical, resolved.clone());
        Ok(resolved)
    }

    fn child(&self, spec: &str) -> Box<dyn Resolver> {
        // Best-effort canonicalization. If it fails, the parent's `resolve`
        // already surfaced the diagnostic; we still need to hand back a
        // usable resolver so the splicer's parse step can run cleanly.
        let canonical = self.canonicalize(spec).unwrap_or_default();
        if !canonical.is_empty() {
            self.state.borrow_mut().in_flight.insert(canonical.clone());
        }
        let new_dir = if canonical.starts_with("http://") || canonical.starts_with("https://") {
            // URLs have no FS directory: keep current_dir so any local
            // ./helpers.py inside the fetched module still resolves against
            // the importer's directory (best we can do without a virtual FS).
            self.current_dir.clone()
        } else if canonical.is_empty() {
            self.current_dir.clone()
        } else {
            Path::new(&canonical).parent().map(PathBuf::from).unwrap_or_else(||
                self.current_dir.clone())
        };
        Box::new(CliResolver {
            state: Rc::clone(&self.state),
            current_dir: new_dir,
            in_flight_marker: if canonical.is_empty() { None } else { Some(canonical) },
        })
    }
}

/* `std::path::absolute` is unstable on stable Rust; this is a minimal
   replacement that joins with CWD when needed and resolves `.`/`..` segments
   without touching the filesystem. Used only for canonicalization keys
   (cache + cycle detection); the eventual file read uses the same string. */
fn absolute(p: &Path) -> PathBuf {
    let base = if p.is_absolute() {
        PathBuf::new()
    } else {
        env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };
    let mut out = base;
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => { out.pop(); }
            other => out.push(other.as_os_str()),
        }
    }
    out
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

    // Root directory for resolution = entry script's directory (or CWD for
    // `-c <code>`). This is where the only packages.json is read; sub-modules'
    // packages.json files are silently ignored, mirroring how Cargo.toml at
    // the workspace root drives all dependencies.
    let root_dir = if is_file {
        Path::new(path).parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
    } else {
        env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };
    let imports = read_packages_json(&root_dir);
    let resolver = Box::new(CliResolver::new(root_dir, imports));

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
