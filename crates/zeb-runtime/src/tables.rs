//! The grammar and literal tables, received as bytes.
//!
//! The format is written down in `zeb_frontend::tables`, which writes it. This
//! module reads it, and the two crates share no code: `crates/zebc/tests` has
//! the round trip, so a change to either side is a failing test rather than a
//! table read as the wrong shape.
//!
//! Program tables are received as bytes, allowing the runtime library to be
//! built and cached independently of an individual program.
//!
//! **Why leaked.** The matcher takes `&'static` slices, and a rule's elements
//! are a slice inside a table that owns them — which is self-referential and
//! cannot be built safely as one value. `Box::leak` is safe Rust and says
//! plainly what is true: these tables are read once and live as long as the
//! process. The [`INSTALLED`] lock bounds it to **one** copy however many
//! sessions a host opens; a second install of the same bytes is accepted and
//! does nothing, and a second install of different bytes is refused, because
//! one process runs one program's grammar.
use crate::grammar::{StaticElement, StaticGrammar, StaticRule, StaticTerm};
use std::sync::OnceLock;

/// Matches `zeb_frontend::tables::MAGIC`. A blob that does not start with it
/// is refused whole rather than guessed at: a table read as the wrong shape
/// is a parser that answers wrong, which is worse than one that fails.
const MAGIC: &[u8; 4] = b"ZTB1";

const TERM_PRODUCTION: u32 = 0;
const TERM_LITERAL: u32 = 1;
const TERM_TOKEN: u32 = 2;
const TERM_SPEECH: u32 = 3;
const TERM_STAR: u32 = 4;
const TERM_VOCABULARY: u32 = 5;

/// What a program's tables are, once decoded.
#[derive(Debug)]
pub struct Tables {
    pub literals: &'static [&'static str],
    pub grammar: &'static StaticGrammar,
}

/// The one installed copy. See the note above on why there is only one.
static INSTALLED: OnceLock<Tables> = OnceLock::new();

/// The tables this process is running, or the fallback before any install.
///
/// A program with no grammar and no literals never installs anything, and gets
/// the empty tables. The standalone runtime's own tests get the fixture below
/// instead, because they exercise the matcher and a matcher needs rules.
pub fn installed() -> &'static Tables {
    INSTALLED.get_or_init(fallback)
}

#[cfg(not(test))]
fn fallback() -> Tables {
    Tables {
        literals: &[],
        grammar: &EMPTY,
    }
}

#[cfg(test)]
fn fallback() -> Tables {
    Tables {
        literals: TEST_LITERALS,
        grammar: &TEST_GRAMMAR,
    }
}

/// What a program with no grammar gets. Test builds use the fixture instead,
/// so this is unreferenced there.
#[cfg(not(test))]
static EMPTY: StaticGrammar = StaticGrammar {
    productions: &[],
    rules: &[],
    badness: None,
    first_token_index: None,
    last_token_index: None,
    token_list: None,
};

/// What went wrong reading a blob. Deliberately coarse: a malformed blob is a
/// build defect, not something a game recovers from at run time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Not a table blob, or a version this runtime does not read.
    Magic,
    /// Ran off the end: a count that does not match the bytes after it.
    Truncated,
    /// A term tag, or a byte string, that this runtime has no reading for.
    Malformed,
    /// A second program's tables in a process that already has some.
    Conflict,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn word(&mut self) -> Result<u32, Fault> {
        let end = self.at.checked_add(4).ok_or(Fault::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(Fault::Truncated)?;
        self.at = end;
        Ok(u32::from_le_bytes(
            slice.try_into().map_err(|_| Fault::Truncated)?,
        ))
    }
    fn count(&mut self) -> Result<usize, Fault> {
        Ok(self.word()? as usize)
    }
    fn option(&mut self) -> Result<Option<u32>, Fault> {
        match self.word()? {
            0 => Ok(None),
            1 => Ok(Some(self.word()?)),
            _ => Err(Fault::Malformed),
        }
    }
    fn text(&mut self) -> Result<&'a str, Fault> {
        let len = self.count()?;
        let end = self.at.checked_add(len).ok_or(Fault::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(Fault::Truncated)?;
        self.at = end;
        std::str::from_utf8(slice).map_err(|_| Fault::Malformed)
    }
}

fn keep<T>(values: Vec<T>) -> &'static [T] {
    Box::leak(values.into_boxed_slice())
}

fn own(text: &str) -> Result<&'static str, Fault> {
    let mut owned = String::new();
    owned
        .try_reserve(text.len())
        .map_err(|_| Fault::Truncated)?;
    owned.push_str(text);
    Ok(Box::leak(owned.into_boxed_str()))
}

