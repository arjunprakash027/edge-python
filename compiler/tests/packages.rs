/* JSON-driven runner for the `packages` subsystem (imports, resolver,
   CallExtern dispatch, code module inlining).

   Each case in `cases/packages.json` declares:
     * `src`     — the EdgePython source to compile and run
     * `output`  — expected stdout lines (printed via `print()`)
     * `error`   — substring expected in the parse/run error (optional)
     * `input`   — lines fed to `input()` (optional)
     * `modules` — map of module spec → { native: [...] } | { code: "..." }
                   The runner builds a TestResolver from this map: native
                   names are looked up in `crate::common::test_native`; code
                   sources are stored verbatim and parsed by the importer.

   Optional compile-time inspection fields (let JSON cases assert on chunk
   shape, replacing what used to live in separate Rust unit tests):
     * `expect_externs`     — chunk.extern_table.len() must equal this
     * `expect_functions`   — chunk.functions.len() must equal this (top
                              level, counting inlined code-module fns)
     * `error_span_covers`  — substring whose source span the diagnostic
                              must cover exactly (start..end matches the
                              substring's position in `src`) */

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use compiler_lib::modules::lexer::lex;
    use compiler_lib::modules::parser::Parser;
    use compiler_lib::modules::vm::VM;
    use compiler_lib::modules::packages::NativeBinding;

    use crate::common::{TestResolver, test_native, load_wasm_bindings, wasm_example_bytes};

    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum ModuleDef {
        Native { native: Vec<String> },
        Code { code: String },
    }

    #[derive(serde::Deserialize)]
    struct Case {
        src: String,
        #[serde(default)]
        output: Vec<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        input: Vec<String>,
        #[serde(default)]
        modules: HashMap<String, ModuleDef>,
        #[serde(default)]
        expect_externs: Option<usize>,
        #[serde(default)]
        expect_functions: Option<usize>,
        #[serde(default)]
        error_span_covers: Option<String>,
    }

    fn build_resolver(modules: &HashMap<String, ModuleDef>) -> TestResolver {
        let mut r = TestResolver::new();
        for (spec, def) in modules {
            match def {
                ModuleDef::Native { native } => {
                    let bindings: Vec<NativeBinding> = native.iter()
                        .map(|n| test_native(n)
                            .unwrap_or_else(|| panic!("unknown test native: {}", n)))
                        .collect();
                    r = r.with_native(spec, bindings);
                }
                ModuleDef::Code { code } => { r = r.with_code(spec, code); }
            }
        }
        r
    }

    /* `Parser::new` (no resolver) defaults to NoopResolver. Verify it
       rejects imports with a clean diagnostic instead of panicking, so
       hosts that never wire up a resolver still get safe behavior.
       Can't live in JSON — the runner always constructs a TestResolver. */
    #[test]
    fn noop_resolver_default() {
        let src = "from json import dumps";
        let (tokens, _) = lex(src);
        let (_, errs) = Parser::new(src, tokens.into_iter()).parse();
        assert!(errs.iter().any(|d|
            d.msg.contains("not found") || d.msg.contains("not configured")),
            "expected NoopResolver error, got: {:?}",
            errs.iter().map(|e| &e.msg).collect::<Vec<_>>());
    }

    /* End-to-end cross-language smoke. Builds the SDK's `reference` example
       to a real wasm32 binary, loads it through the production loader, and
       uses the resulting `NativeBinding`s from an EdgePython script. The
       only test that can't live in JSON: it shells out to `cargo` to
       produce the artifact. Single source of truth: same `reference.rs`
       referenced from `documentation/reference/writing-modules.md`.

       Covers every wire type the SDK supports today: i64 (`add`, `square`),
       f64 (`area`), bool input + i64 return (`pick`), and i64 input + bool
       return (`even`). A regression here means the macro / loader / NaN-box
       round-trip is broken end to end. */
    #[test]
    fn loads_wasm() {
        let bytes = wasm_example_bytes("reference");
        let bindings = load_wasm_bindings(&bytes)
            .expect("loading the SDK's reference.wasm should succeed");

        let names: Vec<&str> = bindings.iter().map(|b| b.name.as_str()).collect();
        for export in ["add", "square", "area", "even", "pick"] {
            assert!(names.contains(&export), "expected '{}' export in {:?}", export, names);
        }

        let resolver = TestResolver::new().with_native("math", bindings);
        let src = "\
from math import add, square, area, even, pick
print(add(2, square(3)))
print(area(2.0))
print(even(4))
print(even(5))
print(pick(True, 10, 99))
print(pick(False, 10, 99))
";
        let (tokens, _) = lex(src);
        let parser = Parser::with_resolver(src, tokens.into_iter(), Box::new(resolver));
        let (chunk, errs) = parser.parse();
        assert!(errs.is_empty(), "parse errors: {:?}", errs.iter().map(|d| &d.msg).collect::<Vec<_>>());

        let mut vm = VM::new(&chunk);
        if let Err(e) = vm.run() {
            panic!("vm should run cleanly, got: {}", e);
        }
        assert_eq!(
            vm.output,
            vec!["11", "12.566370614359172", "True", "False", "99", "10"],
            "wasm round-trip mismatch",
        );
    }

    #[test]
    fn packages_cases() {
        let cases: Vec<Case> = serde_json::from_str(
            include_str!("cases/packages.json")
        ).expect("invalid JSON");

        for case in cases {
            let resolver = Box::new(build_resolver(&case.modules));
            let (tokens, _lex_errs) = lex(&case.src);
            let parser = Parser::with_resolver(&case.src, tokens.into_iter(), resolver);
            let (chunk, parse_errs) = parser.parse();

            // Parse-time errors: match the case's `error` substring + optional
            // span anchor.
            if !parse_errs.is_empty() {
                let combined = parse_errs.iter()
                    .map(|d| d.msg.as_str()).collect::<Vec<_>>().join(" | ");
                let expected = case.error.as_deref().unwrap_or_else(||
                    panic!("unexpected parse error on {:?}: {}", case.src, combined));
                assert!(combined.contains(expected),
                    "parse error mismatch on {:?}: got '{}', expected substring '{}'",
                    case.src, combined, expected);
                if let Some(needle) = &case.error_span_covers {
                    let pos = case.src.find(needle).unwrap_or_else(||
                        panic!("error_span_covers '{}' not found in src", needle));
                    let d = &parse_errs[0];
                    assert_eq!(d.start, pos, "diag start mismatch on {:?}", case.src);
                    assert_eq!(d.end, pos + needle.len(), "diag end mismatch on {:?}", case.src);
                }
                continue;
            }

            // Compile-time chunk-shape assertions (no parse errors).
            if let Some(n) = case.expect_externs {
                assert_eq!(chunk.extern_table.len(), n,
                    "extern_table size mismatch on {:?}", case.src);
            }
            if let Some(n) = case.expect_functions {
                assert_eq!(chunk.functions.len(), n,
                    "functions count mismatch on {:?}", case.src);
            }

            let mut vm = VM::new(&chunk);
            vm.input_buffer = case.input.clone();
            match vm.run() {
                Ok(_) => {
                    if case.error.is_some() {
                        panic!("expected error on {:?}, got success with output {:?}",
                            case.src, vm.output);
                    }
                    assert_eq!(vm.output, case.output, "output mismatch on: {:?}", case.src);
                }
                Err(e) => match &case.error {
                    Some(expected) => assert!(e.to_string().contains(expected.as_str()),
                        "runtime error mismatch on {:?}: got '{}', expected substring '{}'",
                        case.src, e, expected),
                    None => panic!("VM error on {:?}: {}", case.src, e),
                }
            }
        }
    }
}