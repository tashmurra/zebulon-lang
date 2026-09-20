#![forbid(unsafe_code)]
//! The two halves of the table format, checked against each other.
//!
//! `zeb_frontend::tables` writes the blob and `zeb_runtime::tables` reads it.
//! They are separate crates and share no code, so the format is an agreement
//! between two files rather than one type used twice — and an agreement with
//! nothing checking it is one that drifts. `zebc` is the only crate that can
//! see both, so the round trip lives here.
//!
//! Self-authored expectations: the format is Zebulon's own.
use zeb_frontend::{
    parser::{self, Ast, Model},
    source::{Encoding, Source},
    tables,
};
use zeb_runtime::grammar::StaticTerm;

fn tree(text: &str) -> Ast {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        Model::Lifetimes,
    )
    .expect("the world compiles")
}

/// One of every term shape the format can carry: a literal, a production, a
/// token enumerator, a part of speech and a wildcard, with and without a
/// capture, and a rule carrying a badness.
const WORLD: &str = "\
enum token tokWord;
property firstTokenIndex, lastTokenIndex, tokenList;
property badness;
property noun, adjective;
dictionary cmdDict;
relation vocab(entity: Entity, word: Text, part: Vocabulary) many_to_many;
class Thing: object name = 'thing';
coin: Thing name = 'coin' vocab = 'small coin';
class Cmd: object it_ = nil rest_ = nil word_ = nil;
class Noun: object w_ = nil;
grammar noun(one): vocab.noun->w_ : Noun;
grammar command(take): [badness 3] 'take' noun->it_ : Cmd;
grammar command(say):  'say' tokWord->word_ : Cmd;
grammar command(rest): 'rest' * : Cmd;
turn(toks){ \"x\"; return command.parseTokens(toks, cmdDict); }";

#[test]
fn a_blob_written_by_the_front_end_reads_back_the_same_tables() {
    let ast = tree(WORLD);
    let bytes = tables::blob(&ast).expect("the tables encode");
    let read = zeb_runtime::tables::decode(&bytes).expect("and decode");

    // Every production the lowering found has a slot or is anonymous.
    assert!(!read.grammar.productions.is_empty());
    // `badness` is declared, so a match can report one.
    assert!(read.grammar.badness.is_some());
    assert!(read.grammar.first_token_index.is_some());
    assert!(read.grammar.last_token_index.is_some());
    assert!(read.grammar.token_list.is_some());

    // The four rules, and the badness that only one of them carries.
    assert_eq!(read.grammar.rules.len(), 4);
    assert_eq!(
        read.grammar.rules.iter().filter(|r| r.badness == 3).count(),
        1,
        "exactly one rule was written with [badness 3]"
    );

    // One of every term shape survived the round trip.
    let terms: Vec<StaticTerm> = read
        .grammar
        .rules
        .iter()
        .flat_map(|rule| rule.elements.iter().map(|e| e.term))
        .collect();
    assert!(terms.contains(&StaticTerm::Literal("take")));
    assert!(terms.contains(&StaticTerm::Literal("say")));
    assert!(terms.contains(&StaticTerm::Star));
    assert!(
        terms.iter().any(|t| matches!(t, StaticTerm::Production(_))),
        "a rule names another production"
    );
    assert!(
        terms.iter().any(|t| matches!(t, StaticTerm::Token(_))),
        "a rule names a token enumerator"
    );
    assert!(
        terms.iter().any(|t| matches!(t, StaticTerm::Vocabulary(_))),
        "a rule names a part of speech"
    );

    // Captures come back attached to the element that had them.
    assert!(
        read.grammar
            .rules
            .iter()
            .any(|r| r.elements.iter().any(|e| e.capture.is_some())),
        "a capture survived"
    );

    // And the literal pool, which shares the blob with the grammar.
    assert!(
        read.literals.contains(&"small coin"),
        "the program's literals came back: {:?}",
        read.literals
    );
}

#[test]
fn a_program_with_no_grammar_still_round_trips() {
    // The degenerate case is the one an emitter gets wrong: a blob of counts
    // that are all zero still has to read back as empty rather than as an
    // error, because a program may have no grammar at all.
    let ast = tree("class Thing: object name = 'thing';\nmain(){ return 1; }");
    let bytes = tables::blob(&ast).expect("the tables encode");
    let read = zeb_runtime::tables::decode(&bytes).expect("and decode");
    assert!(read.grammar.rules.is_empty());
}

#[test]
fn a_blob_the_front_end_wrote_is_rejected_when_a_byte_moves() {
    let ast = tree(WORLD);
    let good = tables::blob(&ast).expect("the tables encode");
    // The magic is what stops a table being read as the wrong shape.
    let mut wrong = good.clone();
    wrong[0] = b'Z' ^ 0xff;
    assert!(zeb_runtime::tables::decode(&wrong).is_err());
    // And a blob that stops early is refused rather than read as a shorter one.
    assert!(zeb_runtime::tables::decode(&good[..good.len() - 1]).is_err());
}
