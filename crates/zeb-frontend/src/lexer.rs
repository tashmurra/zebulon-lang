use crate::{Diagnostic, source::Source};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Identifier,
    Integer(u32),
    BigNumber,
    Quoted {
        emitting: bool,
        triple: bool,
    },
    StringPart {
        emitting: bool,
        triple: bool,
        head: bool,
        last: bool,
    },
    Symbol(&'static str),
    End,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: Kind,
    pub byte_start: usize,
    pub byte_end: usize,
    start: usize,
    end: usize,
}

impl Token {
    pub fn literal(&self, source: &Source) -> Result<crate::strings::Literal, Diagnostic> {
        match self.kind {
            Kind::StringPart {
                emitting,
                triple,
                head: false,
                ..
            } => crate::strings::decode_part(source, self.start, emitting, triple),
            _ => crate::strings::decode(source, self.start),
        }
    }
    pub fn spelling<'a>(&self, source: &'a Source) -> impl Iterator<Item = char> + 'a {
        source
            .units()
            .get(self.start..self.end)
            .unwrap_or(&[])
            .iter()
            .map(|unit| unit.ch)
    }
}

const SYMBOLS: &[&str] = &[
    "->", "@", ">>>=", "<<=", ">>=", ">>>", "...", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=",
    "|=", "^=", "==", "!=", "<=", ">=", "&&", "||", "<<", ">>", "??", "..", "{", "}", "(", ")",
    "[", "]", ";", ",", ":", "?", "+", "-", "*", "/", "%", "&", "|", "^", "~", "!", "=", "<", ">",
    ".",
];

