use alloc::string::String;

use super::CallFrame;

/* Runtime errors. Static-string variants avoid alloc on the hot error path;
   *Msg / Name / Attribute / Raised variants carry dynamic text so the user
   sees the actual offending name or object type instead of a generic
   "attribute not found". */
#[derive(Debug, Clone)]
pub enum VmErr {
    CallDepth, Heap, Budget, ZeroDiv, Overflow,
    Name(String),
    Type(&'static str),
    TypeMsg(String),
    Value(&'static str),
    Runtime(&'static str),
    Attribute(String),
    Raised(String),
}

impl VmErr {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CallDepth => "RecursionError: max depth",
            Self::Heap => "MemoryError: heap limit",
            Self::Budget => "RuntimeError: budget exceeded",
            Self::ZeroDiv => "ZeroDivisionError: division by zero",
            Self::Overflow => "OverflowError: integer too large for 47-bit Val",
            Self::Type(s) => s,
            Self::Value(s) => s,
            Self::Runtime(s) => s,
            Self::TypeMsg(_) => "TypeError",
            Self::Attribute(_) => "AttributeError",
            Self::Name(_) => "NameError",
            Self::Raised(_) => "Exception",
        }
    }

    pub fn render(&self) -> alloc::string::String {
        use crate::s;
        match self {
            Self::Name(n) => s!("NameError: name '", str n, "' is not defined"),
            Self::Raised(m) => s!("Exception: ", str m),
            Self::Type(m) => s!("TypeError: ", str m),
            Self::TypeMsg(m) => s!("TypeError: ", str m),
            Self::Value(m) => s!("ValueError: ", str m),
            Self::Runtime(m) => s!("RuntimeError: ", str m),
            Self::Attribute(m) => s!("AttributeError: ", str m),
            other => alloc::string::String::from(other.as_str()),
        }
    }

    /* Just the message portion of `render()`, without the "Class:" prefix.
       Used by the catch arm to populate `e.args` on native errors so
       `except E as e: print(e.args)` matches CPython for `1/0` etc.,
       not only for `raise X("msg")`. */
    pub fn message(&self) -> alloc::string::String {
        use alloc::string::String;
        match self {
            Self::Name(n) => crate::s!("name '", str n, "' is not defined"),
            Self::Type(m) | Self::Value(m) | Self::Runtime(m) => String::from(*m),
            Self::TypeMsg(m) | Self::Attribute(m) | Self::Raised(m) => m.clone(),
            Self::ZeroDiv => String::from("division by zero"),
            Self::Overflow => String::from("integer too large for 47-bit Val"),
            Self::CallDepth => String::from("max depth"),
            Self::Heap => String::from("heap limit"),
            Self::Budget => String::from("budget exceeded"),
        }
    }

    /* Same message as render(), but anchored at a source byte offset so the
       parser's Diagnostic renderer adds the rustc-style line/caret preview.
       Falls back to plain render() when no position is known. */
    pub fn render_at(&self, src: &str, byte_pos: Option<usize>, path: Option<&str>) -> alloc::string::String {
        let Some(pos) = byte_pos else { return self.render(); };
        crate::modules::parser::Diagnostic { start: pos, end: pos, msg: self.render() }
            .render(src, path)
    }

    /* Multi-frame traceback. The error site renders first as `error: ...`
       with a rustc-style source preview; each entry in `frames` appends a
       `note: called from <fname>` block walking outward from the innermost
       call to the entry chunk. */
    pub fn render_traceback(
        &self,
        error_src: &str,
        error_byte_pos: Option<usize>,
        error_path: Option<&str>,
        frames: &[CallFrame],
        function_names: &[alloc::string::String],
    ) -> alloc::string::String {
        let mut out = self.render_at(error_src, error_byte_pos, error_path);
        for f in frames.iter().rev() {
            let fname = function_names.get(f.fi)
                .map(|s| s.as_str()).unwrap_or("<anonymous>");
            let pos = f.call_byte_pos as usize;
            let path: Option<&str> = if f.caller_path.is_empty() { None } else { Some(f.caller_path.as_str()) };
            let note = crate::modules::parser::Diagnostic {
                start: pos, end: pos,
                msg: alloc::format!("called from {}()", fname),
            }.render(f.caller_source.as_str(), path);
            // Diagnostic prefixes "error:" by convention; rewrite to "note:"
            // for the chained frames so the topmost line stays the only red.
            let note = note.replacen("error:", "note:", 1);
            out.push('\n');
            out.push_str(&note);
        }
        out
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl core::fmt::Display for VmErr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.render())
    }
}

/* Out-of-line error constructors keep the hot dispatch loop linear in
   the icache; #[cold] + #[inline(never)] push them off the fast path. */
#[cold] #[inline(never)] pub fn cold_heap() -> VmErr { VmErr::Heap }
#[cold] #[inline(never)] pub fn cold_budget() -> VmErr { VmErr::Budget }
#[cold] #[inline(never)] pub fn cold_depth() -> VmErr { VmErr::CallDepth }
#[cold] #[inline(never)] pub fn cold_type(m: &'static str) -> VmErr { VmErr::Type(m) }
#[cold] #[inline(never)] pub fn cold_value(m: &'static str) -> VmErr { VmErr::Value(m) }
#[cold] #[inline(never)] pub fn cold_runtime(m: &'static str) -> VmErr { VmErr::Runtime(m) }
#[cold] #[inline(never)] pub fn cold_overflow() -> VmErr { VmErr::Overflow }
