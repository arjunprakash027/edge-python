#[cfg(test)]
mod test {

    use compiler_lib::modules::lexer::lex;
    use compiler_lib::modules::parser::Parser;
    use compiler_lib::modules::vm::VM;

    #[derive(serde::Deserialize)]
    struct Case {
        src: String,
        output: Vec<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        input: Vec<String>,
    }

    #[test]
    fn test_cases() {
        let cases: Vec<Case> = serde_json::from_str(include_str!("cases/vm.json")).expect("invalid JSON");

        for case in cases {
            let (tokens, lex_errs) = lex(&case.src);
            // If a case expects an error matching a lex diagnostic, it's handled here.
            if !lex_errs.is_empty()
                && let Some(expected) = &case.error
                && lex_errs.iter().any(|e| e.msg.contains(expected.as_str()))
            {
                continue;
            }
            let (chunk, _errors) = Parser::new(&case.src, tokens.into_iter()).parse();
            let mut vm = VM::new(&chunk);
            vm.input_buffer = case.input.clone();
            let result = vm.run();

            match result {
                Ok(_obj) => {
                    assert_eq!(vm.output, case.output, "output mismatch on: {:?}", case.src);
                }
                Err(e) => match &case.error {
                    Some(expected) => assert!(
                        e.to_string().contains(expected.as_str()),
                        "wrong error on {:?}: got '{}', expected '{}'", case.src, e, expected
                    ),
                    None => panic!("VM error on {:?}: {}", case.src, e),
                }
            }
        }
    }

    /* Re-runs every vm.json case under `vm.strict_input = true` — the mode
       browser/WASI hosts use, where reading past the end of the host-supplied
       input buffer is a hard `RuntimeError` instead of returning empty.
       Lex/parse errors are also asserted (`test_cases` ignores them). */
    #[test]
    fn strict_cases() {
        let cases: Vec<Case> = serde_json::from_str(include_str!("cases/vm.json")).expect("invalid JSON");

        for case in cases {
            let (tokens, lex_errs) = lex(&case.src);
            // Lex errors are surfaced as parse-time diagnostics; if a case expects
            // an error matching one of them, treat it as handled and skip further work.
            if !lex_errs.is_empty() {
                if let Some(expected) = &case.error {
                    assert!(
                        lex_errs.iter().any(|e| e.msg.contains(expected.as_str())),
                        "wrong lex error on {:?}: got {:?}, expected '{}'",
                        case.src,
                        lex_errs.iter().map(|e| e.msg).collect::<Vec<_>>(),
                        expected
                    );
                    continue;
                }
                panic!("lex error on {:?}: {:?}", case.src, lex_errs.iter().map(|e| e.msg).collect::<Vec<_>>());
            }
            let (chunk, errs) = Parser::new(&case.src, tokens.into_iter()).parse();
            if !errs.is_empty() {
                match &case.error {
                    Some(expected) => {
                        assert!(
                            errs.iter().any(|e| e.msg.contains(expected.as_str())),
                            "wrong parse error on {:?}: got {:?}, expected '{}'",
                            case.src,
                            errs.iter().map(|e| &e.msg).collect::<Vec<_>>(),
                            expected
                        );
                        continue;
                    }
                    None => panic!("parse error on {:?}: {:?}", case.src, errs.iter().map(|e| &e.msg).collect::<Vec<_>>()),
                }
            }

            let mut vm = VM::new(&chunk);
            vm.strict_input = true;
            vm.input_buffer = case.input.clone();
            let expects_input_error = case.input.is_empty()
                && (case.src.contains("input(") || case.src.contains("input ("));

            match vm.run() {
                Ok(_) => {
                    assert!(
                        !expects_input_error,
                        "expected input() to error under strict mode for: {:?}", case.src
                    );
                    assert_eq!(vm.output, case.output, "output mismatch on: {:?}", case.src);
                }
                Err(e) => match &case.error {
                    Some(expected) => assert!(
                        e.to_string().contains(expected.as_str()),
                        "wrong error on {:?}: got '{}', expected '{}'", case.src, e, expected
                    ),
                    None if expects_input_error => assert!(
                        e.to_string().contains("input"),
                        "expected input RuntimeError under strict mode for: {:?}, got: {}",
                        case.src, e
                    ),
                    None => panic!("VM error on {:?}: {}", case.src, e),
                }
            }
        }
    }
}