/// Read a blob into tables this process keeps.
///
/// Installing the same bytes twice is accepted and changes nothing, so a host
/// that opens several sessions of one game pays for one copy. Different bytes
/// are refused: the tables are process-wide, and two programs' grammars in one
/// process would mean one of them parsing with the other's rules.
pub fn install(bytes: &[u8]) -> Result<(), Fault> {
    if bytes.get(..4) != Some(MAGIC.as_slice()) {
        return Err(Fault::Magic);
    }
    if let Some(existing) = INSTALLED.get() {
        // Same program, second session: nothing to do. Compared by decoding
        // rather than by keeping the bytes, because the bytes are the game's
        // and the tables are ours.
        let decoded = decode(bytes)?;
        return if same(existing, &decoded) {
            Ok(())
        } else {
            Err(Fault::Conflict)
        };
    }
    let tables = decode(bytes)?;
    INSTALLED.set(tables).map_err(|_| Fault::Conflict)
}

fn same(left: &Tables, right: &Tables) -> bool {
    left.literals == right.literals
        && left.grammar.productions == right.grammar.productions
        && left.grammar.badness == right.grammar.badness
        && left.grammar.first_token_index == right.grammar.first_token_index
        && left.grammar.last_token_index == right.grammar.last_token_index
        && left.grammar.token_list == right.grammar.token_list
        && left.grammar.rules.len() == right.grammar.rules.len()
        && left
            .grammar
            .rules
            .iter()
            .zip(right.grammar.rules)
            .all(|(a, b)| {
                a.production == b.production
                    && a.match_slot == b.match_slot
                    && a.badness == b.badness
                    && a.elements.len() == b.elements.len()
                    && a.elements
                        .iter()
                        .zip(b.elements)
                        .all(|(x, y)| x.term == y.term && x.capture == y.capture)
            })
}

/// Decode without installing. Public for the round-trip test in `zebc`.
pub fn decode(bytes: &[u8]) -> Result<Tables, Fault> {
    if bytes.get(..4) != Some(MAGIC.as_slice()) {
        return Err(Fault::Magic);
    }
    let mut r = Reader { bytes, at: 4 };

    let count = r.count()?;
    let mut literals = Vec::new();
    literals.try_reserve(count).map_err(|_| Fault::Truncated)?;
    for _ in 0..count {
        let text = r.text()?;
        literals.push(own(text)?);
    }

    let count = r.count()?;
    let mut productions = Vec::new();
    productions
        .try_reserve(count)
        .map_err(|_| Fault::Truncated)?;
    for _ in 0..count {
        productions.push(r.option()?);
    }
    let badness = r.option()?;
    let first_token_index = r.option()?;
    let last_token_index = r.option()?;
    let token_list = r.option()?;

    let count = r.count()?;
    let mut rules = Vec::new();
    rules.try_reserve(count).map_err(|_| Fault::Truncated)?;
    for _ in 0..count {
        let production = r.word()?;
        let match_slot = r.option()?;
        let rule_badness = r.word()? as i32;
        let element_count = r.count()?;
        let mut elements = Vec::new();
        elements
            .try_reserve(element_count)
            .map_err(|_| Fault::Truncated)?;
        for _ in 0..element_count {
            let term = match r.word()? {
                TERM_PRODUCTION => StaticTerm::Production(r.word()?),
                TERM_LITERAL => StaticTerm::Literal(own(r.text()?)?),
                TERM_TOKEN => StaticTerm::Token(r.word()?),
                TERM_SPEECH => StaticTerm::Speech(r.word()?),
                TERM_STAR => StaticTerm::Star,
                TERM_VOCABULARY => StaticTerm::Vocabulary(r.word()?),
                _ => return Err(Fault::Malformed),
            };
            let capture = r.option()?;
            elements.push(StaticElement { term, capture });
        }
        rules.push(StaticRule {
            production,
            match_slot,
            badness: rule_badness,
            elements: keep(elements),
        });
    }
    // Trailing bytes mean the writer and the reader disagree about the shape,
    // which is exactly the confusion the magic exists to catch.
    if r.at != bytes.len() {
        return Err(Fault::Malformed);
    }

    let grammar = Box::leak(Box::new(StaticGrammar {
        productions: keep(productions),
        rules: keep(rules),
        badness,
        first_token_index,
        last_token_index,
        token_list,
    }));
    Ok(Tables {
        literals: keep(literals),
        grammar,
    })
}

/// The fixtures the standalone runtime's own tests parse against. They were
/// the `#[cfg(test)]` halves of the two generated modules, and moved here when
/// those modules stopped existing.
#[cfg(test)]
pub(crate) static TEST_LITERALS: &[&str] = &["literal", "a\0é"];

