//! A private JSON projection for plaintext keyfiles.
//!
//! Tokens without escapes borrow the caller's input. Escaped strings have one
//! guarded allocation, sized before copying, and numbers never allocate. The
//! projection retains object members and scalar strings/numbers; array contents
//! and other scalars are validated but have no meaning to the keyfile codec.

use std::{
    borrow::Borrow,
    cmp::Ordering,
    collections::{BTreeMap, btree_map::Entry},
    fmt,
};
use zeroize::{Zeroize, Zeroizing};

pub(super) const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
// serde_json's default limit permits 127 enclosing arrays/objects and refuses
// the 128th. Keep that boundary even if another dependency enables raw_value.
const MAX_CONTAINERS: u8 = 127;

#[derive(Clone, Copy, Debug)]
pub(super) struct ParseError;

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid or oversized keyfile JSON")
    }
}
impl std::error::Error for ParseError {}

struct OwnedText {
    text: Zeroizing<String>,
    #[cfg(test)]
    allocation: (usize, usize),
}

impl OwnedText {
    fn with_capacity(capacity: usize) -> Self {
        let text = Zeroizing::new(String::with_capacity(capacity));
        Self {
            #[cfg(test)]
            allocation: (text.as_ptr() as usize, text.capacity()),
            text,
        }
    }
}

impl Drop for OwnedText {
    fn drop(&mut self) {
        // Wipe initialized bytes while their length still permits safe
        // observation in the drop regression. Zeroizing then wipes the full
        // capacity before releasing it. No freed memory is inspected.
        self.text.as_mut_str().zeroize();
        #[cfg(test)]
        DROP_AUDIT.with(|audit| {
            if let Some(audit) = audit.borrow_mut().as_mut() {
                audit.drops += 1;
                audit.bytes += self.text.len();
                audit.wiped &= self.text.as_bytes().iter().all(|byte| *byte == 0);
                audit.stable &=
                    self.allocation == (self.text.as_ptr() as usize, self.text.capacity());
            }
        });
    }
}

enum TextInner<'a> {
    Borrowed(&'a str),
    Owned(OwnedText),
}

pub(super) struct Text<'a>(TextInner<'a>);

impl Text<'_> {
    pub(super) fn as_str(&self) -> &str {
        match &self.0 {
            TextInner::Borrowed(text) => text,
            TextInner::Owned(text) => &text.text,
        }
    }
}
impl Borrow<str> for Text<'_> {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}
impl PartialEq for Text<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}
impl Eq for Text<'_> {}
impl PartialOrd for Text<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Text<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

pub(super) struct Object<'a> {
    // The ordinal retains first-insertion order when a duplicate replaces its
    // value. The tree keeps duplicate handling logarithmic for large keyfiles.
    entries: BTreeMap<Text<'a>, (usize, Value<'a>)>,
}

impl<'input> Object<'input> {
    pub(super) fn keys(&self) -> impl Iterator<Item = &Text<'input>> {
        let mut keys: Vec<_> = self.entries.iter().collect();
        keys.sort_unstable_by_key(|(_, (ordinal, _))| *ordinal);
        keys.into_iter().map(|(key, _)| key)
    }
}

pub(super) enum Value<'a> {
    String(Text<'a>),
    Number(&'a str),
    Object(Object<'a>),
    Other,
}

impl<'input> Value<'input> {
    pub(super) fn get(&self, key: &str) -> Option<&Self> {
        self.as_object()?.entries.get(key).map(|(_, value)| value)
    }

    pub(super) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text.as_str()),
            _ => None,
        }
    }

    pub(super) fn as_object(&self) -> Option<&Object<'input>> {
        match self {
            Self::Object(object) => Some(object),
            _ => None,
        }
    }
}

pub(super) fn parse<'a, G>(
    input: &'a str,
    guard: impl FnOnce(Value<'a>) -> G,
) -> Result<G, ParseError> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(ParseError);
    }
    let mut parser = Parser { input, position: 0 };
    let value = guard(parser.value(0)?);
    parser.whitespace();
    if parser.position != input.len() {
        return Err(ParseError);
    }
    Ok(value)
}

