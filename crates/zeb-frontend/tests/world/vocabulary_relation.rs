#![forbid(unsafe_code)]
//! a relation whose right side is `Text` is vocabulary, and a grammar
//! slot declared against it binds the entities a word names. Expectations are
//! self-authored; TADS binds the token's text, and this deliberately does not.
use zeb_frontend::{
    flow, grammar,
    llvm::{self, Target},
    parser::{self, Model},
    source::{Encoding, Source},
};

fn tree(text: &str) -> Result<parser::Ast, zeb_frontend::Diagnostic> {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        Model::Lifetimes,
    )
}

const WORLD: &str = "\
enum token tokWord;
property firstTokenIndex, lastTokenIndex, tokenList;
dictionary cmdDict;
relation names(entity: Entity, word: Text) many_to_many;
class Thing: object name = 'thing';
coin: Thing name = 'coin' names = 'coin';
class Prod: object tag = nil item_ = nil;
grammar command(get): 'get' names->item_ : Prod tag = 'get';
turn(toks){ return command.parseTokens(toks, cmdDict); }";

#[test]
fn a_vocabulary_slot_compiles_and_emits_a_vocabulary_term() {
    let ast = tree(WORLD).unwrap();
    flow::check(&ast).unwrap();
    let data = grammar::rust_data(&ast).unwrap();
    // The slot is not a part of speech and not a token enumerator: its capture
    // has to reach the dictionary and bind what it finds.
    assert!(data.contains("Vocabulary("), "{data}");
    assert!(!data.contains("Speech("), "{data}");
    llvm::emit_objects(&ast, Target::MacX86_64).unwrap();
}

#[test]
fn an_ordinary_relation_is_unchanged_and_text_belongs_on_the_right() {
    let plain = tree(
        "relation contains(container: Entity, item: Entity) one_to_many reverse location;\n\
         a: object; b: object;\n\
         turn(toks){ contains.set(a, b); return contains.all(a); }",
    )
    .unwrap();
    assert!(!plain.relations[0].vocabulary);
    flow::check(&plain).unwrap();

    assert_eq!(
        tree("relation names(word: Text, entity: Entity) many_to_many; turn(t){return nil;}")
            .unwrap_err()
            .code,
        "parse-expected"
    );
}

const DATA: &str = "\
enum token tokWord;
property firstTokenIndex, lastTokenIndex, tokenList;
dictionary cmdDict;
relation names(entity: Entity, word: Text) many_to_many;
class Thing: object name = 'thing';
coin: Thing name = 'coin' names = 'coin';
";

fn compiles(body: &str) -> Result<(), zeb_frontend::Diagnostic> {
    let ast = tree(&format!("{DATA}turn(toks){{ {body} }}"))?;
    flow::check(&ast)?;
    llvm::emit_objects(&ast, Target::MacX86_64)?;
    Ok(())
}

/// A whole program through flow and emission, for cases that declare their own
/// dictionary and properties rather than reusing DATA.
fn full(text: &str) -> Result<(), zeb_frontend::Diagnostic> {
    let ast = tree(text)?;
    flow::check(&ast)?;
    llvm::emit_objects(&ast, Target::MacX86_64)?;
    Ok(())
}

/// Grammar emission alone, which is where a slot's term is classified.
fn grammar_data(text: &str) -> Result<String, zeb_frontend::Diagnostic> {
    let ast = tree(text)?;
    flow::check(&ast)?;
    grammar::rust_data(&ast)
}

/// the four operations a vocabulary relation answers as data.
#[test]
fn a_vocabulary_relation_answers_as_data() {
    compiles("return names.all(coin);").unwrap();
    compiles("return names.contains(coin, 'coin');").unwrap();
    compiles("names.set(coin, 'shilling'); return nil;").unwrap();
    compiles("names.unset(coin, 'coin'); return nil;").unwrap();
}

/// A word is not a container, and a set of words has no single partner, so the
/// operations that mean those things are refused rather than answering nothing.
#[test]
fn the_operations_that_do_not_apply_are_refused() {
    for body in [
        "return names.get(coin);",
        "return names.outermost(coin);",
        "return names.descendants(coin);",
        "return names.ancestors(coin);",
    ] {
        assert_eq!(
            compiles(body).unwrap_err().code,
            "sem-vocabulary",
            "{body} was accepted"
        );
    }
}

/// Reading word-to-entity is what a grammar slot does; the relation's reverse
/// name is not a second way in.
#[test]
fn a_vocabulary_relations_reverse_name_is_not_a_table() {
    let source = "\
enum token tokWord;
property firstTokenIndex, lastTokenIndex, tokenList;
dictionary cmdDict;
relation names(entity: Entity, word: Text) many_to_many reverse named_by;
coin: object names = 'coin';
turn(toks){ return named_by.all(coin); }";
    let ast = tree(source).unwrap();
    assert_eq!(flow::check(&ast).unwrap_err().code, "sem-vocabulary");
}