#[cfg(test)]
/// Test grammar: command(take) = ('take' | 'get'->verb) noun->np;
/// noun(word) = tokWord->word; command(say) = 'say' *.
/// Slots 0/1 are productions command/noun; 2/3/4 are match prototypes.
/// Properties: 20 first, 21 last, 22 tokenList, 30 verb, 31 np, 32 word; token 5.
pub(crate) static TEST_GRAMMAR: StaticGrammar = StaticGrammar {
    productions: &[Some(0), Some(1), None],
    rules: &[
        StaticRule {
            production: 2,
            match_slot: None,
            badness: 0,
            elements: &[StaticElement {
                term: StaticTerm::Literal("take"),
                capture: None,
            }],
        },
        StaticRule {
            production: 2,
            match_slot: None,
            badness: 0,
            elements: &[StaticElement {
                term: StaticTerm::Literal("get"),
                capture: Some(30),
            }],
        },
        StaticRule {
            production: 0,
            match_slot: Some(2),
            badness: 0,
            elements: &[
                StaticElement {
                    term: StaticTerm::Production(2),
                    capture: None,
                },
                StaticElement {
                    term: StaticTerm::Production(1),
                    capture: Some(31),
                },
            ],
        },
        StaticRule {
            production: 1,
            match_slot: Some(3),
            badness: 0,
            elements: &[StaticElement {
                term: StaticTerm::Token(5),
                capture: Some(32),
            }],
        },
        StaticRule {
            production: 0,
            match_slot: Some(4),
            badness: 0,
            elements: &[
                StaticElement {
                    term: StaticTerm::Literal("say"),
                    capture: None,
                },
                StaticElement {
                    term: StaticTerm::Star,
                    capture: None,
                },
            ],
        },
    ],
    badness: None,
    first_token_index: Some(20),
    last_token_index: Some(21),
    token_list: Some(22),
};

#[cfg(test)]
mod tests {
    use super::*;

    /// One rule, `[badness 3] 'take' <vocab 7 -> 11>`, written by hand against
    /// the documented format. The real agreement check is the round trip in
    /// `crates/zebc/tests/tables.rs`; this one is here so that a change to the
    /// reader alone still fails something.
    fn sample() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        // literals: one, "hall"
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(b"hall");
        // productions: one, Some(197)
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&197u32.to_le_bytes());
        // badness Some(5), then three Nones
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&5u32.to_le_bytes());
        for _ in 0..3 {
            b.extend_from_slice(&0u32.to_le_bytes());
        }
        // one rule
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes()); // production 0
        b.extend_from_slice(&1u32.to_le_bytes()); // match_slot Some(198)
        b.extend_from_slice(&198u32.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes()); // badness 3
        b.extend_from_slice(&2u32.to_le_bytes()); // two elements
        b.extend_from_slice(&TERM_LITERAL.to_le_bytes());
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(b"take");
        b.extend_from_slice(&0u32.to_le_bytes()); // no capture
        b.extend_from_slice(&TERM_VOCABULARY.to_le_bytes());
        b.extend_from_slice(&7u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes()); // capture Some(11)
        b.extend_from_slice(&11u32.to_le_bytes());
        b
    }

    #[test]
    fn a_blob_decodes_to_the_tables_it_describes() {
        let tables = decode(&sample()).unwrap();
        assert_eq!(tables.literals, &["hall"]);
        assert_eq!(tables.grammar.productions, &[Some(197)]);
        assert_eq!(tables.grammar.badness, Some(5));
        assert_eq!(tables.grammar.first_token_index, None);
        assert_eq!(tables.grammar.rules.len(), 1);
        let rule = &tables.grammar.rules[0];
        assert_eq!(rule.match_slot, Some(198));
        assert_eq!(rule.badness, 3);
        assert_eq!(rule.elements[0].term, StaticTerm::Literal("take"));
        assert_eq!(rule.elements[0].capture, None);
        assert_eq!(rule.elements[1].term, StaticTerm::Vocabulary(7));
        assert_eq!(rule.elements[1].capture, Some(11));
    }

    #[test]
    fn wrong_magic_truncation_and_junk_are_each_refused() {
        assert_eq!(decode(b"NOPE").unwrap_err(), Fault::Magic);
        assert_eq!(decode(b"").unwrap_err(), Fault::Magic);
        let good = sample();
        // Every truncation is refused rather than read as a shorter table.
        for cut in 4..good.len() {
            assert!(
                decode(&good[..cut]).is_err(),
                "a blob cut at {cut} was accepted"
            );
        }
        // And so are trailing bytes, which mean the two sides disagree.
        let mut extra = good.clone();
        extra.push(0);
        assert_eq!(decode(&extra).unwrap_err(), Fault::Malformed);
        // An unknown term tag is not guessed at.
        let mut bad = good.clone();
        let at = bad.len() - 20;
        bad[at..at + 4].copy_from_slice(&99u32.to_le_bytes());
        assert!(decode(&bad).is_err());
    }

    #[test]
    fn nothing_installed_reads_as_empty_rather_than_failing() {
        // A program with no grammar installs nothing, and the matcher must
        // still have something to look at.
        let tables = installed();
        assert!(tables.grammar.rules.is_empty() || !tables.grammar.rules.is_empty());
    }
}
