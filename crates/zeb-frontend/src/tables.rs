//! Grammar and literal tables serialized into each program's native module.
//!
//! The runtime decodes these bytes into owned structures at startup. The
//! format contains no pointers or private Rust layouts. Keeping program tables
//! out of the runtime library allows compatible builds to share its artifact.
//!
//! # The format
//!
//! Little-endian throughout, `u32` unless said otherwise. Read by
//! `zeb_runtime::tables`, which is a separate crate and shares no code with
//! this one — the format below is the whole of the agreement, and
//! `crates/zebc/tests/tables.rs` round-trips one against the other so that a
//! change to either is a failing test rather than a corrupt table.
//!
//! ```text
//! magic        "ZTB1"                        4 bytes, not a u32
//! literals     count, then per literal: len, len bytes of UTF-8
//! productions  count, then per production: an option (see below)
//! badness      option
//! firstToken   option
//! lastToken    option
//! tokenList    option
//! rules        count, then per rule:
//!                production   u32
//!                matchSlot    option
//!                badness      i32, two's complement in a u32
//!                elements     count, then per element:
//!                               term     tag, then its payload
//!                               capture  option
//!
//! option       0                 for none
//!              1, value          for some
//!
//! term tag     0 value           production
//!              1 len, bytes      literal
//!              2 value           token enumerator
//!              3 value           part of speech
//!              4                 wildcard
//!              5 value           vocabulary
//! ```
//!
//! A reader that does not recognise the magic refuses the whole blob rather
//! than guessing, because a table read as the wrong shape is a parser that
//! answers wrong rather than a program that fails.
use crate::{
    Diagnostic,
    grammar::{self, Term},
    parser::{Ast, Declaration, Syntax},
    string_pool,
};
use std::collections::{HashMap, HashSet};

/// Term tags. Named rather than written as numbers at the call site, because
/// the decoder names them too and the two lists have to be read against
/// each other.
pub const TERM_PRODUCTION: u32 = 0;
pub const TERM_LITERAL: u32 = 1;
pub const TERM_TOKEN: u32 = 2;
pub const TERM_SPEECH: u32 = 3;
pub const TERM_STAR: u32 = 4;
pub const TERM_VOCABULARY: u32 = 5;

/// The four bytes a blob starts with. Bumped when the format changes, so an
/// old bundle and a new runtime refuse each other instead of misreading.
pub const MAGIC: &[u8; 4] = b"ZTB1";

struct Writer {
    bytes: Vec<u8>,
    at: usize,
}
impl Writer {
    fn word(&mut self, value: u32) -> Result<(), Diagnostic> {
        self.bytes
            .try_reserve(4)
            .map_err(|_| Diagnostic::resource(self.at))?;
        self.bytes.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }
    fn count(&mut self, value: usize) -> Result<(), Diagnostic> {
        let value = u32::try_from(value).map_err(|_| Diagnostic::resource(self.at))?;
        self.word(value)
    }
    fn option(&mut self, value: Option<u32>) -> Result<(), Diagnostic> {
        match value {
            None => self.word(0),
            Some(value) => {
                self.word(1)?;
                self.word(value)
            }
        }
    }
    fn text(&mut self, value: &str) -> Result<(), Diagnostic> {
        self.count(value.len())?;
        self.bytes
            .try_reserve(value.len())
            .map_err(|_| Diagnostic::resource(self.at))?;
        self.bytes.extend_from_slice(value.as_bytes());
        Ok(())
    }
}

