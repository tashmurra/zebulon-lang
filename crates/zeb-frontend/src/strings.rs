//! Independent plain-literal scanning from the public TADS string specification.
//! Interpolation, regex literals and source-controlled newline modes remain open.
use crate::{Diagnostic, source::Source};

#[derive(Debug)]
pub struct Literal {
    pub text: String,
    pub emitting: bool,
    pub triple: bool,
}

/// Scan once with a sink; tokenization validates without allocating decoded text.
pub(crate) fn scan(
    source: &Source,
    start: usize,
    mut emit: impl FnMut(char) -> Result<(), Diagnostic>,
) -> Result<(usize, bool, bool, bool), Diagnostic> {
    let units = source.units();
    let ch = |i: usize| units.get(i).map(|u| u.ch);
    let byte = units.get(start).map_or(source.byte_len(), |u| u.start);
    let quote = ch(start)
        .filter(|c| matches!(c, '\'' | '"'))
        .ok_or(Diagnostic::new(
            "lex-string",
            "expected string delimiter",
            byte,
        ))?;
    let triple = ch(start + 1) == Some(quote) && ch(start + 2) == Some(quote);
    let cursor = start + if triple { 3 } else { 1 };
    let (end, embedded) =
        scan_part(source, cursor, quote, triple, &mut emit).map_err(|mut error| {
            if error.code == "lex-string" && error.message == "unterminated string literal" {
                error.byte = byte;
            }
            error
        })?;
    Ok((end, quote == '"', triple, embedded))
}

pub(crate) fn scan_part(
    source: &Source,
    start: usize,
    quote: char,
    triple: bool,
    mut emit: impl FnMut(char) -> Result<(), Diagnostic>,
) -> Result<(usize, bool), Diagnostic> {
    let units = source.units();
    let ch = |i: usize| units.get(i).map(|u| u.ch);
    let byte = units.get(start).map_or(source.byte_len(), |u| u.start);
    let mut cursor = start;
    let mut escaped_newline = false;
    while let Some(current) = ch(cursor) {
        if current == quote {
            let run_start = cursor;
            while ch(cursor) == Some(quote) {
                cursor += 1;
            }
            let count = cursor - run_start;
            if !triple {
                return Ok((run_start + 1, false));
            }
            let text_count = if count >= 3 { count - 3 } else { count };
            for _ in 0..text_count {
                emit(quote)?;
            }
            if count >= 3 {
                return Ok((cursor, false));
            }
            escaped_newline = false;
            continue;
        }
        if current == '<' && ch(cursor + 1) == Some('<') {
            return Ok((cursor, true));
        }
        if matches!(current, '\r' | '\n') {
            cursor += 1;
            if current == '\r' && ch(cursor) == Some('\n') {
                cursor += 1;
            }
            if !escaped_newline {
                emit(' ')?;
                while matches!(ch(cursor), Some(' ' | '\t')) {
                    cursor += 1;
                }
            }
            escaped_newline = false;
            continue;
        }
        if current != '\\' {
            emit(current)?;
            cursor += 1;
            escaped_newline = false;
            continue;
        }
        let escape_byte = units[cursor].start;
        cursor += 1;
        let code = ch(cursor).ok_or(Diagnostic::new(
            "lex-string",
            "incomplete escape at end of source",
            escape_byte,
        ))?;
        cursor += 1;
        escaped_newline = code == 'n';
        if triple && code == quote {
            emit(quote)?;
            while ch(cursor) == Some(quote) {
                emit(quote)?;
                cursor += 1;
            }
            continue;
        }
        match code {
            '\r' | '\n' => {
                if code == '\r' && ch(cursor) == Some('\n') {
                    cursor += 1;
                }
            }
            '\\' | '\'' | '"' | '<' | '>' => emit(code)?,
            'n' => emit('\n')?,
            'r' => emit('\r')?,
            't' => emit('\t')?,
            'b' => emit('\u{b}')?,
            'v' => emit('\u{e}')?,
            '^' => emit('\u{f}')?,
            ' ' => emit('\u{15}')?,
            'x' | 'u' => {
                let limit = if code == 'x' { 2 } else { 4 };
                let mut value = 0u32;
                let mut count = 0;
                while count < limit {
                    let Some(digit) = ch(cursor).and_then(|c| c.to_digit(16)) else {
                        break;
                    };
                    value = value * 16 + digit;
                    cursor += 1;
                    count += 1;
                }
                if count == 0 {
                    return Err(Diagnostic::new(
                        "lex-escape",
                        "numeric escape requires digits",
                        escape_byte,
                    ));
                }
                emit(char::from_u32(value).ok_or(Diagnostic::new(
                    "lex-escape",
                    "escape is not a Unicode scalar",
                    escape_byte,
                ))?)?;
            }
            '0'..='7' => {
                let limit = if code <= '3' { 3 } else { 2 };
                let mut value = u32::from(code) - u32::from('0');
                let mut count = 1;
                while count < limit {
                    let Some(digit) = ch(cursor).and_then(|c| c.to_digit(8)) else {
                        break;
                    };
                    value = value * 8 + digit;
                    cursor += 1;
                    count += 1;
                }
                emit(char::from_u32(value).expect("bounded byte escape"))?;
            }
            _ => {
                emit('\\')?;
                emit(code)?;
            }
        }
    }
    Err(Diagnostic::new(
        "lex-string",
        "unterminated string literal",
        byte,
    ))
}

pub(crate) fn decode(source: &Source, start: usize) -> Result<Literal, Diagnostic> {
    let mut text = String::new();
    let byte = source
        .units()
        .get(start)
        .map_or(source.byte_len(), |u| u.start);
    let (_, emitting, triple, _) = scan(source, start, |ch| {
        text.try_reserve(ch.len_utf8())
            .map_err(|_| Diagnostic::resource(byte))?;
        text.push(ch);
        Ok(())
    })?;
    Ok(Literal {
        text,
        emitting,
        triple,
    })
}

pub(crate) fn decode_part(
    source: &Source,
    start: usize,
    emitting: bool,
    triple: bool,
) -> Result<Literal, Diagnostic> {
    let mut text = String::new();
    scan_part(
        source,
        start,
        if emitting { '"' } else { '\'' },
        triple,
        |ch| {
            text.try_reserve(ch.len_utf8())
                .map_err(|_| Diagnostic::resource(0))?;
            text.push(ch);
            Ok(())
        },
    )?;
    Ok(Literal {
        text,
        emitting,
        triple,
    })
}
