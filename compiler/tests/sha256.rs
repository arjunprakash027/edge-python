/* SHA-256 unit tests. The four FIPS 180-2 / RFC 6234 vectors are the
   acceptance bar for any new SHA-256 implementation; passing them is
   strong evidence the constants and round structure are right. Plus
   round-trip and rejection cases for the hex helpers used by the
   integrity parser. */

#[cfg(test)]
mod test {
    use compiler_lib::modules::sha256::{sha256, hex_encode, hex_decode_32};

    fn expect(input: &[u8], hex: &str) {
        assert_eq!(hex_encode(&sha256(input)), hex,
            "sha256 mismatch for input of length {}", input.len());
    }

    #[test]
    fn nist_empty() {
        expect(b"",
            "e3b0c44298fc1c149afbf4c8996fb924\
             27ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn nist_abc() {
        expect(b"abc",
            "ba7816bf8f01cfea414140de5dae2223\
             b00361a396177a9cb410ff61f20015ad");
    }

    /* 56-byte input lands exactly on the padding-block boundary — exercises
       the path where the length-encoding spills into a second block. */
    #[test]
    fn nist_two_block() {
        expect(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "248d6a61d20638b8e5c026930c3e6039\
             a33ce45964ff2167f6ecedd419db06c1");
    }

    /* Million 'a's — the long-input vector. Validates that block iteration
       (15625 blocks here) accumulates correctly. */
    #[test]
    fn nist_million_a() {
        let input = vec![b'a'; 1_000_000];
        expect(&input,
            "cdc76e5c9914fb9281a1c7e284d73e67\
             f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn hex_round_trip() {
        let bytes: [u8; 32] = [
            0xde, 0xad, 0xbe, 0xef, 0x00, 0xff, 0x01, 0x02,
            0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a,
            0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12,
            0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a,
        ];
        let hex = hex_encode(&bytes);
        assert_eq!(hex.len(), 64);
        assert_eq!(hex_decode_32(&hex), Some(bytes));
    }

    #[test]
    fn hex_decode_rejects_bad_input() {
        // wrong length
        assert!(hex_decode_32("abc").is_none());
        assert!(hex_decode_32(&"a".repeat(63)).is_none());
        assert!(hex_decode_32(&"a".repeat(65)).is_none());
        // non-hex char
        assert!(hex_decode_32(&format!("{}{}", "a".repeat(63), "z")).is_none());
        // both cases accepted
        assert!(hex_decode_32(&"A".repeat(64)).is_some());
        assert!(hex_decode_32(&"a".repeat(64)).is_some());
    }
}