/// The grammar and literal tables for this program.
///
/// The same lowering the Rust generator used, so the tables are the ones the
/// matcher has always been given; only their carriage has changed.
pub fn blob(ast: &Ast) -> Result<Vec<u8>, Diagnostic> {
    let mut w = Writer {
        bytes: Vec::new(),
        at: 0,
    };
    w.bytes
        .try_reserve(MAGIC.len())
        .map_err(|_| Diagnostic::resource(0))?;
    w.bytes.extend_from_slice(MAGIC);

    let pool = string_pool::collect(ast)?;
    w.count(pool.text.len())?;
    for text in &pool.text {
        w.text(text)?;
    }

    let grammar = grammar::lower(ast)?;
    let plan = crate::object_init::declarations(ast)?;
    let enums = crate::sema::enumerators(ast)?;
    let property = |name: &str| {
        plan.properties
            .iter()
            .position(|p| p == name)
            .and_then(|i| u32::try_from(i).ok())
    };
    let mut slots = HashMap::new();
    slots
        .try_reserve(ast.objects.len())
        .map_err(|_| Diagnostic::resource(0))?;
    for (index, id) in ast.objects.iter().enumerate() {
        if let Syntax::Object { name, .. } = &ast.nodes[id.0].syntax {
            let slot = u32::try_from(index).map_err(|_| Diagnostic::resource(0))?;
            slots.insert(name.as_str(), slot);
        }
    }
    let mut vocabulary = HashSet::new();
    let mut tokens = HashSet::new();
    let relations: HashSet<&str> = ast
        .relations
        .iter()
        .filter(|relation| relation.vocabulary && !relation.labelled)
        .map(|relation| relation.forward.as_str())
        .collect();
    let labelled_vocabulary: HashSet<&str> = ast
        .relations
        .iter()
        .filter(|relation| relation.vocabulary && relation.labelled)
        .map(|relation| relation.forward.as_str())
        .collect();
    for node in &ast.nodes {
        match &node.syntax {
            Syntax::Declaration(Declaration::VocabularyProperties(names)) => {
                vocabulary
                    .try_reserve(names.len())
                    .map_err(|_| Diagnostic::resource(node.start))?;
                vocabulary.extend(names.iter().map(String::as_str));
            }
            Syntax::Declaration(Declaration::Enumerators {
                names, token: true, ..
            }) => {
                tokens
                    .try_reserve(names.len())
                    .map_err(|_| Diagnostic::resource(node.start))?;
                tokens.extend(names.iter().map(String::as_str));
            }
            _ => {}
        }
    }

    w.count(grammar.productions.len())?;
    for name in &grammar.productions {
        let slot = match name {
            Some(name) => Some(*slots.get(name).ok_or_else(|| {
                Diagnostic::new(
                    "grammar-production-object",
                    "grammar production has no static object",
                    0,
                )
            })?),
            None => None,
        };
        w.option(slot)?;
    }
    for name in ["badness", "firstTokenIndex", "lastTokenIndex", "tokenList"] {
        w.option(property(name))?;
    }

    w.count(grammar.rules.len())?;
    for rule in &grammar.rules {
        w.at = rule.byte;
        let match_slot = match rule.match_name {
            Some(name) => Some(*slots.get(name).ok_or_else(|| {
                Diagnostic::new(
                    "grammar-match-object",
                    "grammar rule has no match object",
                    rule.byte,
                )
            })?),
            None => None,
        };
        w.word(u32::try_from(rule.production.0).map_err(|_| Diagnostic::resource(rule.byte))?)?;
        w.option(match_slot)?;
        w.word(rule.badness as u32)?;
        w.count(rule.elements.len())?;
        for element in &rule.elements {
            match element.term {
                Term::Production(id) => {
                    w.word(TERM_PRODUCTION)?;
                    w.word(u32::try_from(id.0).map_err(|_| Diagnostic::resource(rule.byte))?)?;
                }
                Term::Wildcard => w.word(TERM_STAR)?,
                Term::Literal(value) => {
                    w.word(TERM_LITERAL)?;
                    w.text(value)?;
                }
                Term::Symbol(name) if tokens.contains(name) => {
                    w.word(TERM_TOKEN)?;
                    w.word(enums[name])?;
                }
                Term::Symbol(name) if relations.contains(name) => {
                    w.word(TERM_VOCABULARY)?;
                    w.word(property(name).expect("declared vocabulary"))?;
                }
                Term::Symbol(name)
                    if name
                        .split_once('.')
                        .is_some_and(|(head, _)| labelled_vocabulary.contains(head)) =>
                {
                    let (_, part) = name.split_once('.').expect("checked above");
                    let identity = property(part).ok_or_else(|| {
                        Diagnostic::new(
                            "sem-vocabulary",
                            "a grammar slot's part of speech is a declared property",
                            rule.byte,
                        )
                    })?;
                    w.word(TERM_VOCABULARY)?;
                    w.word(identity)?;
                }
                Term::Symbol(name) if labelled_vocabulary.contains(name) => {
                    return Err(Diagnostic::new(
                        "sem-vocabulary",
                        "a grammar slot names one part of speech of a labelled vocabulary relation, written vocab.noun",
                        rule.byte,
                    ));
                }
                Term::Symbol(name) if vocabulary.contains(name) => {
                    w.word(TERM_SPEECH)?;
                    w.word(property(name).expect("declared vocabulary"))?;
                }
                Term::Symbol(_) => {
                    return Err(Diagnostic::new(
                        "grammar-symbol-unavailable",
                        "grammar symbol is not a production, token enumerator or dictionary property",
                        rule.byte,
                    ));
                }
            }
            let capture = match element.capture {
                Some(name) => Some(property(name).ok_or_else(|| {
                    Diagnostic::new(
                        "grammar-capture",
                        "grammar capture has no property identity",
                        rule.byte,
                    )
                })?),
                None => None,
            };
            w.option(capture)?;
        }
    }
    Ok(w.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        parser::{self, Model},
        source::{Encoding, Source},
    };

    fn tree(text: &str) -> Ast {
        parser::parse_with(
            &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
            Model::Lifetimes,
        )
        .unwrap()
    }

    const WORLD: &str = "\
enum token tokWord;
property firstTokenIndex, lastTokenIndex, tokenList;
property badness;
property noun;
dictionary cmdDict;
relation vocab(entity: Entity, word: Text, part: Vocabulary) many_to_many;
class Thing: object name = 'thing';
coin: Thing name = 'coin' vocab = 'coin';
class Cmd: object it_ = nil;
grammar command(take): [badness 3] 'take' vocab.noun->it_ : Cmd;
turn(toks){ \"x\"; return command.parseTokens(toks, cmdDict); }";

    #[test]
    fn a_blob_starts_with_its_magic_and_carries_every_literal() {
        let bytes = blob(&tree(WORLD)).unwrap();
        assert_eq!(&bytes[..4], MAGIC);
        let count = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        // The program's literals, whatever the pool decided they are; what
        // matters is that the count and the bytes agree with each other.
        let mut at = 8usize;
        for _ in 0..count {
            let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            at += 4;
            std::str::from_utf8(&bytes[at..at + len]).expect("a literal is UTF-8");
            at += len;
        }
        assert!(at <= bytes.len());
    }

    #[test]
    fn a_rule_keeps_its_badness_and_its_terms() {
        let bytes = blob(&tree(WORLD)).unwrap();
        // The rule carries `[badness 3]`, a literal and a vocabulary slot, so
        // all three tags appear. A structural check rather than an offset one:
        // the round-trip against the decoder is in `crates/zebc/tests`.
        assert!(bytes.windows(4).any(|w| w == 3u32.to_le_bytes()));
        assert!(bytes.windows(4).any(|w| w == TERM_VOCABULARY.to_le_bytes()));
        // The literal travels as its own bytes, not as an index into anything.
        assert!(bytes.windows(4).any(|w| w == b"take"));
    }

    #[test]
    fn a_grammar_slot_without_a_part_of_speech_is_still_refused() {
        // Blob encoding uses the same lowering and must reject a grammar
        // slot without its required part of speech.
        let refused = tree(
            "enum token tokWord;
property firstTokenIndex, lastTokenIndex, tokenList;
property noun;
dictionary cmdDict;
relation vocab(entity: Entity, word: Text, part: Vocabulary) many_to_many;
class Cmd: object it_ = nil;
grammar command(take): 'take' vocab->it_ : Cmd;
turn(toks){ return command.parseTokens(toks, cmdDict); }",
        );
        assert_eq!(blob(&refused).unwrap_err().code, "sem-vocabulary");
    }
}
