/* Minimal subset JSON parser for `packages.json`, plus path helpers used by
   any host that wants nested-manifest support.

   Why a hand-rolled parser instead of pulling serde_json: the manifest format
   is ten lines of JSON; serde_json would balloon the wasm artifact and add
   a heavy build-time dependency. ~110 LoC of obvious code keeps the WASM
   compiler self-contained.

   The walk-up helpers operate on opaque "spec strings" — they treat URLs and
   filesystem paths uniformly, since both share `/`-delimited segments. URL
   walks stop at `scheme://host/`; local-path walks stop at empty. */

use alloc::string::{String, ToString};

use crate::s;
use crate::modules::fx::FxHashMap;

/* A parsed `packages.json`.

   `imports` maps bare names (the left side of `from <name> import ...`) to
   target specs. Targets are resolved relative to the manifest's directory by
   the resolver, not here, so this struct stays a passive data shape.

   `extends` (if present) names a directory containing another manifest whose
   imports are inherited when a name isn't found locally. Without `extends`,
   a manifest is hermetic — a missing alias is an error. */
#[derive(Clone)]
pub struct Manifest {
    pub imports: FxHashMap<String, String>,
    pub extends: Option<String>,
}

/* Parse the manifest subset:
     {
       "imports": { "name": "target", ... },
       "extends": "../shared"
     }
   Both fields optional. Unknown top-level keys are accepted (and their
   values skipped, recursively) so future additions don't break old
   compilers. Numbers, arrays, and booleans are rejected: the format is
   string-only by contract. */
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    let src = core::str::from_utf8(bytes)
        .map_err(|_| s!("packages.json is not valid UTF-8"))?;
    let mut p = Reader { src: src.as_bytes(), pos: 0 };
    let mut m = Manifest { imports: FxHashMap::default(), extends: None };

    p.skip_ws();
    p.expect(b'{', "packages.json must be a JSON object")?;
    p.skip_ws();
    if p.peek() == Some(b'}') { return Ok(m); }

    loop {
        p.skip_ws();
        let key = p.read_string()?;
        p.skip_ws();
        p.expect(b':', "expected ':' after key in packages.json")?;
        p.skip_ws();
        match key.as_str() {
            "imports" => p.read_imports_into(&mut m.imports)?,
            "extends" => m.extends = Some(p.read_string()?),
            _         => p.skip_value()?,
        }
        p.skip_ws();
        match p.peek() {
            Some(b',') => { p.pos += 1; continue; }
            Some(b'}') => return Ok(m),
            _ => return Err(s!("expected ',' or '}' in packages.json")),
        }
    }
}

/* Iterate the directory containing `start` plus every parent, in order.
   Each yielded value ends with '/' or is "" (the topmost local dir). URL
   walks stop at `scheme://host/`; local walks stop after yielding "".
   Used by the resolver to walk up looking for `<dir>packages.json`. */
pub fn walk_up_dirs(start: &str) -> impl Iterator<Item = String> + '_ {
    let _ = start;
    let mut current = Some(start.to_string());
    core::iter::from_fn(move || {
        let dir = current.take()?;
        current = parent_dir(&dir);
        Some(dir)
    })
}

/* Directory containing `spec` — everything up to and including the last '/'.
   "lib/foo.py" -> "lib/", "foo.py" -> "", "https://x/a/b" -> "https://x/a/". */
pub fn dir_of(spec: &str) -> &str {
    match spec.rfind('/') {
        Some(i) => &spec[..=i],
        None => "",
    }
}

/* Resolve `target` against `dir`. Pass-through for absolute forms (URLs,
   leading '/'). Otherwise: leading `../` pops `base` one parent at a time;
   bare `..` and `.` are handled as terminals; remaining text is appended,
   with leading `./` stripped only when joining onto a non-empty base (so
   `join("", "./util.py")` preserves the `./` prefix the user wrote, while
   `join("lib/", "./util.py")` produces `lib/util.py` rather than
   `lib/./util.py`). */
