#![forbid(unsafe_code)]
use zeb_frontend::{
    grammar::{self, Term},
    parser,
    source::{Encoding, Source},
};
fn parse(text: &str) -> parser::Ast {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}
#[test]
fn forward_recursive_rules_keep_production_and_match_identity() {
    let tree = parse(
        "grammar command; grammar command(more): command->previous 'then' command->next : object; grammar command(look): 'look' : object;",
    );
    let grammar = grammar::lower(&tree).unwrap();
    assert_eq!(grammar.productions, [Some("command")]);
    assert_eq!(grammar.rules.len(), 2);
    let more = &grammar.rules[0];
    assert_eq!(more.match_name, Some("command(more)"));
    assert_eq!(more.elements[0].term, Term::Production(more.production));
    assert_eq!(more.elements[0].capture, Some("previous"));
    assert_eq!(more.elements[2].capture, Some("next"));
    assert_eq!(grammar.rules[1].elements[0].term, Term::Literal("look"));
}
#[test]
fn groups_are_linear_and_empty_alternatives_are_real_rules() {
    let tree =
        parse("grammar door: ('door'->word | 'exit'->word) ( | 'the') ('room' | 'hall') : object;");
    let grammar = grammar::lower(&tree).unwrap();
    assert_eq!(grammar.productions.len(), 4);
    assert_eq!(grammar.rules.len(), 7);
    assert_eq!(
        grammar
            .rules
            .iter()
            .filter(|r| r.elements.is_empty())
            .count(),
        1
    );
    let root = grammar
        .rules
        .iter()
        .find(|r| r.match_name.is_some())
        .unwrap();
    assert_eq!(root.elements.len(), 3);
    assert_eq!(
        grammar
            .rules
            .iter()
            .filter(|r| r.elements.iter().any(|e| e.capture == Some("word")))
            .count(),
        2
    );
}
#[test]
fn duplicate_rule_names_and_unresolved_symbol_kinds_are_errors() {
    for (source, code) in [
        (
            "grammar p(a): 'a' : object; grammar p(a): 'b' : object;",
            "grammar-duplicate",
        ),
        (
            "grammar p: noun->obj : object;",
            "grammar-symbol-unavailable",
        ),
    ] {
        assert_eq!(grammar::rust_data(&parse(source)).unwrap_err().code, code);
    }
    // Lowering keeps the unclassified symbol for emission-time classification.
    let tree = parse("grammar p: noun->obj : object;");
    assert_eq!(
        grammar::lower(&tree).unwrap().rules[0].elements[0].term,
        Term::Symbol("noun")
    );
}

#[test]
fn emitted_tables_classify_tokens_speech_badness_and_captures() {
    let tree = parse(
        "enum token tokWord; dictionary property noun; property firstTokenIndex; grammar cmd(x): [badness 50] 'x' noun->n_ tokWord->w_ * : object;",
    );
    let data = grammar::rust_data(&tree).unwrap();
    assert!(data.contains("badness: 50"), "{data}");
    assert!(data.contains("StaticTerm::Literal(\"x\")"), "{data}");
    assert!(data.contains("StaticTerm::Speech("), "{data}");
    assert!(data.contains("StaticTerm::Token(0)"), "{data}");
    assert!(data.contains("StaticTerm::Star"), "{data}");
    assert!(data.contains("first_token_index: Some("), "{data}");
    assert!(data.contains("last_token_index: None"), "{data}");
}
