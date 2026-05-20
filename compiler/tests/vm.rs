#[cfg(test)]
mod test {

    use compiler_lib::modules::lexer::lex;
    use compiler_lib::modules::parser::Parser;
    use compiler_lib::modules::vm::VM;
    use compiler_lib::modules::vm::types::{SchedulerStatus, VmErr};

    #[derive(serde::Deserialize)]
    struct Case {
        src: String,
        output: Vec<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        input: Vec<String>,
        #[serde(default)]
        events: Vec<String>,
        // Events pushed one-at-a-time after each PendingEvent yield (host-resume path).
        #[serde(default)]
        interactive_events: Vec<String>,
    }

    // Resume on each PendingEvent by pushing the next interactive_events entry.
    fn drive(vm: &mut VM, interactive: &[String]) -> Result<(), VmErr> {
        let mut idx = 0;
        loop {
            match vm.run() {
                Ok(_) => return Ok(()),
                Err(VmErr::HostYield(SchedulerStatus::PendingEvent)) => {
                    if idx >= interactive.len() { return Ok(()); }
                    vm.push_event(&interactive[idx]).expect("push_event");
                    idx += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    #[test]
    fn test_cases() {
        let cases: Vec<Case> = serde_json::from_str(include_str!("cases/vm.json")).expect("invalid JSON");

        for case in cases {
            let (tokens, lex_errs) = lex(&case.src);
            // Skip cases whose expected error is already raised by the lexer.
            if !lex_errs.is_empty()
                && let Some(expected) = &case.error
                && lex_errs.iter().any(|e| e.msg.contains(expected.as_str()))
            {
                continue;
            }
            let (chunk, _errors) = Parser::new(&case.src, tokens.into_iter()).parse();
            let mut vm = VM::new(&chunk);
            vm.input_buffer = case.input.clone();
            for evt in &case.events { vm.push_event(evt).expect("push_event"); }
            let result = drive(&mut vm, &case.interactive_events);

            match result {
                Ok(_obj) => { assert_eq!(vm.output, case.output, "output mismatch on: {:?}", case.src); }
                Err(e) => match &case.error {
                    Some(expected) => assert!(e.to_string().contains(expected.as_str()), "wrong error on {:?}: got '{}', expected '{}'", case.src, e, expected),
                    None => panic!("VM error on {:?}: {}", case.src, e),
                }
            }
        }
    }

    /* Reruns every vm.json case in strict_input mode (host-supplied buffer; reading past = RuntimeError). Lex/parse errors are also asserted here. */
    #[test]
    fn strict_cases() {
        let cases: Vec<Case> = serde_json::from_str(include_str!("cases/vm.json")).expect("invalid JSON");

        for case in cases {
            let (tokens, lex_errs) = lex(&case.src);
            // Lex errors are surfaced as diagnostics; match against the expected error and move on.
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
            for evt in &case.events { vm.push_event(evt).expect("push_event"); }
            let expects_input_error = case.input.is_empty() && (case.src.contains("input(") || case.src.contains("input ("));

            match drive(&mut vm, &case.interactive_events) {
                Ok(_) => {
                    assert!(!expects_input_error, "expected input() to error under strict mode for: {:?}", case.src);
                    assert_eq!(vm.output, case.output, "output mismatch on: {:?}", case.src);
                }
                Err(e) => match &case.error {
                    Some(expected) => assert!(e.to_string().contains(expected.as_str()), "wrong error on {:?}: got '{}', expected '{}'", case.src, e, expected),
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