struct Parser<'a> {
    input: &'a str,
    position: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.position).copied()
    }

    fn next(&mut self) -> Result<u8, ParseError> {
        let byte = self.peek().ok_or(ParseError)?;
        self.position += 1;
        Ok(byte)
    }

    fn expect(&mut self, expected: u8) -> Result<(), ParseError> {
        if self.next()? != expected {
            return Err(ParseError);
        }
        Ok(())
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.position += 1;
        }
    }

    fn value(&mut self, depth: u8) -> Result<Value<'a>, ParseError> {
        self.whitespace();
        match self.peek().ok_or(ParseError)? {
            b'"' => self.string().map(Value::String),
            b'-' | b'0'..=b'9' => self.number().map(Value::Number),
            b'{' | b'[' if depth >= MAX_CONTAINERS => Err(ParseError),
            b'{' => self.object(depth + 1).map(Value::Object),
            b'[' => {
                self.array(depth + 1)?;
                Ok(Value::Other)
            }
            b'n' => self.literal(b"null"),
            b't' => self.literal(b"true"),
            b'f' => self.literal(b"false"),
            _ => Err(ParseError),
        }
    }

    fn literal(&mut self, token: &[u8]) -> Result<Value<'a>, ParseError> {
        for byte in token {
            self.expect(*byte)?;
        }
        Ok(Value::Other)
    }

    fn object(&mut self, depth: u8) -> Result<Object<'a>, ParseError> {
        self.expect(b'{')?;
        let mut entries = BTreeMap::new();
        self.whitespace();
        if self.peek() == Some(b'}') {
            self.position += 1;
            return Ok(Object { entries });
        }
        loop {
            let key = self.string()?;
            self.whitespace();
            self.expect(b':')?;
            let value = self.value(depth)?;
            let ordinal = entries.len();
            match entries.entry(key) {
                Entry::Occupied(mut entry) => entry.get_mut().1 = value,
                Entry::Vacant(entry) => {
                    entry.insert((ordinal, value));
                }
            }
            self.whitespace();
            match self.next()? {
                b'}' => return Ok(Object { entries }),
                b',' => self.whitespace(),
                _ => return Err(ParseError),
            }
        }
    }

    fn array(&mut self, depth: u8) -> Result<(), ParseError> {
        self.expect(b'[')?;
        self.whitespace();
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(());
        }
        loop {
            // Array elements cannot supply a keyfile field. Validate and drop
            // each guarded element immediately instead of retaining its data.
            drop(self.value(depth)?);
            self.whitespace();
            match self.next()? {
                b']' => return Ok(()),
                b',' => {}
                _ => return Err(ParseError),
            }
        }
    }

    fn number(&mut self) -> Result<&'a str, ParseError> {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        match self.next()? {
            b'0' => {}
            b'1'..=b'9' => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.position += 1;
                }
            }
            _ => return Err(ParseError),
        }
        if self.peek() == Some(b'.') {
            self.position += 1;
            self.digits()?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            self.digits()?;
        }
        Ok(&self.input[start..self.position])
    }

    fn digits(&mut self) -> Result<(), ParseError> {
        if !matches!(self.next()?, b'0'..=b'9') {
            return Err(ParseError);
        }
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.position += 1;
        }
        Ok(())
    }

    fn string(&mut self) -> Result<Text<'a>, ParseError> {
        self.expect(b'"')?;
        let start = self.position;
        let mut escaped = false;
        let end = loop {
            match self.next()? {
                b'"' => break self.position - 1,
                b'\\' => {
                    escaped = true;
                    self.escape()?;
                }
                0..=0x1f => return Err(ParseError),
                _ => {}
            }
        };
        let raw = &self.input[start..end];
        if !escaped {
            return Ok(Text(TextInner::Borrowed(raw)));
        }
        // A decoded JSON string is never longer than its encoded contents.
        // Validate once before allocation, then decode into this one guarded
        // buffer. No reallocations can release earlier plaintext copies.
        let mut decoded = OwnedText::with_capacity(raw.len());
        let mut parser = Parser {
            input: raw,
            position: 0,
        };
        let mut segment = 0;
        while let Some(byte) = parser.peek() {
            if byte == b'\\' {
                decoded.text.push_str(&raw[segment..parser.position]);
                parser.position += 1;
                decoded.text.push(parser.escape()?);
                segment = parser.position;
            } else {
                parser.position += 1;
            }
        }
        decoded.text.push_str(&raw[segment..]);
        Ok(Text(TextInner::Owned(decoded)))
    }

    fn escape(&mut self) -> Result<char, ParseError> {
        match self.next()? {
            b'"' => Ok('"'),
            b'\\' => Ok('\\'),
            b'/' => Ok('/'),
            b'b' => Ok('\u{8}'),
            b'f' => Ok('\u{c}'),
            b'n' => Ok('\n'),
            b'r' => Ok('\r'),
            b't' => Ok('\t'),
            b'u' => {
                let first = self.hex_quad()?;
                let scalar = match first {
                    0xd800..=0xdbff => {
                        self.expect(b'\\')?;
                        self.expect(b'u')?;
                        let second = self.hex_quad()?;
                        if !(0xdc00..=0xdfff).contains(&second) {
                            return Err(ParseError);
                        }
                        0x10000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00)
                    }
                    0xdc00..=0xdfff => return Err(ParseError),
                    _ => u32::from(first),
                };
                char::from_u32(scalar).ok_or(ParseError)
            }
            _ => Err(ParseError),
        }
    }

    fn hex_quad(&mut self) -> Result<u16, ParseError> {
        let mut value = 0;
        for _ in 0..4 {
            let digit = match self.next()? {
                byte @ b'0'..=b'9' => byte - b'0',
                byte @ b'a'..=b'f' => byte - b'a' + 10,
                byte @ b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(ParseError),
            };
            value = (value << 4) | u16::from(digit);
        }
        Ok(value)
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(super) struct DropAudit {
    pub(super) drops: usize,
    pub(super) bytes: usize,
    pub(super) wiped: bool,
    pub(super) stable: bool,
}

#[cfg(test)]
thread_local! {
    static DROP_AUDIT: std::cell::RefCell<Option<DropAudit>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn audit_drops<T>(run: impl FnOnce() -> T) -> (T, DropAudit) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            DROP_AUDIT.with(|audit| *audit.borrow_mut() = None);
        }
    }
    DROP_AUDIT.with(|audit| {
        *audit.borrow_mut() = Some(DropAudit {
            drops: 0,
            bytes: 0,
            wiped: true,
            stable: true,
        });
    });
    let reset = Reset;
    let result = run();
    let audit = DROP_AUDIT
        .with(|audit| audit.borrow_mut().take())
        .unwrap_or(DropAudit {
            drops: 0,
            bytes: 0,
            wiped: false,
            stable: false,
        });
    drop(reset);
    (result, audit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaped_allocations_wipe_in_place_on_success_duplicate_partial_and_trailing_paths() {
        for (input, expected_drops, success) in [
            (r#"{"secretPhrase":"public\u0020first"}"#, 1, true),
            (
                r#"{"secretPhrase":"public\u0020first","secretPhrase":"public\u0020last"}"#,
                2,
                true,
            ),
            (
                r#"{"secretPhrase":"public\u0020first","nested":["public\u0020second", "unfinished"#,
                2,
                false,
            ),
            (
                r#"{"secretPhrase":"public\u0020first","next":"unfinished\u0020"#,
                1,
                false,
            ),
            (r#"{"secretPhrase":"public\u0020first"} trailing"#, 1, false),
            (
                r#"{"public\u0020key":"public\u0020first","public key":"public\u0020last"}"#,
                3,
                true,
            ),
            (
                r#"{"public\u0020key":"public\u0020first","public\u0020key":"public\u0020last"}"#,
                4,
                true,
            ),
        ] {
            let (parsed, audit) = audit_drops(|| parse(input, |value| value).map(drop));
            assert_eq!(parsed.is_ok(), success, "{input}");
            assert_eq!(audit.drops, expected_drops, "{input}");
            assert!(audit.bytes > 0 && audit.wiped && audit.stable, "{input}");
        }
    }

    #[test]
    fn unescaped_strings_and_arbitrary_precision_numbers_borrow_without_secret_allocations()
    -> Result<(), ParseError> {
        let input = r#"{"secretPhrase":"public disposable phrase","number":123456789012345678901234567890,"unknown":[-0,1.20E+99999999999999999999999999999999999]}"#;
        let (parsed, audit) = audit_drops(|| {
            let value = parse(input, |value| value)?;
            let phrase = value
                .get("secretPhrase")
                .and_then(Value::as_str)
                .ok_or(ParseError)?;
            assert!(phrase.as_ptr() >= input.as_ptr());
            assert_eq!(phrase, "public disposable phrase");
            match value.get("number") {
                Some(Value::Number(number)) => {
                    assert_eq!(*number, "123456789012345678901234567890")
                }
                _ => return Err(ParseError),
            }
            Ok::<_, ParseError>(())
        });
        parsed?;
        assert_eq!(audit.drops, 0);
        Ok(())
    }

    #[test]
    fn unicode_and_duplicate_keys_match_serde_json_for_public_fixture_strings()
    -> Result<(), Box<dyn std::error::Error>> {
        for input in [
            r#"{"value":"\"\\\/\b\f\n\r\t\u0000\u007f\u07ff\u0800\ud7ff\ue000\uffff\ud800\udc00\udbff\udfff"}"#,
            r#"{"value":"é 中文 🦀","\u0076alue":"last\u0020value"}"#,
            r#"{"value":"first","other":{"value":"nested"},"value":"last"}"#,
            r#"{"value":null,"value":[],"value":123456789012345678901234567890}"#,
        ] {
            let expected: serde_json::Value = serde_json::from_str(input)?;
            let actual = parse(input, |value| value)?;
            assert_eq!(
                actual.get("value").and_then(Value::as_str),
                expected.get("value").and_then(serde_json::Value::as_str)
            );
            assert_eq!(
                actual.as_object().map(|object| object.keys().count()),
                expected.as_object().map(serde_json::Map::len)
            );
        }
        Ok(())
    }

    #[test]
    fn syntax_unicode_and_depth_acceptance_match_serde_json() {
        for input in [
            "",
            " ",
            "[",
            "{",
            "{\"a\":1,}",
            "[1,]",
            "{\"a\" 1}",
            "{1:2}",
            "[truefalse]",
            "[null null]",
            "[01]",
            "[-01]",
            "[1.]",
            "[.1]",
            "[1e]",
            "[1e+]",
            "[--1]",
            "true trailing",
            "\u{feff}{}",
            "[NaN]",
            "[Infinity]",
            "{\"a\":\"line\nfeed\"}",
            r#"{"a":"\x00"}"#,
            r#"{"a":"\u12"}"#,
            r#"{"a":"\udc00"}"#,
            r#"{"a":"\ud800"}"#,
            r#"{"a":"\ud800\u0041"}"#,
            r#"{"a":"\ud800\ud800"}"#,
            "{}",
            " []\r\n",
            "[0,-0,1e+2,1E-2,0.0,1e99999999999999999999999999999]",
            "true",
            "false",
            "null",
        ] {
            assert_eq!(
                parse(input, |value| value).is_ok(),
                serde_json::from_str::<serde_json::Value>(input).is_ok(),
                "{input}"
            );
        }
        for depth in [126, 127, 128, 129] {
            let input = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
            assert_eq!(
                parse(&input, |value| value).is_ok(),
                serde_json::from_str::<serde_json::Value>(&input).is_ok(),
                "depth {depth}"
            );
        }
        let oversized = " ".repeat(MAX_INPUT_BYTES + 1);
        assert!(parse(&oversized, |value| value).is_err());
    }

    #[test]
    fn every_utf8_prefix_of_nested_public_json_matches_dependency_acceptance() {
        for input in [
            r#"{"secretPhrase":"public\u0020phrase","escaped\u0020key":{"nested":["é 中文 🦀",true,false,null,{"pair":"\ud83e\udd80"}]},"number":-123.45E+678,"secretPhrase":"last\u0020phrase"}"#,
            r#"[{},[],[0,-0,1e+0,123456789012345678901234567890],"\"\\\/\b\f\n\r\t",{"empty":""}]"#,
        ] {
            for end in (0..=input.len()).filter(|end| input.is_char_boundary(*end)) {
                let prefix = &input[..end];
                assert_eq!(
                    parse(prefix, |value| value).is_ok(),
                    serde_json::from_str::<serde_json::Value>(prefix).is_ok(),
                    "prefix {end} of {input}"
                );
            }
        }
    }
}
