//! JSON for the adapter. The value type, parser, and serializer live in the
//! `muse-msp-client` crate (std only); this module re-exports them and adds
//! the adapter's own helpers. ACP frames travel as `serde_json` values through
//! `acp-lite`, and `to_value`/`from_value` convert at that one boundary.

pub use muse_msp_client::json::*;

pub fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::with_capacity(data.len() / 3 * 4 + 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | (*c.get(2).unwrap_or(&0) as u32);
        o.push(T[((n >> 18) & 63) as usize] as char);
        o.push(T[((n >> 12) & 63) as usize] as char);
        o.push(if c.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        o.push(if c.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    o
}

pub fn mint_id(prefix: &str, counter: &std::sync::atomic::AtomicU64) -> String {
    // Short unique ids: prefix + counter + urandom tail (bounded read).
    let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tail = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = Read::read_exact(&mut f, &mut tail);
    }
    let hex: String = tail.iter().map(|b| format!("{b:02x}")).collect();
    format!("{prefix}{n}-{hex}")
}

/// `j` as a `serde_json` value, for the ACP connection. Numbers keep their
/// digits (`arbitrary_precision`; only exponent notation is normalized, as
/// `1e400` to `1e+400`) and objects their key order (`preserve_order`), so
/// converting back gives the same `J`.
pub fn to_value(j: &J) -> serde_json::Value {
    use serde_json::Value;
    match j {
        J::Null => Value::Null,
        J::Bool(b) => Value::Bool(*b),
        J::Num(text) => text
            .parse::<serde_json::Number>()
            .map(Value::Number)
            .unwrap_or(Value::Null),
        J::Str(s) => Value::String(s.clone()),
        J::Arr(items) => Value::Array(items.iter().map(to_value).collect()),
        J::Obj(pairs) => Value::Object(
            pairs
                .iter()
                .map(|(key, value)| (key.clone(), to_value(value)))
                .collect(),
        ),
    }
}

/// A `serde_json` value from the ACP connection as `J`.
pub fn from_value(value: &serde_json::Value) -> J {
    use serde_json::Value;
    match value {
        Value::Null => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Number(n) => J::Num(n.to_string()),
        Value::String(s) => J::Str(s.clone()),
        Value::Array(items) => J::Arr(items.iter().map(from_value).collect()),
        Value::Object(map) => J::Obj(
            map.iter()
                .map(|(key, value)| (key.clone(), from_value(value)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod conversion_tests {
    use super::*;

    #[test]
    fn values_round_trip_with_number_text_and_key_order() {
        let text = r#"{"z":1,"a":[1.50,-0,12345678901234567890123,1e+400],"m":{"b":null,"a":true},"s":"é\n\u2028"}"#;
        let j = parse_json(text).unwrap();
        let value = to_value(&j);
        assert_eq!(value.to_string(), j_to_string(&j));
        assert_eq!(j_to_string(&from_value(&value)), j_to_string(&j));
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["z", "a", "m", "s"]);
    }
}

#[cfg(test)]
mod differential_tests {
    use super::b64;
    use super::parse_json;
    use std::io::Write;
    use std::process::{Command, Stdio};

    fn corpus() -> Vec<&'static str> {
        vec![
            // Valid: scalars, numbers, escapes, whitespace, nesting.
            "null",
            "true",
            "false",
            "0",
            "-0",
            "42",
            "-17",
            "3.25",
            "-0.5",
            "1e10",
            "1E-3",
            "123.456e+7",
            "\"\"",
            "\"a\\\"b\"",
            "\"tab\\tnl\\n\"",
            "\"back\\\\slash\"",
            "\"\\ud83d\\ude00\"",
            "\"\\u00e9\"",
            "\"\\ud834\\udd1e\"",
            "\"\\u0000\"",
            "\"\\u001f\"",
            "\"\\u007f\"",
            "\"汉字\"",
            "\"\\u0020\"",
            "[]",
            "{}",
            "[1,2,3]",
            "{\"a\":1,\"b\":null}",
            " [ 1 , 2 ] ",
            "\t{\n\t\"k\"\r:\t[ true , false ]\n}\n",
            "[[[[[[[[\"deep\"]]]]]]]]",
            "{\"outer\":{\"inner\":[{\"leaf\":\"value\"}]}}",
            // Invalid: things a hostile or buggy emitter might send.
            "",
            "   ",
            "tru",
            "nul",
            "01",
            "+1",
            ".5",
            "1.",
            "-",
            "1e",
            "1e+",
            "[1.2.3]",
            "[1,]",
            "{,}",
            "{\"a\":1,}",
            "{'a':1}",
            "{a:1}",
            "[1 2]",
            "// comment",
            "/* comment */",
            "{} trailing",
            "[1] junk",
            "\"unterminated",
            "\"bad\\escape\"",
            "\"raw\nnewline\"",
            "\"raw\ttab\"",
            "[",
            "]",
            "{",
            "}",
            "{\"a\"}",
            "{\"a\":}",
            "[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[0]",
            "]",
            "[,]",
            "{\"a\":1}{\"b\":2}",
        ]
    }

    /// Python's `json` module classifies the corpus; the adapter must accept
    /// every Python-valid document and reject the rest. Lone surrogates are
    /// deliberately absent: Python accepts them and strict RFC 8259 does not
    /// (covered by `unicode_surrogate_pairs_are_decoded_strictly`).
    #[test]
    fn acceptance_agrees_with_python_json() {
        let docs = corpus();
        let mut child = Command::new("python3")
            .arg("-c")
            .arg(concat!(
                "import base64, json, sys\n",
                "for line in sys.stdin:\n",
                "    doc = base64.b64decode(line.strip()).decode('utf-8')\n",
                "    try:\n",
                "        json.loads(doc)\n",
                "        print(1)\n",
                "    except Exception:\n",
                "        print(0)\n"
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("python3 is required by the test suite");
        {
            let stdin = child.stdin.as_mut().expect("python stdin");
            for doc in &docs {
                // Base64 keeps raw newlines inside documents framed as one
                // line each, so the classifier sees exactly the corpus.
                writeln!(stdin, "{}", b64(doc.as_bytes())).expect("write corpus");
            }
        }
        let out = child.wait_with_output().expect("python completes");
        assert!(out.status.success(), "python classifier failed: {out:?}");
        let verdicts: Vec<bool> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim() == "1")
            .collect();
        assert_eq!(verdicts.len(), docs.len(), "classifier line count");
        let (mut valid, mut invalid) = (0usize, 0usize);
        for (doc, python_ok) in docs.iter().zip(&verdicts) {
            let ours = parse_json(doc).is_ok();
            assert_eq!(
                ours, *python_ok,
                "divergence from Python json on {doc:?}: python={python_ok} adapter={ours}"
            );
            if *python_ok {
                valid += 1;
            } else {
                invalid += 1;
            }
        }
        assert!(valid > 20 && invalid > 20, "corpus lost its edge cases");

        // Python's json module accepts IEEE extension constants that strict
        // RFC 8259 forbids; the adapter must keep rejecting them.
        for doc in ["NaN", "Infinity", "-Infinity"] {
            assert!(
                parse_json(doc).is_err(),
                "adapter must stay strict against Python extension {doc}"
            );
        }
    }
}
