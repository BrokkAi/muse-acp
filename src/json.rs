//! Minimal JSON value + parser (std only). NDJSON framing is handled by callers.

#[derive(Debug, Clone)]
pub enum J {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    pub fn get(&self, key: &str) -> Option<&J> {
        if let J::Obj(pairs) = self {
            for (k, v) in pairs {
                if k == key {
                    return Some(v);
                }
            }
        }
        None
    }
    pub fn as_str(&self) -> Option<&str> {
        if let J::Str(s) = self { Some(s) } else { None }
    }
    pub fn as_u64(&self) -> Option<u64> {
        if let J::Num(n) = self {
            n.parse().ok()
        } else {
            None
        }
    }
}

const MAX_DEPTH: usize = 64;

struct Parser<'a> {
    b: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            b: s.as_bytes(),
            pos: 0,
            depth: 0,
        }
    }
    fn skip_ws(&mut self) {
        while self.pos < self.b.len() && matches!(self.b[self.pos], b' ' | b'\t' | b'\n' | b'\r') {
            self.pos += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }
    fn parse_value(&mut self) -> Result<J, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'n') => self.parse_lit("null", J::Null),
            Some(b't') => self.parse_lit("true", J::Bool(true)),
            Some(b'f') => self.parse_lit("false", J::Bool(false)),
            Some(b'"') => Ok(J::Str(self.parse_string()?)),
            Some(b'[') | Some(b'{') => {
                if self.depth >= MAX_DEPTH {
                    return Err("nesting too deep".to_string());
                }
                self.depth += 1;
                let r = if self.peek() == Some(b'[') {
                    self.parse_array()
                } else {
                    self.parse_object()
                };
                self.depth -= 1;
                r
            }
            Some(c) if c == b'-' || c.is_ascii_digit() => self.parse_number(),
            Some(c) => Err(format!("unexpected char '{}' at {}", c as char, self.pos)),
            None => Err("unexpected end of input".to_string()),
        }
    }
    fn parse_lit(&mut self, lit: &str, v: J) -> Result<J, String> {
        if self.b.len() >= self.pos + lit.len()
            && &self.b[self.pos..self.pos + lit.len()] == lit.as_bytes()
        {
            self.pos += lit.len();
            Ok(v)
        } else {
            Err(format!("invalid literal at {}", self.pos))
        }
    }
    /// Strict RFC 8259 numbers: -?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?
    /// The lexeme is kept verbatim so request ids round-trip exactly.
    fn parse_number(&mut self) -> Result<J, String> {
        let start = self.pos;
        let take = |p: &mut Self, c: u8| -> bool {
            if p.peek() == Some(c) {
                p.pos += 1;
                true
            } else {
                false
            }
        };
        take(self, b'-');
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
            }
            Some(c) if c.is_ascii_digit() => {
                while self.peek().is_some_and(|d| d.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            _ => return Err(format!("invalid number at {start}")),
        }
        if take(self, b'.') {
            if !self.peek().is_some_and(|d| d.is_ascii_digit()) {
                return Err(format!("invalid number at {start}"));
            }
            while self.peek().is_some_and(|d| d.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if self.peek() == Some(b'e') || self.peek() == Some(b'E') {
            self.pos += 1;
            let _ = take(self, b'+') || take(self, b'-');
            if !self.peek().is_some_and(|d| d.is_ascii_digit()) {
                return Err(format!("invalid number at {start}"));
            }
            while self.peek().is_some_and(|d| d.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        Ok(J::Num(
            String::from_utf8_lossy(&self.b[start..self.pos]).into_owned(),
        ))
    }
    fn hex4(&mut self) -> Result<u32, String> {
        if self.pos + 4 > self.b.len() {
            return Err("truncated \\u escape".to_string());
        }
        let s = std::str::from_utf8(&self.b[self.pos..self.pos + 4])
            .map_err(|_| "\\u not utf8".to_string())?;
        let v = u32::from_str_radix(s, 16).map_err(|_| "bad \\u hex".to_string())?;
        self.pos += 4;
        Ok(v)
    }
    fn parse_string(&mut self) -> Result<String, String> {
        self.pos += 1; // open quote
        let mut out = String::new();
        loop {
            if self.pos >= self.b.len() {
                return Err("unterminated string".to_string());
            }
            let c = self.b[self.pos];
            match c {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos >= self.b.len() {
                        return Err("truncated escape".to_string());
                    }
                    let e = self.b[self.pos];
                    self.pos += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let cp = self.hex4()?;
                            let scalar = if (0xD800..=0xDBFF).contains(&cp) {
                                if self.b.get(self.pos..self.pos + 2) != Some(b"\\u") {
                                    return Err(
                                        "high surrogate must be followed by a low surrogate"
                                            .to_string(),
                                    );
                                }
                                self.pos += 2;
                                let low = self.hex4()?;
                                if !(0xDC00..=0xDFFF).contains(&low) {
                                    return Err(
                                        "high surrogate must be followed by a low surrogate"
                                            .to_string(),
                                    );
                                }
                                0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00)
                            } else if (0xDC00..=0xDFFF).contains(&cp) {
                                return Err("unexpected low surrogate".to_string());
                            } else {
                                cp
                            };
                            out.push(
                                char::from_u32(scalar)
                                    .ok_or_else(|| "invalid unicode scalar".to_string())?,
                            );
                        }
                        _ => return Err(format!("bad escape \\{}", e as char)),
                    }
                }
                _ => {
                    if c < 0x20 {
                        return Err(format!("unescaped control in string at {}", self.pos));
                    }
                    let rest = &self.b[self.pos..];
                    let s = std::str::from_utf8(rest)
                        .map_err(|_| "invalid utf8 in string".to_string())?;
                    let ch = s.chars().next().ok_or("empty string tail")?;
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }
    fn parse_array(&mut self) -> Result<J, String> {
        self.pos += 1; // [
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b']') {
                self.pos += 1;
                return Ok(J::Arr(items));
            }
            let v = self.parse_value()?;
            items.push(v);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                    if self.peek() == Some(b']') {
                        return Err(format!("trailing comma in array at {}", self.pos));
                    }
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(J::Arr(items));
                }
                _ => return Err(format!("expected ',' or ']' at {}", self.pos)),
            }
        }
    }
    fn parse_object(&mut self) -> Result<J, String> {
        self.pos += 1; // {
        let mut pairs = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b'}') {
                self.pos += 1;
                return Ok(J::Obj(pairs));
            }
            if self.peek() != Some(b'"') {
                return Err(format!("expected string key at {}", self.pos));
            }
            let k = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(format!("expected ':' at {}", self.pos));
            }
            self.pos += 1;
            let v = self.parse_value()?;
            pairs.push((k, v));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                    if self.peek() == Some(b'}') {
                        return Err(format!("trailing comma in object at {}", self.pos));
                    }
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(J::Obj(pairs));
                }
                _ => return Err(format!("expected ',' or '}}' at {}", self.pos)),
            }
        }
    }
}