pub fn join_relative(dir: &str, target: &str) -> String {
    if target.contains("://") || target.starts_with('/') {
        return target.to_string();
    }
    let mut base = dir.to_string();
    let mut t = target;
    while let Some(rest) = t.strip_prefix("../") {
        base = parent_dir(&base).unwrap_or_default();
        t = rest;
    }
    if t == ".." { return parent_dir(&base).unwrap_or_default(); }
    if t == "." || t.is_empty() { return base; }
    if !base.is_empty() {
        while let Some(rest) = t.strip_prefix("./") { t = rest; }
        if !base.ends_with('/') { base.push('/'); }
    }
    base.push_str(t);
    base
}

fn parent_dir(dir: &str) -> Option<String> {
    if dir.is_empty() { return None; }
    let trimmed = dir.trim_end_matches('/');
    /* URL guard: never strip the host. After "scheme://" the remaining must
       still contain a '/' for there to be a path to walk up. */
    if let Some(scheme_end) = trimmed.find("://") {
        let after = &trimmed[scheme_end + 3..];
        if !after.contains('/') { return None; }
    }
    match trimmed.rsplit_once('/') {
        Some(("", _)) => Some(String::new()),
        Some((head, _)) => Some(s!(str head, "/")),
        None => Some(String::new()),
    }
}

struct Reader<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn peek(&self) -> Option<u8> { self.src.get(self.pos).copied() }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if matches!(c, b' ' | b'\t' | b'\n' | b'\r') { self.pos += 1; } else { break; }
        }
    }

    fn expect(&mut self, c: u8, msg: &'static str) -> Result<(), String> {
        if self.peek() == Some(c) { self.pos += 1; Ok(()) } else { Err(s!(str msg)) }
    }

    fn read_string(&mut self) -> Result<String, String> {
        self.expect(b'"', "expected '\"' starting a string")?;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(s!("unterminated string in packages.json")),
                Some(b'"') => { self.pos += 1; return Ok(out); }
                Some(b'\\') => {
                    self.pos += 1;
                    let esc = self.peek().ok_or_else(|| s!("dangling '\\' in packages.json"))?;
                    match esc {
                        b'"'  => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/'  => out.push('/'),
                        b'n'  => out.push('\n'),
                        b't'  => out.push('\t'),
                        b'r'  => out.push('\r'),
                        _ => return Err(s!("unsupported escape '\\", char esc as char, "' in packages.json")),
                    }
                    self.pos += 1;
                }
                Some(c) => { out.push(c as char); self.pos += 1; }
            }
        }
    }

    fn read_imports_into(&mut self, out: &mut FxHashMap<String, String>) -> Result<(), String> {
        self.expect(b'{', "'imports' must be an object")?;
        self.skip_ws();
        if self.peek() == Some(b'}') { self.pos += 1; return Ok(()); }
        loop {
            self.skip_ws();
            let k = self.read_string()?;
            self.skip_ws();
            self.expect(b':', "expected ':' after import name")?;
            self.skip_ws();
            let v = self.read_string()?;
            out.insert(k, v);
            self.skip_ws();
            match self.peek() {
                Some(b',') => { self.pos += 1; continue; }
                Some(b'}') => { self.pos += 1; return Ok(()); }
                _ => return Err(s!("expected ',' or '}' in 'imports'")),
            }
        }
    }

    /* Skip a string or string-keyed object. Used to forgive future top-level
       keys; numeric/array/bool values are not supported anywhere in the
       manifest and surface as an error so a typo doesn't load silently. */
    fn skip_value(&mut self) -> Result<(), String> {
        match self.peek() {
            Some(b'"') => { let _ = self.read_string()?; Ok(()) }
            Some(b'{') => {
                self.pos += 1;
                self.skip_ws();
                if self.peek() == Some(b'}') { self.pos += 1; return Ok(()); }
                loop {
                    self.skip_ws();
                    let _ = self.read_string()?;
                    self.skip_ws();
                    self.expect(b':', "expected ':' in nested object")?;
                    self.skip_ws();
                    self.skip_value()?;
                    self.skip_ws();
                    match self.peek() {
                        Some(b',') => { self.pos += 1; continue; }
                        Some(b'}') => { self.pos += 1; return Ok(()); }
                        _ => return Err(s!("expected ',' or '}' in nested object")),
                    }
                }
            }
            _ => Err(s!("unsupported value in packages.json (only strings / string-objects)")),
        }
    }
}
