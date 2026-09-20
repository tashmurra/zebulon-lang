#![forbid(unsafe_code)]
use zeb_frontend::{
    flow, llvm,
    parser::{self, Syntax},
    source::{Encoding, Source},
};
fn parse(text: &str) -> parser::Ast {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}

/// `p: Class { q = v }` defines a nested anonymous object and stores a
/// reference to it; Adv3 uses it for direction match phrases in `en_us.t`.
#[test]
fn nested_object_property_defines_an_anonymous_object() {
    let tree = parse(
        "class Dir: object dir = nil; north: object p = 1; \
         rule: object dirMatch: Dir { dir = north }; \
         main(){ return rule.dirMatch.dir == north; }",
    );
    let nested = tree
        .nodes
        .iter()
        .filter(|node| matches!(&node.syntax, Syntax::Object { name, .. } if name.starts_with("$nested")))
        .count();
    assert_eq!(nested, 1);
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, llvm::Target::MacX86_64).unwrap();
}

#[test]
fn nested_object_methods_are_rejected() {
    let source = Source::decode(
        b"class Dir: object d = nil; rule: object m: Dir { d() { return 1; } };".to_vec(),
        Encoding::Utf8,
    )
    .unwrap();
    assert_eq!(
        parser::parse_with(&source, parser::Model::Ownership)
            .unwrap_err()
            .code,
        "parse-expected"
    );
}
