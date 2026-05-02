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
            let (tokens, _) = lex(&case.src);
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
}