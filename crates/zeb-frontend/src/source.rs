use crate::Diagnostic;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Encoding {
    Auto,
    Utf8,
    Ascii,
    Latin1,
    Utf16Le,
    Utf16Be,
}

impl Encoding {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "auto" => Self::Auto,
            "utf-8" => Self::Utf8,
            "ascii" => Self::Ascii,
            "latin-1" => Self::Latin1,
            "utf-16le" => Self::Utf16Le,
            "utf-16be" => Self::Utf16Be,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unit {
    pub ch: char,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug)]
pub struct Source {
    bytes: Vec<u8>,
    units: Vec<Unit>,
    lines: Vec<usize>,
    pub encoding: Encoding,
}

impl Source {
    pub fn read(path: &Path, encoding: Encoding, limit: usize) -> Result<Self, Diagnostic> {
        Self::read_options(path, encoding, limit, false)
    }
    pub fn read_declared(
        path: &Path,
        encoding: Encoding,
        limit: usize,
    ) -> Result<Self, Diagnostic> {
        Self::read_options(path, encoding, limit, true)
    }
    fn read_options(
        path: &Path,
        mut encoding: Encoding,
        limit: usize,
        declared: bool,
    ) -> Result<Self, Diagnostic> {
        let mut file = File::open(path)
            .map_err(|_| Diagnostic::new("source-io", "cannot open source file", 0))?;
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 8192];
        loop {
            let length = match file.read(&mut chunk) {
                Ok(length) => length,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    return Err(Diagnostic::new(
                        "source-io",
                        "cannot read source file",
                        bytes.len(),
                    ));
                }
            };
            if length == 0 {
                break;
            }
            if length > limit.saturating_sub(bytes.len()) {
                return Err(Diagnostic::new(
                    "source-limit",
                    "source exceeds configured byte budget",
                    limit,
                ));
            }
            bytes
                .try_reserve(length)
                .map_err(|_| Diagnostic::resource(bytes.len()))?;
            bytes.extend_from_slice(&chunk[..length]);
        }
        if declared && bytes.starts_with(b"#charset ") {
            let first = bytes
                .split(|b| matches!(b, b'\r' | b'\n'))
                .next()
                .unwrap_or(&[]);
            let line = std::str::from_utf8(first)
                .map_err(|_| Diagnostic::new("source-encoding", "invalid charset directive", 0))?;
            let name = line[9..].trim();
            let name = name
                .strip_prefix('"')
                .and_then(|n| n.strip_suffix('"'))
                .ok_or(Diagnostic::new(
                    "source-encoding",
                    "invalid charset directive",
                    0,
                ))?;
            encoding = match name {
                "us-ascii" => Some(Encoding::Ascii),
                "latin1" => Some(Encoding::Latin1),
                _ => Encoding::parse(name),
            }
            .ok_or(Diagnostic::new(
                "source-encoding",
                "unsupported file charset",
                0,
            ))?;
        }
        Self::decode(bytes, encoding)
    }

    pub fn decode(bytes: Vec<u8>, requested: Encoding) -> Result<Self, Diagnostic> {
        let (bom, skip) = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
            (Some(Encoding::Utf8), 3)
        } else if bytes.starts_with(&[0xff, 0xfe]) {
            (Some(Encoding::Utf16Le), 2)
        } else if bytes.starts_with(&[0xfe, 0xff]) {
            (Some(Encoding::Utf16Be), 2)
        } else {
            (None, 0)
        };
        let encoding = if requested == Encoding::Auto {
            bom.unwrap_or(Encoding::Utf8)
        } else {
            requested
        };
        if bom.is_some_and(|value| value != encoding) {
            return Err(Diagnostic::new(
                "source-bom",
                "BOM conflicts with selected encoding",
                0,
            ));
        }
        let mut units = Vec::new();
        let mut push = |ch, start, end| -> Result<(), Diagnostic> {
            if ch == '\0' {
                return Err(Diagnostic::new(
                    "source-nul",
                    "physical NUL is invalid source",
                    start,
                ));
            }
            units
                .try_reserve(1)
                .map_err(|_| Diagnostic::resource(start))?;
            units.push(Unit { ch, start, end });
            Ok(())
        };
        match encoding {
            Encoding::Auto => unreachable!("auto resolved above"),
            Encoding::Utf8 => {
                let text = std::str::from_utf8(&bytes[skip..]).map_err(|error| {
                    Diagnostic::new(
                        "source-encoding",
                        "invalid UTF-8 sequence",
                        skip + error.valid_up_to(),
                    )
                })?;
                for (offset, ch) in text.char_indices() {
                    push(ch, skip + offset, skip + offset + ch.len_utf8())?;
                }
            }
            Encoding::Ascii | Encoding::Latin1 => {
                for (offset, &byte) in bytes.iter().enumerate().skip(skip) {
                    if encoding == Encoding::Ascii && !byte.is_ascii() {
                        return Err(Diagnostic::new(
                            "source-encoding",
                            "non-ASCII source byte",
                            offset,
                        ));
                    }
                    push(char::from(byte), offset, offset + 1)?;
                }
            }
            Encoding::Utf16Le | Encoding::Utf16Be => {
                if !(bytes.len() - skip).is_multiple_of(2) {
                    return Err(Diagnostic::new(
                        "source-encoding",
                        "incomplete UTF-16 code unit",
                        bytes.len() - 1,
                    ));
                }
                let word = |index: usize| {
                    let pair = [bytes[index], bytes[index + 1]];
                    if encoding == Encoding::Utf16Le {
                        u16::from_le_bytes(pair)
                    } else {
                        u16::from_be_bytes(pair)
                    }
                };
                let mut offset = skip;
                while offset < bytes.len() {
                    let start = offset;
                    let first = word(offset);
                    offset += 2;
                    let scalar = if (0xd800..=0xdbff).contains(&first) {
                        if offset == bytes.len() {
                            return Err(Diagnostic::new(
                                "source-encoding",
                                "unpaired UTF-16 high surrogate",
                                start,
                            ));
                        }
                        let second = word(offset);
                        if !(0xdc00..=0xdfff).contains(&second) {
                            return Err(Diagnostic::new(
                                "source-encoding",
                                "invalid UTF-16 surrogate pair",
                                start,
                            ));
                        }
                        offset += 2;
                        0x10000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00)
                    } else {
                        u32::from(first)
                    };
                    let ch = char::from_u32(scalar).ok_or(Diagnostic::new(
                        "source-encoding",
                        "unpaired UTF-16 low surrogate",
                        start,
                    ))?;
                    push(ch, start, offset)?;
                }
            }
        }
        let mut lines = Vec::new();
        lines.try_reserve(1).map_err(|_| Diagnostic::resource(0))?;
        lines.push(skip);
        let mut previous_cr = false;
        for unit in &units {
            if unit.ch == '\n' && previous_cr {
                if let Some(last) = lines.last_mut() {
                    *last = unit.end;
                }
            } else if unit.ch == '\r' || unit.ch == '\n' {
                lines
                    .try_reserve(1)
                    .map_err(|_| Diagnostic::resource(unit.start))?;
                lines.push(unit.end);
            }
            previous_cr = unit.ch == '\r';
        }
        Ok(Self {
            bytes,
            units,
            lines,
            encoding,
        })
    }

    pub fn units(&self) -> &[Unit] {
        &self.units
    }
    pub fn original_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }

    /// One-based line and original encoded byte column (BOM is not content).
    pub fn location(&self, byte: usize) -> (usize, usize) {
        let line = self
            .lines
            .partition_point(|&start| start <= byte)
            .saturating_sub(1);
        let start = self.lines.get(line).copied().unwrap_or(0);
        (line + 1, byte.saturating_sub(start) + 1)
    }
}