/// Its rows live in a dictionary, so one has to exist before the declaration.
#[test]
fn a_vocabulary_relation_needs_a_dictionary_first() {
    assert_eq!(
        tree("relation names(entity: Entity, word: Text) many_to_many; turn(t){return nil;}")
            .unwrap_err()
            .code,
        "parse-expected"
    );
    // Declared after the relation is still too late: the relation records the one
    // in force where it was written.
    assert_eq!(
        tree(
            "relation names(entity: Entity, word: Text) many_to_many;\n\
             dictionary cmdDict;\n\
             turn(t){return nil;}"
        )
        .unwrap_err()
        .code,
        "parse-expected"
    );
}

/// A vocabulary read is a relation read: only the storage behind it differs, so
/// it carries the same ownership obligations as `contains.all`, under either
/// memory model. This pins the parity rather than the current answer.
#[test]
fn reading_the_words_matches_an_ordinary_relations_read() {
    let under = |model, relation: &str, body: &str| {
        parser::parse_with(
            &Source::decode(
                format!(
                    "dictionary cmdDict;\n\
                     relation names(entity: Entity, word: Text) many_to_many;\n\
                     relation contains(container: Entity, item: Entity) one_to_many;\n\
                     coin: object names = 'coin';\n\
                     turn(toks){{ {body} }}"
                )
                .replace("RELATION", relation)
                .as_bytes()
                .to_vec(),
                Encoding::Utf8,
            )
            .unwrap(),
            model,
        )
        .and_then(|ast| flow::check(&ast).map(|_| ()))
    };
    for model in [Model::Lifetimes, Model::Ownership] {
        let vocabulary = under(model, "names", "local w = names.all(coin); return nil;");
        let ordinary = under(
            model,
            "contains",
            "local w = contains.all(coin); return nil;",
        );
        assert_eq!(
            vocabulary.as_ref().err().map(|e| e.code),
            ordinary.as_ref().err().map(|e| e.code),
            "the two reads disagree under {model:?}"
        );
    }
}

// a labelled vocabulary relation's label is its part of speech,
// written as a property address because that is how TADS names a property.
// A relation-family label is an enumerator, unlike a vocabulary property.

const PARTS: &str = "dictionary cmdDict;\nproperty noun, adjective;\nrelation vocab(entity: Entity, word: Text, part: Vocabulary) many_to_many;\ndesk: object n = 1;\n";

#[test]
fn a_vocabulary_relation_may_have_a_part_of_speech() {
    full(&format!(
        "{PARTS}turn(toks) {{ vocab.set(desk, 'table', &noun); vocab.unset(desk, 'oak', &adjective); return nil; }}"
    ))
    .unwrap();
}

#[test]
fn every_part_of_speech_operation_takes_the_label() {
    full(&format!(
        "{PARTS}turn(toks) {{ vocab.contains(desk, 'table', &noun); return vocab.all(desk, &adjective); }}"
    ))
    .unwrap();
}

/// The part of speech is a property, not an enumerator, so a bare name is
/// refused with the spelling that works.
#[test]
fn a_bare_name_is_not_a_part_of_speech() {
    let error = full(&format!(
        "{PARTS}turn(toks) {{ vocab.set(desk, 'table', noun); return nil; }}"
    ))
    .unwrap_err();
    assert_eq!(error.code, "sem-vocabulary");
    assert!(error.message.contains("&noun"), "{error:?}");
}

#[test]
fn a_part_of_speech_must_be_a_declared_property() {
    let error = full(&format!(
        "{PARTS}turn(toks) {{ vocab.set(desk, 'table', &nosuch); return nil; }}"
    ))
    .unwrap_err();
    assert_eq!(error.code, "sem-vocabulary");
}

/// Omitting it is a short call, not a misshapen argument, and says so.
#[test]
fn omitting_the_part_of_speech_is_refused_as_a_missing_one() {
    let error = full(&format!("{PARTS}turn(toks) {{ return vocab.all(desk); }}")).unwrap_err();
    assert_eq!(error.code, "sem-vocabulary");
    assert!(error.message.contains("last"), "{error:?}");
}

/// The words are filed under the part of speech, so the relation's own forward
/// name indexes nothing and a slot has nowhere to say which part it wants.
/// Refused rather than compiled into a match that never fires.
#[test]
fn a_grammar_slot_cannot_name_a_labelled_vocabulary_relation() {
    let error = grammar_data(&format!(
        "enum token tokWord;\n{PARTS}class Prod: object tag = nil item_ = nil;\ngrammar command(take): 'take' vocab->item_ : Prod tag = 'take';\nturn(toks) {{ return nil; }}"
    ))
    .unwrap_err();
    assert_eq!(error.code, "sem-vocabulary");
    assert!(error.message.contains("part of speech"), "{error:?}");
}

