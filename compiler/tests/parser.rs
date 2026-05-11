#[cfg(test)]
mod test {

    use compiler_lib::modules::lexer::lex;
    use compiler_lib::modules::parser::{Parser, Value, Diagnostic};

    #[derive(serde::Deserialize)]
    struct Case {
        src: String,
        constants: Vec<String>,
        names: Vec<String>,
        instructions: Vec<(String, u16)>,
        #[serde(default)]
        functions: usize,
        #[serde(default)]
        classes: usize,
        #[serde(default)]
        errors: Vec<String>,
    }

    #[test]
    fn test_cases() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("cases/parser.json")).expect("invalid JSON");

        for case in cases {
            let (tokens, lex_errs) = lex(&case.src);
            let mut parser = Parser::new(&case.src, tokens.into_iter());
            for e in lex_errs { parser.errors.push(Diagnostic { start: e.start, end: e.end, msg: e.msg.into() }); }
            let (chunk, diagnostics) = parser.parse();

            let constants: Vec<String> = chunk
                .constants
                .iter()
                .map(|v| match v {
                    Value::Str(s) => s.clone(),
                    Value::Bytes(b) => format!("b{:?}",
                    String::from_utf8_lossy(b).to_string()),
                    Value::Int(i) => i.to_string(),
                    Value::LongInt(i) => i.to_string(),
                    Value::Float(f) => f.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::None => "None".to_string(),
                })
                .collect();

            let instructions: Vec<(String, u16)> = chunk
                .instructions
                .iter()
                // Debug-format opcodes for snapshot comparison (test-only).
                .map(|i| (format!("{:?}", i.opcode), i.operand))
                .collect();

            assert_eq!(
                constants, case.constants,
                "constants mismatch on: {:?}",
                case.src
            );
            assert_eq!(chunk.names, case.names, "names mismatch on: {:?}", case.src);
            assert_eq!(
                instructions, case.instructions,
                "bytecode mismatch on: {:?}",
                case.src
            );
            assert_eq!(
                chunk.functions.len(),
                case.functions,
                "functions mismatch on: {:?}",
                case.src
            );
            assert_eq!(
                chunk.classes.len(),
                case.classes,
                "classes mismatch on: {:?}",
                case.src
            );

            if !case.errors.is_empty() {
                let actual: Vec<String> = diagnostics.iter().map(|e| e.msg.clone()).collect();
                assert_eq!(actual, case.errors, "errors mismatch on: {:?}", case.src);
            }
        }
    }
}
