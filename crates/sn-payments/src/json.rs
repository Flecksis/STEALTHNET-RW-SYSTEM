use std::collections::HashSet;

const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Standard,
    Php,
}

pub(crate) struct Member<'a> {
    pub(crate) key: String,
    pub(crate) raw_key: &'a [u8],
    pub(crate) raw_value: &'a [u8],
}

impl Json {
    pub(crate) fn parse(bytes: &[u8]) -> Option<Json> {
        let mut parser = Parser { bytes, pos: 0 };
        parser.skip_ws();
        let value = parser.value(0)?;
        parser.skip_ws();
        (parser.pos == bytes.len()).then_some(value)
    }

    pub(crate) fn object<K: Into<String>>(pairs: impl IntoIterator<Item = (K, Json)>) -> Json {
        Json::Object(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    pub(crate) fn number(raw: impl Into<String>) -> Json {
        Json::Number(raw.into())
    }

    pub(crate) fn push(&mut self, key: &str, value: Json) {
        if let Json::Object(members) = self {
            members.retain(|(k, _)| k != key);
            members.push((key.to_string(), value));
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub(crate) fn at(&self, path: &[&str]) -> Option<&Json> {
        path.iter().try_fold(self, |value, key| value.get(key))
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub(crate) fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub(crate) fn scalar_text(&self) -> Option<&str> {
        match self {
            Json::String(s) | Json::Number(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        self.write(&mut out, Style::Standard);
        out.into_bytes()
    }

    pub(crate) fn to_php(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, Style::Php);
        out
    }

    fn write(&self, out: &mut String, style: Style) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Number(raw) => out.push_str(raw),
            Json::String(s) => write_string(out, s, style),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out, style);
                }
                out.push(']');
            }
            Json::Object(members) if members.is_empty() && style == Style::Php => {
                out.push_str("[]")
            }
            Json::Object(members) => {
                out.push('{');
                for (i, (key, value)) in members.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(out, key, style);
                    out.push(':');
                    value.write(out, style);
                }
                out.push('}');
            }
        }
    }
}

impl From<&str> for Json {
    fn from(value: &str) -> Self {
        Json::String(value.to_string())
    }
}

impl From<String> for Json {
    fn from(value: String) -> Self {
        Json::String(value)
    }
}

impl From<bool> for Json {
    fn from(value: bool) -> Self {
        Json::Bool(value)
    }
}

impl From<i64> for Json {
    fn from(value: i64) -> Self {
        Json::Number(value.to_string())
    }
}

fn write_string(out: &mut String, value: &str, style: Style) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' if style == Style::Php => out.push_str("\\/"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' | '\u{2029}' if style == Style::Php => {
                out.push_str(&format!("\\u{:04x}", u32::from(ch)))
            }
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
}