// an adv3Lite vocab string splits into rows while compiling.

const STRING: &str = "dictionary cmdDict;\nproperty noun, adjective;\nrelation vocab(entity: Entity, word: Text, part: Vocabulary) many_to_many;\nclass Thing: object name = nil;\n";

#[test]
fn a_vocab_string_declares_words_in_both_parts_of_speech() {
    full(&format!(
        "{STRING}desk: Thing vocab = 'oak desk; large wooden; table counter';\nturn(toks) {{ return vocab.all(desk, &noun); }}"
    ))
    .unwrap();
}

/// The pronoun section sets adv3Lite `Mentionable` properties, which are not
/// language features. Refused rather than quietly dropped.
#[test]
fn a_pronoun_section_is_refused() {
    let error = full(&format!(
        "{STRING}desk: Thing vocab = 'desk; big; table; it';\nturn(toks) {{ return nil; }}"
    ))
    .unwrap_err();
    assert_eq!(error.code, "sem-vocabulary");
    assert!(error.message.contains("pronoun"), "{error:?}");
}

/// Sections are separated inside one literal, not by writing several.
#[test]
fn a_vocab_string_is_one_literal() {
    let error = full(&format!(
        "{STRING}desk: Thing vocab = 'desk' 'table';\nturn(toks) {{ return nil; }}"
    ))
    .unwrap_err();
    assert!(format!("{error:?}").contains("one literal"), "{error:?}");
}

/// An unlabelled vocabulary relation accepts a list of words.
#[test]
fn an_unlabelled_relation_still_takes_a_word_list() {
    full("dictionary cmdDict;\nrelation names(entity: Entity, word: Text) many_to_many;\ncoin: object names = 'coin' 'shilling';\nturn(toks) { return names.all(coin); }")
        .unwrap();
}

// a grammar slot names one part of speech.

const SLOT: &str = "enum token tokWord;\nproperty firstTokenIndex, lastTokenIndex, tokenList;\ndictionary cmdDict;\nproperty noun, adjective;\nrelation vocab(entity: Entity, word: Text, part: Vocabulary) many_to_many;\nclass Thing: object name = nil;\ndesk: Thing vocab = 'oak desk';\nclass Prod: object tag = nil item_ = nil;\n";

#[test]
fn a_slot_may_name_a_part_of_speech() {
    let data = grammar_data(&format!(
        "{SLOT}grammar command(take): 'take' vocab.noun->item_ : Prod tag = 'take';\nturn(toks) {{ return nil; }}"
    ))
    .unwrap();
    // It matches as any vocabulary slot does; only the property differs.
    assert!(data.contains("Vocabulary("), "{data}");
}

/// Two slots naming different parts look under different properties, which is
/// the whole point — otherwise an adjective would answer a noun slot.
#[test]
fn two_parts_of_speech_resolve_to_different_properties() {
    let data = grammar_data(&format!(
        "{SLOT}grammar command(take): 'take' vocab.noun->item_ : Prod tag = 'take';\ngrammar command(rub): 'rub' vocab.adjective->item_ : Prod tag = 'rub';\nturn(toks) {{ return nil; }}"
    ))
    .unwrap();
    let mut terms: Vec<&str> = data
        .match_indices("Vocabulary(")
        .map(|(at, _)| &data[at..data[at..].find(')').map(|end| at + end).unwrap_or(at)])
        .collect();
    terms.sort_unstable();
    terms.dedup();
    assert_eq!(
        terms.len(),
        2,
        "both slots looked under one property: {data}"
    );
}

/// The bare name is refused: a slot matching every part would bind an entity by
/// its adjective, and `take oak` answering the desk is what parts of speech are
/// there to prevent.
#[test]
fn a_slot_must_name_a_part_of_speech() {
    let error = grammar_data(&format!(
        "{SLOT}grammar command(take): 'take' vocab->item_ : Prod tag = 'take';\nturn(toks) {{ return nil; }}"
    ))
    .unwrap_err();
    assert_eq!(error.code, "sem-vocabulary");
    assert!(error.message.contains("vocab.noun"), "{error:?}");
}

#[test]
fn a_slots_part_of_speech_must_be_declared() {
    let error = grammar_data(&format!(
        "{SLOT}grammar command(take): 'take' vocab.nosuch->item_ : Prod tag = 'take';\nturn(toks) {{ return nil; }}"
    ))
    .unwrap_err();
    assert_eq!(error.code, "sem-vocabulary");
}