pub fn lex(source: &Source) -> Result<Vec<Token>, Diagnostic> {
    let units = source.units();
    let ch = |index: usize| units.get(index).map(|unit| unit.ch);
    let mut tokens = Vec::new();
    let mut cursor = 0;
    let mut embeddings: Vec<(bool, bool, usize)> = Vec::new();
    while let Some(current) = ch(cursor) {
        if let Some(&(emitting, triple, depth)) = embeddings.last()
            && current == '>'
            && ch(cursor + 1) == Some('>')
            && depth == 0
        {
            cursor += 2;
            let start = cursor;
            let (end, more) = crate::strings::scan_part(
                source,
                start,
                if emitting { '"' } else { '\'' },
                triple,
                |_| Ok(()),
            )?;
            cursor = if more { end + 2 } else { end };
            tokens
                .try_reserve(1)
                .map_err(|_| Diagnostic::resource(units[start].start))?;
            tokens.push(Token {
                kind: Kind::StringPart {
                    emitting,
                    triple,
                    head: false,
                    last: !more,
                },
                byte_start: units[start].start,
                byte_end: units[cursor - 1].end,
                start,
                end: cursor,
            });
            if !more {
                embeddings.pop();
            }
            continue;
        }
        if current.is_ascii_whitespace() {
            cursor += 1;
            continue;
        }
        let start = cursor;
        let byte = units[start].start;
        if current == '/' && ch(cursor + 1) == Some('/') {
            cursor += 2;
            while ch(cursor).is_some_and(|value| value != '\r' && value != '\n') {
                cursor += 1;
            }
            continue;
        }
        if current == '/' && ch(cursor + 1) == Some('*') {
            cursor += 2;
            while ch(cursor).is_some() && !(ch(cursor) == Some('*') && ch(cursor + 1) == Some('/'))
            {
                cursor += 1;
            }
            if ch(cursor).is_none() {
                return Err(Diagnostic::new(
                    "lex-comment",
                    "unterminated block comment",
                    byte,
                ));
            }
            cursor += 2;
            continue;
        }
        let kind = if current.is_ascii_alphabetic() || current == '_' {
            cursor += 1;
            while ch(cursor).is_some_and(|value| value.is_ascii_alphanumeric() || value == '_') {
                cursor += 1;
            }
            Kind::Identifier
        } else if current.is_ascii_digit()
            || (current == '.' && ch(cursor + 1).is_some_and(|value| value.is_ascii_digit()))
        {
            if current == '0' && matches!(ch(cursor + 1), Some('x' | 'X')) {
                cursor += 2;
                let digits = cursor;
                while ch(cursor).is_some_and(|value| value.is_ascii_hexdigit()) {
                    cursor += 1;
                }
                if cursor == digits {
                    return Err(Diagnostic::new(
                        "lex-number",
                        "hexadecimal literal needs digits",
                        byte,
                    ));
                }
                if ch(cursor).is_some_and(|value| value.is_ascii_alphanumeric() || value == '_') {
                    return Err(Diagnostic::new(
                        "lex-number",
                        "invalid hexadecimal digit or suffix",
                        units[cursor].start,
                    ));
                }
                Kind::Integer(16)
            } else {
                let mut decimal = false;
                while ch(cursor).is_some_and(|value| value.is_ascii_digit()) {
                    cursor += 1;
                }
                if ch(cursor) == Some('.') && ch(cursor + 1) != Some('.') {
                    decimal = true;
                    cursor += 1;
                    while ch(cursor).is_some_and(|value| value.is_ascii_digit()) {
                        cursor += 1;
                    }
                }
                if matches!(ch(cursor), Some('e' | 'E')) {
                    decimal = true;
                    cursor += 1;
                    if matches!(ch(cursor), Some('+' | '-')) {
                        cursor += 1;
                    }
                    let digits = cursor;
                    while ch(cursor).is_some_and(|value| value.is_ascii_digit()) {
                        cursor += 1;
                    }
                    if cursor == digits {
                        return Err(Diagnostic::new("lex-number", "exponent needs digits", byte));
                    }
                }
                if ch(cursor).is_some_and(|value| value.is_ascii_alphabetic() || value == '_') {
                    return Err(Diagnostic::new(
                        "lex-number",
                        "invalid numeric suffix",
                        units[cursor].start,
                    ));
                }
                if decimal {
                    Kind::BigNumber
                } else if current == '0' && cursor - start > 1 {
                    if units[start..cursor].iter().any(|unit| unit.ch > '7') {
                        return Err(Diagnostic::new("lex-number", "invalid octal digit", byte));
                    }
                    Kind::Integer(8)
                } else {
                    Kind::Integer(10)
                }
            }
        } else if matches!(current, '\'' | '"') {
            let (end, emitting, triple, embedded) =
                crate::strings::scan(source, cursor, |_| Ok(()))?;
            cursor = if embedded { end + 2 } else { end };
            if embedded {
                embeddings
                    .try_reserve(1)
                    .map_err(|_| Diagnostic::resource(byte))?;
                embeddings.push((emitting, triple, 0));
                Kind::StringPart {
                    emitting,
                    triple,
                    head: true,
                    last: false,
                }
            } else {
                Kind::Quoted { emitting, triple }
            }
        } else if current == '#' {
            return Err(Diagnostic::new(
                "lex-unavailable",
                "preprocessing is not implemented in this slice",
                byte,
            ));
        } else {
            let symbol = SYMBOLS
                .iter()
                .find(|symbol| {
                    symbol
                        .chars()
                        .enumerate()
                        .all(|(index, value)| ch(cursor + index) == Some(value))
                })
                .ok_or(Diagnostic::new(
                    "lex-character",
                    "invalid character for this lexical profile",
                    byte,
                ))?;
            cursor += symbol.len();
            Kind::Symbol(symbol)
        };
        if let Some((_, _, depth)) = embeddings.last_mut() {
            match kind {
                Kind::Symbol("(" | "[" | "{") => {
                    *depth = depth.checked_add(1).ok_or(Diagnostic::resource(byte))?
                }
                Kind::Symbol(")" | "]" | "}") => *depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        let end_byte = units[cursor - 1].end;
        tokens
            .try_reserve(1)
            .map_err(|_| Diagnostic::resource(byte))?;
        tokens.push(Token {
            kind,
            byte_start: byte,
            byte_end: end_byte,
            start,
            end: cursor,
        });
    }
    if !embeddings.is_empty() {
        return Err(Diagnostic::new(
            "lex-string",
            "unterminated interpolation",
            source.byte_len(),
        ));
    }
    tokens
        .try_reserve(1)
        .map_err(|_| Diagnostic::resource(source.byte_len()))?;
    tokens.push(Token {
        kind: Kind::End,
        byte_start: source.byte_len(),
        byte_end: source.byte_len(),
        start: cursor,
        end: cursor,
    });
    Ok(tokens)
}
