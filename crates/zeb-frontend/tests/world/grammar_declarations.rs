#![forbid(unsafe_code)]
use zeb_frontend::{
    parser::{self, Declaration, GrammarItem, Syntax},
    sema,
    source::{Encoding, Source},
};
fn parse(text: &str) -> Result<parser::Ast, zeb_frontend::Diagnostic> {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
}
#[test]
fn command_rule_preserves_capture_and_native_method_body() {
    let tree = parse("grammar commandPhrase; class FirstCommandProd: object; grammar firstCommandPhrase(commandOnly): commandPhrase->cmd_ : FirstCommandProd getNextCommandIndex(){return cmd_.getNextCommandIndex();};").unwrap();
    let rule = tree
        .nodes
        .iter()
        .find_map(|n| match &n.syntax {
            Syntax::Declaration(Declaration::Grammar(r)) if r.match_name.is_some() => Some(r),
            _ => None,
        })
        .unwrap();
    assert_eq!(rule.production, "firstCommandPhrase");
    assert_eq!(rule.tag.as_deref(), Some("commandOnly"));
    assert_eq!(
        rule.items,
        vec![
            GrammarItem::Symbol("commandPhrase".into()),
            GrammarItem::Capture("cmd_".into())
        ]
    );
    assert_eq!(tree.functions.len(), 1);
    // Both productions become static objects that can receive parseTokens.
    let objects: Vec<_> = tree
        .objects
        .iter()
        .filter_map(|id| match &tree.nodes[id.0].syntax {
            Syntax::Object { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert!(objects.contains(&"commandPhrase") && objects.contains(&"firstCommandPhrase"));
    sema::analyze(&tree).unwrap();
}
#[test]
fn alternatives_empty_groups_and_forward_productions_are_retained() {
    let tree = parse("grammar child; grammar door: ('door'->door_ | 'exit'->door_) 'to' ( | 'the') child->np_ : object;").unwrap();
    let rules: Vec<_> = tree
        .nodes
        .iter()
        .filter_map(|n| match &n.syntax {
            Syntax::Declaration(Declaration::Grammar(r)) => Some(r),
            _ => None,
        })
        .collect();
    assert!(rules[0].match_name.is_none());
    assert_eq!(
        rules[1]
            .items
            .iter()
            .filter(|x| **x == GrammarItem::Alternative)
            .count(),
        2
    );
    assert_eq!(
        rules[1].items.last(),
        Some(&GrammarItem::Capture("np_".into()))
    );
}
#[test]
fn malformed_rules_do_not_become_ordinary_objects() {
    for source in [
        "grammar p: ( 'a' : object;",
        "grammar p: ) : object;",
        "grammar p: ->x : object;",
        "grammar p: 'a'-> : object;",
    ] {
        assert!(parse(source).is_err(), "{source}");
    }
}
