/* End-to-end integrity-verification tests.

   The parser is the trust boundary: when an import spec carries a
   `#sha256-...` fragment, the parser must hash the bytes the resolver
   returns and refuse to compile on any mismatch, malformed fragment, or
   resolver that doesn't support `fetch_bytes`. Each test below pins one
   slice of that contract end-to-end through Parser::with_resolver. */

#[cfg(test)]
mod test {
    use compiler_lib::modules::lexer::lex;
    use compiler_lib::modules::parser::Parser;
    use compiler_lib::modules::packages::NativeBinding;
    use compiler_lib::modules::sha256::{sha256, hex_encode};

    use crate::common::{TestResolver, test_native};

    /* Build a resolver that knows about a native module under `spec`,
       backed by `bytes` for integrity checks. Both `with_native` (gives
       the parser something to actually resolve) and `with_bytes` (lets
       fetch_bytes return real content) need to agree on the spec. */
    fn resolver_with_native(spec: &str, bytes: Vec<u8>, native_name: &str) -> TestResolver {
        let bindings: Vec<NativeBinding> = [native_name].iter()
            .map(|n| test_native(n).expect("unknown test native"))
            .collect();
        TestResolver::new()
            .with_native(spec, bindings)
            .with_bytes(spec, bytes)
    }

    fn parse(src: &str, resolver: TestResolver) -> Vec<String> {
        let (tokens, _) = lex(src);
        let p = Parser::with_resolver(src, tokens.into_iter(), Box::new(resolver));
        let (_, errs) = p.parse();
        errs.into_iter().map(|d| d.msg).collect()
    }

    /* Happy path: bytes hash to the value declared in the fragment, parser
       proceeds normally, no diagnostics. */
    #[test]
    fn matching_hash_compiles() {
        let bytes = b"reproducible module bytes".to_vec();
        let hex = hex_encode(&sha256(&bytes));
        let spec_url = "https://example.com/m.wasm";
        let src = format!("from \"{}#sha256-{}\" import const_42\nprint(const_42())",
            spec_url, hex);
        let errs = parse(&src, resolver_with_native(spec_url, bytes, "const_42"));
        assert!(errs.is_empty(), "expected clean parse, got: {:?}", errs);
    }

    /* The whole point: a mismatched hash refuses to compile, and the
       diagnostic shows both expected and actual hashes so the user knows
       which side drifted. */
    #[test]
    fn mismatched_hash_rejects() {
        let bytes = b"the actual served bytes".to_vec();
        let actual = hex_encode(&sha256(&bytes));
        // 64 hex chars but not the right ones.
        let wrong = "0".repeat(64);
        let src = format!("from \"https://m.wasm#sha256-{}\" import const_42", wrong);
        let resolver = resolver_with_native("https://m.wasm", bytes, "const_42");
        let errs = parse(&src, resolver);
        assert!(errs.iter().any(|m| m.contains("integrity check failed")),
            "expected mismatch error, got: {:?}", errs);
        assert!(errs.iter().any(|m| m.contains(&wrong)),
            "expected error to surface the declared hash, got: {:?}", errs);
        assert!(errs.iter().any(|m| m.contains(&actual)),
            "expected error to surface the computed hash, got: {:?}", errs);
    }

    /* Hex chars outside [0-9a-fA-F] — caught at the parse stage, before
       any fetch_bytes call. */
    #[test]
    fn malformed_hex_rejects() {
        let mut hex = "z".repeat(64);
        hex.replace_range(0..0, ""); // hex is exactly 64 chars
        let src = format!("from \"https://m.wasm#sha256-{}\" import f", hex);
        let errs = parse(&src, TestResolver::new());
        assert!(errs.iter().any(|m| m.contains("invalid hex")),
            "expected malformed-hex error, got: {:?}", errs);
    }

    /* Wrong fragment length — also caught upstream of the resolver. */
    #[test]
    fn wrong_length_rejects() {
        let src = "from \"https://m.wasm#sha256-deadbeef\" import f";
        let errs = parse(src, TestResolver::new());
        assert!(errs.iter().any(|m| m.contains("64 hex chars")),
            "expected length error, got: {:?}", errs);
    }

    /* Unknown algorithm prefix — only sha256 is honored today; anything
       else is a hard error so future syntax changes don't silently go
       unverified. */
    #[test]
    fn unknown_algorithm_rejects() {
        let src = "from \"https://m.wasm#md5-abcdef\" import f";
        let errs = parse(src, TestResolver::new());
        assert!(errs.iter().any(|m| m.contains("unrecognized integrity fragment")),
            "expected unknown-algorithm error, got: {:?}", errs);
    }

    /* Resolver that doesn't seed bytes: even if the hash-shape is
       well-formed, fetch_bytes returns Err and the parser surfaces the
       host's message verbatim. The `#sha256-` declaration is a contract
       the host MUST honour or fail loud. */
    #[test]
    fn resolver_without_fetch_bytes_rejects() {
        // 64 zeros — well-formed shape, no resolver bytes available.
        let zeros = "0".repeat(64);
        let src = format!("from \"https://m.wasm#sha256-{}\" import f", zeros);
        // TestResolver with nothing seeded: fetch_bytes returns Err.
        let errs = parse(&src, TestResolver::new());
        assert!(errs.iter().any(|m| m.contains("integrity verification not supported")),
            "expected unsupported-host error, got: {:?}", errs);
    }

    /* Imports without an integrity fragment behave exactly as before:
       fetch_bytes is never called, so even a resolver with no bytes seeded
       loads cleanly. Pins backwards compatibility. */
    #[test]
    fn no_fragment_skips_verification() {
        let bindings: Vec<NativeBinding> = [test_native("const_42").unwrap()].into();
        let resolver = TestResolver::new().with_native("plain", bindings);
        let src = "from plain import const_42\nprint(const_42())";
        let errs = parse(src, resolver);
        assert!(errs.is_empty(), "expected clean parse, got: {:?}", errs);
    }
}