pub(crate) fn members(bytes: &[u8]) -> Option<Vec<Member<'_>>> {
    let mut parser = Parser { bytes, pos: 0 };
    parser.skip_ws();
    parser.eat(b'{')?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    parser.skip_ws();
    if parser.eat(b'}').is_none() {
        loop {
            parser.skip_ws();
            let key_start = parser.pos;
            let key = parser.string()?;
            let key_end = parser.pos;
            if !seen.insert(key.clone()) {
                return None;
            }
            parser.skip_ws();
            parser.eat(b':')?;
            parser.skip_ws();
            let value_start = parser.pos;
            parser.value(1)?;
            let value_end = parser.pos;
            out.push(Member {
                key,
                raw_key: bytes.get(key_start..key_end)?,
                raw_value: bytes.get(value_start..value_end)?,
            });
            parser.skip_ws();
            match parser.peek()? {
                b',' => parser.pos += 1,
                b'}' => {
                    parser.pos += 1;
                    break;
                }
                _ => return None,
            }
        }
    }
    parser.skip_ws();
    (parser.pos == bytes.len()).then_some(out)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> Option<()> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Some(())
        } else {
            None
        }
    }

    fn literal(&mut self, word: &[u8]) -> Option<()> {
        if self.bytes.get(self.pos..self.pos + word.len()) == Some(word) {
            self.pos += word.len();
            Some(())
        } else {
            None
        }
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        match self.peek()? {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => self.string().map(Json::String),
            b't' => self.literal(b"true").map(|_| Json::Bool(true)),
            b'f' => self.literal(b"false").map(|_| Json::Bool(false)),
            b'n' => self.literal(b"null").map(|_| Json::Null),
            b'-' | b'0'..=b'9' => self.number().map(Json::Number),
            _ => None,
        }
    }

    fn object(&mut self, depth: usize) -> Option<Json> {
        self.eat(b'{')?;
        let mut members = Vec::new();
        let mut seen = HashSet::new();
        self.skip_ws();
        if self.eat(b'}').is_some() {
            return Some(Json::Object(members));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            if !seen.insert(key.clone()) {
                return None;
            }
            self.skip_ws();
            self.eat(b':')?;
            self.skip_ws();
            let value = self.value(depth + 1)?;
            members.push((key, value));
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    return Some(Json::Object(members));
                }
                _ => return None,
            }
        }
    }

    fn array(&mut self, depth: usize) -> Option<Json> {
        self.eat(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.eat(b']').is_some() {
            return Some(Json::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value(depth + 1)?);
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b']' => {
                    self.pos += 1;
                    return Some(Json::Array(items));
                }
                _ => return None,
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        self.eat(b'"')?;
        let mut out = String::new();
        loop {
            let start = self.pos;
            while let Some(byte) = self.peek() {
                if byte == b'"' || byte == b'\\' || byte < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            out.push_str(std::str::from_utf8(self.bytes.get(start..self.pos)?).ok()?);
            match self.peek()? {
                b'"' => {
                    self.pos += 1;
                    return Some(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let escape = self.peek()?;
                    self.pos += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode()?),
                        _ => return None,
                    }
                }
                _ => return None,
            }
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = self.bytes.get(self.pos..self.pos + 4)?;
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let value = u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()?;
        self.pos += 4;
        Some(value)
    }

    fn unicode(&mut self) -> Option<char> {
        let first = self.hex4()?;
        if (0xD800..0xDC00).contains(&first) {
            self.literal(b"\\u")?;
            let second = self.hex4()?;
            if !(0xDC00..0xE000).contains(&second) {
                return None;
            }
            char::from_u32(0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00))
        } else {
            char::from_u32(first)
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        self.pos - start
    }

    fn number(&mut self) -> Option<String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek()? {
            b'0' => self.pos += 1,
            b'1'..=b'9' => {
                self.digits();
            }
            _ => return None,
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if self.digits() == 0 {
                return None;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if self.digits() == 0 {
                return None;
            }
        }
        std::str::from_utf8(self.bytes.get(start..self.pos)?)
            .ok()
            .map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn числа_остаются_исходным_текстом() {
        let value =
            Json::parse(br#"{"a": 19.99, "b": 1200, "c": 0.1, "d": -5, "e": 1e3}"#).unwrap();
        assert_eq!(value.get("a"), Some(&Json::number("19.99")));
        assert_eq!(value.get("b").and_then(Json::scalar_text), Some("1200"));
        assert_eq!(value.get("c").and_then(Json::scalar_text), Some("0.1"));
        assert_eq!(value.get("d").and_then(Json::scalar_text), Some("-5"));
        assert_eq!(value.get("e").and_then(Json::scalar_text), Some("1e3"));
    }

    #[test]
    fn строгий_разбор_отклоняет_мусор() {
        for raw in [
            &b""[..],
            b"{",
            b"{\"a\":1,}",
            b"[1,]",
            b"{\"a\":01}",
            b"{\"a\":1.}",
            b"{\"a\":.5}",
            b"{\"a\":+1}",
            b"{\"a\":NaN}",
            b"{\"a\":\"\x01\"}",
            b"{\"a\":\"\\x\"}",
            b"{\"a\":\"\\ud800\"}",
            b"{\"a\":\"\\u+041\"}",
            b"{\"a\":1}{",
            b"{\"a\":1,\"a\":2}",
            b"{'a':1}",
        ] {
            assert_eq!(Json::parse(raw), None, "{}", String::from_utf8_lossy(raw));
        }
    }

    #[test]
    fn глубокая_вложенность_не_роняет_разбор() {
        let deep = "[".repeat(10_000) + &"]".repeat(10_000);
        assert_eq!(Json::parse(deep.as_bytes()), None);
    }

    #[test]
    fn строки_с_экранированием_и_юникодом() {
        let value =
            Json::parse(r#"{"s":"a\/b \"q\" \u0410\u044f \ud83d\ude00 \n"}"#.as_bytes()).unwrap();
        assert_eq!(
            value.get("s").and_then(Json::as_str),
            Some("a/b \"q\" Ая 😀 \n")
        );
    }

    #[test]
    fn запись_в_стиле_php_экранирует_слэши_и_оставляет_юникод() {
        let value = Json::object([
            ("url", Json::from("https://a.b/c")),
            ("name", Json::from("Тест")),
            ("ctl", Json::from("\u{1f}")),
            ("empty", Json::Object(Vec::new())),
            ("n", Json::number("3.00")),
        ]);
        assert_eq!(
            value.to_php(),
            r#"{"url":"https:\/\/a.b\/c","name":"Тест","ctl":"\u001f","empty":[],"n":3.00}"#
        );
        assert_eq!(
            String::from_utf8(value.to_bytes()).unwrap(),
            r#"{"url":"https://a.b/c","name":"Тест","ctl":"\u001f","empty":{},"n":3.00}"#
        );
    }

    #[test]
    fn члены_верхнего_уровня_с_исходными_байтами() {
        let raw = br#"{"a":"x\/y", "b" : {"c":[1, 2]},"sign":"ff"}"#;
        let members = members(raw).unwrap();
        let keys: Vec<&str> = members.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(keys, ["a", "b", "sign"]);
        assert_eq!(members[0].raw_value, br#""x\/y""#);
        assert_eq!(members[1].raw_key, br#""b""#);
        assert_eq!(members[1].raw_value, br#"{"c":[1, 2]}"#);
        assert!(super::members(br#"{"a":1,"a":2}"#).is_none());
        assert!(super::members(b"[1]").is_none());
    }
}
