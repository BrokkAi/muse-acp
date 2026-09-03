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
                            out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
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
    use super::parse_json;

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

    /// Nesting is bounded so hostile frames cannot overflow the stack.
    #[test]
    fn nesting_is_bounded() {
        let deep = "[".repeat(100) + &"]".repeat(100);
        assert!(parse_json(&deep).is_err());
        let shallow = "[".repeat(8) + &"]".repeat(8);
        assert!(parse_json(&shallow).is_ok());
    }
}
