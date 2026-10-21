// SPDX-License-Identifier: FSL-1.1-ALv2
//! Strict JSON tree that keeps number lexemes exact.
//!
//! Caller schemas and model output must be compared without floating point: an enum value such as
//! `1.10` equals `1.1`, but `0.1000000000000000055511151231257827` is not `0.1`. The tree therefore
//! stores every number as its source lexeme and [`crate::decimal::Exact`] compares them. The parser
//! rejects duplicate object keys, invalid escapes, lone surrogates, control characters, trailing
//! content and nesting beyond a caller-chosen limit, so hostile documents cannot exhaust the stack.
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// The validated source lexeme of the number.
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError;

impl Json {
    /// Parse a complete document with at most `max_depth` nested arrays or objects.
    pub fn parse(text: &str, max_depth: usize) -> Result<Json, ParseError> {
        let mut parser = Parser {
            bytes: text.as_bytes(),
            text,
            position: 0,
            max_depth,
        };
        parser.skip_whitespace();
        let value = parser.value(0)?;
        parser.skip_whitespace();
        if parser.position != parser.bytes.len() {
            return Err(ParseError);
        }
        Ok(value)
    }

    pub fn as_object(&self) -> Option<&BTreeMap<String, Json>> {
        match self {
            Json::Object(map) => Some(map),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(text) => Some(text),
            _ => None,
        }
    }

    /// Compact serialization; numbers are written as their exact lexemes and object keys in
    /// sorted order.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Number(lexeme) => out.push_str(lexeme),
            Json::String(text) => write_string(text, out),
            Json::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Object(map) => {
                out.push('{');
                for (index, (key, value)) in map.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_string(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// Write a JSON string literal using serde_json's escaping (valid for any `&str`).
pub fn write_string(text: &str, out: &mut String) {
    out.push_str(&serde_json::to_string(text).expect("a string always serializes"));
}

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    position: usize,
    max_depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }
    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }
    fn expect(&mut self, literal: &str) -> Result<(), ParseError> {
        if self.bytes[self.position..].starts_with(literal.as_bytes()) {
            self.position += literal.len();
            Ok(())
        } else {
            Err(ParseError)
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, ParseError> {
        match self.peek().ok_or(ParseError)? {
            b'n' => self.expect("null").map(|()| Json::Null),
            b't' => self.expect("true").map(|()| Json::Bool(true)),
            b'f' => self.expect("false").map(|()| Json::Bool(false)),
            b'"' => self.string().map(Json::String),
            b'[' => self.array(depth),
            b'{' => self.object(depth),
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err(ParseError),
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, ParseError> {
        if depth >= self.max_depth {
            return Err(ParseError);
        }
        self.position += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(Json::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.value(depth + 1)?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b']') => {
                    self.position += 1;
                    return Ok(Json::Array(items));
                }
                _ => return Err(ParseError),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, ParseError> {
        if depth >= self.max_depth {
            return Err(ParseError);
        }
        self.position += 1;
        let mut map = BTreeMap::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.position += 1;
            return Ok(Json::Object(map));
        }
        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return Err(ParseError);
            }
            let key = self.string()?;
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return Err(ParseError);
            }
            self.position += 1;
            self.skip_whitespace();
            let value = self.value(depth + 1)?;
            // A duplicate key makes the document ambiguous; reject it instead of picking a winner.
            if map.insert(key, value).is_some() {
                return Err(ParseError);
            }
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b'}') => {
                    self.position += 1;
                    return Ok(Json::Object(map));
                }
                _ => return Err(ParseError),
            }
        }
    }

    fn number(&mut self) -> Result<Json, ParseError> {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        match self.peek() {
            Some(b'0') => self.position += 1,
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(ParseError),
        }
        if self.peek() == Some(b'.') {
            self.position += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(ParseError);
            }
            self.digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(ParseError);
            }
            self.digits();
        }
        Ok(Json::Number(self.text[start..self.position].to_owned()))
    }

    fn digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.position += 1;
        }
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let slice = self
            .bytes
            .get(self.position..self.position + 4)
            .ok_or(ParseError)?;
        let mut value = 0_u32;
        for byte in slice {
            value = value * 16 + (*byte as char).to_digit(16).ok_or(ParseError)?;
        }
        self.position += 4;
        Ok(value)
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.position += 1;
        let mut out = String::new();
        loop {
            let byte = self.peek().ok_or(ParseError)?;
            match byte {
                b'"' => {
                    self.position += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.position += 1;
                    let escape = self.peek().ok_or(ParseError)?;
                    self.position += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&first) {
                                if !self.bytes[self.position..].starts_with(b"\\u") {
                                    return Err(ParseError);
                                }
                                self.position += 2;
                                let second = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&second) {
                                    return Err(ParseError);
                                }
                                0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&first) {
                                return Err(ParseError);
                            } else {
                                first
                            };
                            out.push(char::from_u32(code).ok_or(ParseError)?);
                        }
                        _ => return Err(ParseError),
                    }
                }
                0..=0x1f => return Err(ParseError),
                _ => {
                    // Copy one UTF-8 scalar; the input is a valid `&str`.
                    let rest = &self.text[self.position..];
                    let ch = rest.chars().next().ok_or(ParseError)?;
                    out.push(ch);
                    self.position += ch.len_utf8();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalars_and_keeps_number_lexemes() {
        let value = Json::parse(
            r#"{"a":[1.10,-0,1e400,"x\u00e9\ud83d\ude00",true,null]}"#,
            8,
        )
        .expect("valid");
        assert_eq!(
            value.serialize(),
            "{\"a\":[1.10,-0,1e400,\"x\u{e9}\u{1f600}\",true,null]}"
        );
    }

    #[test]
    fn rejects_malformed_documents() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "{\"a\":1}x",
            "01",
            "1.",
            ".5",
            "+1",
            "1e",
            "NaN",
            "'a'",
            "\"\\x\"",
            "\"\\ud800\"",
            "\"\\udc00\"",
            "\"a\nb\"",
            "{\"a\":1,\"a\":2}",
            "[1 2]",
        ] {
            assert!(Json::parse(bad, 8).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn nesting_is_bounded() {
        let deep = format!("{}1{}", "[".repeat(9), "]".repeat(9));
        assert!(Json::parse(&deep, 8).is_err());
        assert!(Json::parse(&deep, 9).is_ok());
        let huge = format!("{}1{}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(Json::parse(&huge, 64).is_err());
    }
}
