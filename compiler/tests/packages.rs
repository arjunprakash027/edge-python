/* 
JSON-driven runner for the `packages` subsystem (imports, resolver, CallExtern, code-module inlining).
See `cases/packages.json` for the case schema (src, output, error, input, modules, manifests, and optional chunk-shape assertions: expect_externs, expect_functions, error_span_covers). 
*/

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use compiler_lib::modules::lexer::lex;
    use compiler_lib::modules::parser::Parser;
    use compiler_lib::modules::vm::VM;
    use compiler_lib::modules::packages::NativeBinding;

    use crate::common::{TestResolver, test_native};

    /* `native`/`code` drive resolve(); optional `bytes` drives fetch_bytes() for `#sha256-` cases. */
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum ModuleDef {
        Native {
            native: Vec<String>,
            #[serde(default)]
            bytes: Option<String>,
        },
        Code {
            code: String,
            #[serde(default)]
            bytes: Option<String>,
        },
    }

    /* Per-directory `packages.json` for walk-up cases; flat fixtures use `aliases` instead. */
    #[derive(serde::Deserialize)]
    struct ManifestDef {
        #[serde(default)]
        imports: HashMap<String, String>,
        #[serde(default)]
        extends: Option<String>,
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
        /* Synthetic root `packages.json`; nested entries in `manifests` shadow this. */
        #[serde(default)]
        aliases: HashMap<String, String>,
        /* Nested manifests by directory; exercises walk-up, `extends`, and circular-extends paths. */
        #[serde(default)]
        manifests: HashMap<String, ManifestDef>,
        #[serde(default)]
        expect_externs: Option<usize>,
        #[serde(default)]
        expect_functions: Option<usize>,
        #[serde(default)]
        error_span_covers: Option<String>,
    }

    fn build_resolver(modules: &HashMap<String, ModuleDef>, aliases: &HashMap<String, String>, manifests: &HashMap<String, ManifestDef>) -> TestResolver {
        let mut r = TestResolver::new();
        for (spec, def) in modules {
            match def {
                ModuleDef::Native { native, bytes } => {
                    let bindings: Vec<NativeBinding> = native.iter()
                        .map(|n| test_native(n)
                        .unwrap_or_else(|| panic!("unknown test native: {}", n)))
                        .collect();
                    r = r.with_native(spec, bindings);
                    if let Some(b) = bytes { r = r.with_bytes(spec, b.clone().into_bytes()); }
                }
                ModuleDef::Code { code, bytes } => {
                    r = r.with_code(spec, code);
                    if let Some(b) = bytes { r = r.with_bytes(spec, b.clone().into_bytes()); }
                }
            }
            /* Auto self-alias bare-name modules so flat fixtures work without explicit `aliases`. */
            if !spec.contains('/') {
                r = r.with_alias(spec, spec);
            }
        }
        for (name, target) in aliases {
            r = r.with_alias(name, target);
        }
        for (dir, m) in manifests {
            let pairs: Vec<(&str, &str)> = m.imports.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            r = r.with_manifest(dir, &pairs, m.extends.as_deref());
        }
        r
    }

    /* NoopResolver path: imports must produce a clean diagnostic, not panic. Lives in Rust since the JSON runner always builds a TestResolver. */
    #[test]
    fn noop_resolver_default() {
        let src = "from json import dumps";
        let (tokens, _) = lex(src);
        let (_, errs) = Parser::new(src, tokens.into_iter()).parse();
        assert!(errs.iter().any(|d| d.msg.contains("not found") || d.msg.contains("not configured")), "expected NoopResolver error, got: {:?}", errs.iter().map(|e| &e.msg).collect::<Vec<_>>());
    }

    #[test]
    fn packages_cases() {
        let cases: Vec<Case> = serde_json::from_str(
            include_str!("cases/packages.json")
        ).expect("invalid JSON");

        for case in cases {
            let resolver = Box::new(build_resolver(&case.modules, &case.aliases, &case.manifests));
            let (tokens, _lex_errs) = lex(&case.src);
            let parser = Parser::with_resolver(&case.src, tokens.into_iter(), resolver);
            let (chunk, parse_errs) = parser.parse();

            // Parse-time errors: match `error` substring plus optional span anchor.
            if !parse_errs.is_empty() {
                let combined = parse_errs.iter().map(|d| d.msg.as_str()).collect::<Vec<_>>().join(" | ");
                let expected = case.error.as_deref().unwrap_or_else(|| panic!("unexpected parse error on {:?}: {}", case.src, combined));
                assert!(combined.contains(expected), "parse error mismatch on {:?}: got '{}', expected substring '{}'", case.src, combined, expected);
                if let Some(needle) = &case.error_span_covers {
                    let pos = case.src.find(needle).unwrap_or_else(|| panic!("error_span_covers '{}' not found in src", needle));
                    let d = &parse_errs[0];
                    assert_eq!(d.start, pos, "diag start mismatch on {:?}", case.src);
                    assert_eq!(d.end, pos + needle.len(), "diag end mismatch on {:?}", case.src);
                }
                continue;
            }

            // Compile-time chunk-shape assertions (no parse errors).
            if let Some(n) = case.expect_externs { assert_eq!(chunk.extern_table.len(), n, "extern_table size mismatch on {:?}", case.src); }
            if let Some(n) = case.expect_functions { assert_eq!(chunk.functions.len(), n, "functions count mismatch on {:?}", case.src); }

            let mut vm = VM::new(&chunk);
            vm.input_buffer = case.input.clone();
            match vm.run() {
                Ok(_) => {
                    if case.error.is_some() { panic!("expected error on {:?}, got success with output {:?}", case.src, vm.output); }
                    assert_eq!(vm.output, case.output, "output mismatch on: {:?}", case.src);
                }
                Err(e) => match &case.error {
                    Some(expected) => assert!(e.to_string().contains(expected.as_str()), "runtime error mismatch on {:?}: got '{}', expected substring '{}'", case.src, e, expected),
                    None => panic!("VM error on {:?}: {}", case.src, e),
                }
            }
        }
    }
}
