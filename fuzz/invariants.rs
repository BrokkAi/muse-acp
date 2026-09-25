// Shared by libFuzzer and the ordinary deterministic seed-corpus test.
pub fn check(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    check_frame(text);
    // Exercise truncated frames at a fuzz-selected UTF-8 boundary, in addition
    // to letting the engine mutate malformed frames directly.
    let cut = data.first().copied().unwrap_or(0) as usize % (text.len() + 1);
    if text.is_char_boundary(cut) {
        check_frame(&text[..cut]);
    }
    let quoted = crate::json::esc(text);
    let decoded = crate::json::parse_json(&quoted).expect("escaped text parses");
    assert_eq!(decoded.as_str(), Some(text));
    // Parsing an invalid frame must leave the next independent frame healthy.
    assert!(crate::json::parse_json(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).is_ok());
}

fn check_frame(text: &str) {
    if let Ok(value) = crate::json::parse_json(text) {
        let encoded = crate::json::j_to_string(&value);
        let reparsed = crate::json::parse_json(&encoded).expect("serializer produced invalid JSON");
        assert_eq!(encoded, crate::json::j_to_string(&reparsed));
        // Independent strict parser oracle (arbitrary_precision preserves the
        // unbounded numeric lexemes that the adapter deliberately supports).
        let reference: serde_json::Value =
            serde_json::from_str(text).expect("accepted invalid JSON");
        let roundtrip: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(reference, roundtrip);
    }
}