pub fn parse_json(s: &str) -> Result<J, String> {
    let mut p = Parser::new(s);
    let v = p.parse_value()?;
    p.skip_ws();
    if p.pos != p.b.len() {
        return Err("trailing characters".to_string());
    }
    Ok(v)
}

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            '\u{0008}' => o.push_str("\\b"),
            '\u{000C}' => o.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                o.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub fn j_to_string(j: &J) -> String {
    match j {
        J::Null => "null".to_string(),
        J::Bool(true) => "true".to_string(),
        J::Bool(false) => "false".to_string(),
        J::Num(n) => n.clone(),
        J::Str(s) => esc(s),
        J::Arr(items) => {
            let parts: Vec<String> = items.iter().map(j_to_string).collect();
            format!("[{}]", parts.join(","))
        }
        J::Obj(pairs) => {
            let parts: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}:{}", esc(k), j_to_string(v)))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::{J, parse_json};

    /// Strict RFC 8259 numbers: leading zeros, bare fractions, and
    /// dangling exponents are rejected, not echoed back as ids.
    #[test]
    fn numbers_are_strict() {
        for bad in [
            "{\"id\":01}",
            "[1.2.3]",
            "[1e]",
            "[-]",
            "[01]",
            "[+1]",
            "[.5]",
            "[1.]",
            "[1e+]",
            "{\"a\":1} trailing",
        ] {
            assert!(parse_json(bad).is_err(), "must reject {bad}");
        }
        for good in [
            "[0]",
            "[-0]",
            "[42]",
            "[-0.5]",
            "[1e10]",
            "[1E-3]",
            "[123.456e+7]",
            "{\"jsonrpc\":\"2.0\",\"id\":3}",
        ] {
            assert!(parse_json(good).is_ok(), "must accept {good}");
        }
    }

    /// Trailing commas are a common emitter bug; Python and strict JSON
    /// reject them, and so must we (found by the Python differential corpus).
    #[test]
    fn trailing_commas_are_rejected() {
        for bad in ["[1,]", "[1,2,]", "{\"a\":1,}", "[[1,],2]", "{\"a\":[1,]},"] {
            assert!(parse_json(bad).is_err(), "must reject {bad}");
        }
    }

    /// Nesting is bounded so hostile frames cannot overflow the stack.
    #[test]
    fn nesting_is_bounded() {
        let deep = "[".repeat(100) + &"]".repeat(100);
        assert!(parse_json(&deep).is_err());
        let shallow = "[".repeat(8) + &"]".repeat(8);
        assert!(parse_json(&shallow).is_ok());
    }

    #[test]
    fn unicode_surrogate_pairs_are_decoded_strictly() {
        let parsed = parse_json(r#""\ud83d\ude00""#).expect("valid surrogate pair");
        assert!(matches!(parsed, J::Str(ref value) if value == "😀"));

        for bad in [
            r#""\ud83d""#,
            r#""\ude00""#,
            r#""\ud83d\u0041""#,
            r#""\ud83dx""#,
        ] {
            assert!(parse_json(bad).is_err(), "must reject {bad}");
        }
    }
}

#[cfg(test)]
mod property_tests {
    use super::{J, j_to_string, parse_json};

    /// Deterministic xorshift64*: reproducible fuzz without a runtime
    /// dependency or an external fuzzer in CI.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545F4914F6CDD1D)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }
    }

    fn random_string(rng: &mut Rng) -> String {
        let len = rng.below(24) as usize;
        let mut out = String::new();
        for _ in 0..len {
            match rng.below(8) {
                // Structure-sensitive ASCII.
                0 => out.push((b'"' + rng.below(6) as u8) as char),
                // Control characters must be escaped by the serializer.
                1 => out.push((rng.below(0x20) as u8) as char),
                2 => out.push('\\'),
                3 => out.push('\u{7f}'),
                // Multi-byte UTF-8 (all valid scalar values).
                4 => out.push(char::from_u32(0x80 + rng.below(0x7ff) as u32).unwrap()),
                // 0x800..=0xD7FF: stays below the surrogate block.
                5 => out.push(char::from_u32(0x800 + rng.below(0xd000) as u32).unwrap()),
                6 => out.push(char::from_u32(0x10000 + rng.below(0xffff) as u32).unwrap()),
                _ => out.push((b'a' + rng.below(26) as u8) as char),
            }
        }
        out
    }

    fn random_value(rng: &mut Rng, depth: u64) -> J {
        match rng.below(if depth == 0 { 4 } else { 6 }) {
            0 => J::Null,
            1 => J::Bool(rng.below(2) == 1),
            2 => J::Num(match rng.below(5) {
                0 => "0".to_string(),
                1 => format!("-{}", rng.below(1_000_000)),
                2 => format!("{}.{}", rng.below(1000), rng.below(1000)),
                3 => format!("{}e-{}", rng.below(1000), rng.below(30)),
                _ => format!("-{}.{}e+{}", rng.below(100), rng.below(100), rng.below(9)),
            }),
            3 => J::Str(random_string(rng)),
            4 => J::Arr(
                (0..rng.below(5))
                    .map(|_| random_value(rng, depth - 1))
                    .collect(),
            ),
            _ => J::Obj(
                (0..rng.below(5))
                    .map(|_| (random_string(rng), random_value(rng, depth - 1)))
                    .collect(),
            ),
        }
    }

    /// Serialize → parse → serialize must be an identity for every generated
    /// value: no panic, no drift, no accepted-but-mangled payload.
    #[test]
    fn random_values_round_trip_identically() {
        let mut rng = Rng(0x9E3779B97F4A7C15);
        for _ in 0..20_000 {
            let value = random_value(&mut rng, 5);
            let once = j_to_string(&value);
            let parsed =
                parse_json(&once).unwrap_or_else(|e| panic!("parse failed for {once}: {e}"));
            let twice = j_to_string(&parsed);
            assert_eq!(once, twice, "round trip drifted for {once}");
        }
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
