//! Canonical StringTagVisitor representation used by NbtPredicate.CODEC.
use super::Tag;
use crate::protocol::Result;
use std::fmt::Write;

pub fn encode(value: &Tag) -> Result<String> {
    let mut out = String::new();
    append(value, &mut out)?;
    Ok(out)
}
fn quote(value: &str, out: &mut String) {
    let delimiter = value
        .chars()
        .find(|c| *c == '\'' || *c == '"')
        .map(|c| if c == '"' { '\'' } else { '"' })
        .unwrap_or('"');
    out.push(delimiter);
    for ch in value.chars() {
        match ch {
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            ch if ch < '\u{20}' => write!(out, "\\x{:02X}", ch as u32).unwrap(),
            ch => {
                if ch == '\\' || ch == delimiter {
                    out.push('\\');
                }
                out.push(ch);
            }
        }
    }
    out.push(delimiter);
}
fn key(value: &str, out: &mut String) {
    let mut chars = value.chars();
    let unquoted = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '.')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-'))
        && value != "true"
        && value != "false";
    if unquoted {
        out.push_str(value);
    } else {
        quote(value, out);
    }
}
fn floating(raw: String, number: f64) -> String {
    if !number.is_finite() {
        return raw;
    }
    let negative = raw.starts_with('-');
    let unsigned = raw.trim_start_matches('-');
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map(|(m, e)| (m, e.parse::<i32>().unwrap()))
        .unwrap_or((unsigned, 0));
    let decimal = mantissa.find('.').unwrap_or(mantissa.len()) as i32;
    let mut digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let leading = digits.bytes().take_while(|c| *c == b'0').count();
    if leading == digits.len() {
        return if negative { "-0.0" } else { "0.0" }.into();
    }
    digits.drain(..leading);
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    let power = exponent + decimal - leading as i32 - 1;
    let mut result = if negative {
        "-".to_owned()
    } else {
        String::new()
    };
    if !(-3..7).contains(&power) {
        result.push(digits.as_bytes()[0] as char);
        result.push('.');
        if digits.len() == 1 {
            result.push('0');
        } else {
            result.push_str(&digits[1..]);
        }
        write!(result, "E{power}").unwrap();
    } else if power < 0 {
        result.push_str("0.");
        for _ in 0..(-power - 1) {
            result.push('0');
        }
        result.push_str(&digits);
    } else {
        let length = power as usize + 1;
        if digits.len() <= length {
            result.push_str(&digits);
            for _ in digits.len()..length {
                result.push('0');
            }
            result.push_str(".0");
        } else {
            result.push_str(&digits[..length]);
            result.push('.');
            result.push_str(&digits[length..]);
        }
    }
    result
}
fn append(value: &Tag, out: &mut String) -> Result<()> {
    match value {
        Tag::Empty => out.push_str("END"),
        Tag::Bool(n) => write!(out, "{}b", u8::from(*n)).unwrap(),
        Tag::Byte(n) => write!(out, "{n}b").unwrap(),
        Tag::Short(n) => write!(out, "{n}s").unwrap(),
        Tag::Int(n) => write!(out, "{n}").unwrap(),
        Tag::Long(n) => write!(out, "{n}L").unwrap(),
        Tag::Float(n) => {
            out.push_str(&floating(format!("{n:?}"), f64::from(*n)));
            out.push('f');
        }
        Tag::Double(n) => {
            out.push_str(&floating(format!("{n:?}"), *n));
            out.push('d');
        }
        Tag::String(s) => quote(s, out),
        Tag::Bytes(v) => {
            out.push_str("[B;");
            for (i, n) in v.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write!(out, "{}B", *n as i8).unwrap();
            }
            out.push(']');
        }
        Tag::Ints(v) => {
            out.push_str("[I;");
            for (i, n) in v.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write!(out, "{n}").unwrap();
            }
            out.push(']');
        }
        Tag::Longs(v) => {
            out.push_str("[L;");
            for (i, n) in v.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write!(out, "{n}L").unwrap();
            }
            out.push(']');
        }
        Tag::List(v) => {
            out.push('[');
            for (i, v) in v.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                append(v, out)?;
            }
            out.push(']');
        }
        Tag::Map(fields) => {
            let mut fields = fields
                .iter()
                .map(|(k, v)| match k {
                    Tag::String(k) => Ok((k, v)),
                    _ => Err("NBT compound key must be string".into()),
                })
                .collect::<Result<Vec<_>>>()?;
            fields.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, (k, v)) in fields.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                key(k, out);
                out.push(':');
                append(v, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

pub fn decode(text: &str) -> Result<Tag> {
    let mut parser = Parser { text, offset: 0 };
    let value = parser.value(0)?;
    parser.space();
    if parser.offset != text.len() {
        return Err("trailing SNBT data".into());
    }
    Ok(value)
}
struct Parser<'a> {
    text: &'a str,
    offset: usize,
}
impl Parser<'_> {
    fn space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.next();
        }
    }
    fn peek(&self) -> Option<char> {
        self.text[self.offset..].chars().next()
    }
    fn next(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.offset += ch.len_utf8();
        Some(ch)
    }
    fn take(&mut self, ch: char) -> bool {
        self.space();
        if self.peek() == Some(ch) {
            self.next();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, ch: char) -> Result<()> {
        if self.take(ch) {
            Ok(())
        } else {
            Err(format!("expected SNBT '{ch}'"))
        }
    }
    fn word(&mut self, is_key: bool) -> String {
        self.space();
        let start = self.offset;
        while self.peek().is_some_and(|c| {
            !c.is_whitespace() && !matches!(c, ',' | ']' | '}') && (!is_key || c != ':')
        }) {
            self.next();
        }
        self.text[start..self.offset].to_owned()
    }
    fn quoted(&mut self) -> Result<String> {
        let delimiter = self.next().ok_or("SNBT string")?;
        let mut result = String::new();
        loop {
            match self.next().ok_or("unterminated SNBT string")? {
                ch if ch == delimiter => return Ok(result),
                '\\' => match self.next().ok_or("SNBT escape")? {
                    '\\' => result.push('\\'),
                    '"' => result.push('"'),
                    '\'' => result.push('\''),
                    'n' => result.push('\n'),
                    'r' => result.push('\r'),
                    't' => result.push('\t'),
                    'b' => result.push('\u{8}'),
                    'f' => result.push('\u{c}'),
                    'x' => {
                        let mut number = 0u32;
                        for _ in 0..2 {
                            number = (number << 4)
                                | self
                                    .next()
                                    .and_then(|c| c.to_digit(16))
                                    .ok_or("SNBT hex escape")?;
                        }
                        result.push(char::from_u32(number).unwrap());
                    }
                    'u' => {
                        let mut number = 0u32;
                        for _ in 0..4 {
                            number = (number << 4)
                                | self
                                    .next()
                                    .and_then(|c| c.to_digit(16))
                                    .ok_or("SNBT unicode escape")?;
                        }
                        result.push(char::from_u32(number).ok_or("SNBT unpaired surrogate")?);
                    }
                    _ => return Err("invalid SNBT escape".into()),
                },
                ch => result.push(ch),
            }
        }
    }
    fn value(&mut self, depth: u8) -> Result<Tag> {
        if depth > 64 {
            return Err("SNBT depth bound".into());
        }
        self.space();
        match self.peek().ok_or("missing SNBT value")? {
            '"' | '\'' => Ok(Tag::string(self.quoted()?)),
            '{' => {
                self.next();
                let mut fields = Vec::new();
                if self.take('}') {
                    return Ok(Tag::Map(fields));
                }
                loop {
                    self.space();
                    let key = if matches!(self.peek(), Some('"' | '\'')) {
                        self.quoted()?
                    } else {
                        let key = self.word(true);
                        if key.is_empty() {
                            return Err("empty unquoted SNBT key".into());
                        }
                        key
                    };
                    self.expect(':')?;
                    let value = self.value(depth + 1)?;
                    if let Some((_, old)) = fields.iter_mut().find(|(k, _)| k == &Tag::string(&key))
                    {
                        *old = value;
                    } else {
                        fields.push((Tag::string(key), value));
                    }
                    if self.take('}') {
                        break;
                    }
                    self.expect(',')?;
                }
                Ok(Tag::Map(fields))
            }
            '[' => {
                self.next();
                self.space();
                let start = self.offset;
                let array = self
                    .next()
                    .filter(|c| matches!(c, 'B' | 'I' | 'L'))
                    .filter(|_| self.take(';'));
                if array.is_none() {
                    self.offset = start;
                }
                let mut values = Vec::new();
                if !self.take(']') {
                    loop {
                        values.push(self.value(depth + 1)?);
                        if self.take(']') {
                            break;
                        }
                        self.expect(',')?;
                    }
                }
                match array {
                    Some('B') => Ok(Tag::Bytes(
                        values
                            .into_iter()
                            .map(|v| match v {
                                Tag::Byte(n) => Ok(n as u8),
                                _ => Err("SNBT byte array element".into()),
                            })
                            .collect::<Result<_>>()?,
                    )),
                    Some('I') => Ok(Tag::Ints(
                        values
                            .into_iter()
                            .map(|v| match v {
                                Tag::Int(n) => Ok(n),
                                _ => Err("SNBT int array element".into()),
                            })
                            .collect::<Result<_>>()?,
                    )),
                    Some('L') => Ok(Tag::Longs(
                        values
                            .into_iter()
                            .map(|v| match v {
                                Tag::Long(n) => Ok(n),
                                _ => Err("SNBT long array element".into()),
                            })
                            .collect::<Result<_>>()?,
                    )),
                    _ => Ok(Tag::List(values)),
                }
            }
            _ => {
                let word = self.word(false);
                if word.is_empty() {
                    return Err("empty SNBT token".into());
                }
                match word.as_str() {
                    "true" => return Ok(Tag::Byte(1)),
                    "false" => return Ok(Tag::Byte(0)),
                    _ => {}
                }
                let last = word.as_bytes()[word.len() - 1].to_ascii_lowercase();
                let number = if matches!(last, b'b' | b's' | b'l' | b'f' | b'd') {
                    &word[..word.len() - 1]
                } else {
                    word.as_str()
                };
                let result = match last {
                    b'b' => number.parse::<i8>().ok().map(Tag::Byte),
                    b's' => number.parse::<i16>().ok().map(Tag::Short),
                    b'l' => number.parse::<i64>().ok().map(Tag::Long),
                    b'f' => number.parse::<f32>().ok().map(Tag::Float),
                    b'd' => number.parse::<f64>().ok().map(Tag::Double),
                    _ => word
                        .parse::<i32>()
                        .ok()
                        .map(Tag::Int)
                        .or_else(|| word.parse::<f64>().ok().map(Tag::Double)),
                };
                Ok(result.unwrap_or_else(|| Tag::string(word)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_empty_compound_key_is_valid_but_missing_key_is_not() {
        assert_eq!(
            decode("{\"\":2}").unwrap(),
            Tag::record([("", Tag::Int(2))])
        );
        assert_eq!(decode("{'':2}").unwrap(), Tag::record([("", Tag::Int(2))]));
        assert!(decode("{:2}").is_err());
    }
}
